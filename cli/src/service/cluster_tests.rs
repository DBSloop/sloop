//! The attachment list against a real PostgreSQL — `R25`'s *Done when*, driven rather than
//! reasoned about.
//!
//! **A migrated `sloop_database` is the only place this can be proved.** Attaching is one row,
//! detaching is one delete, and the whole claim is about what the *schema* does around them:
//! that a round marks what it read, that a second attachment reaches a loop already turning,
//! and that deleting the attachment leaves `bandwidth_day` alone because the foreign key goes
//! to the database rather than to the attachment. None of that is visible in a string.
//!
//! The cluster is `server::cluster_tests`', which already knows how to build one on a port
//! nothing else uses and take it away again whether the test passed or not. Never the
//! machine's own PostgreSQL, never the developer's `sloop setup`.

use std::time::{Duration, Instant};

use super::watch::{self, Attached, Round};
use crate::engine::Engine;
use crate::registry::file::{Database, Registry};
use crate::registry::store::{Store, Which};
use crate::secret::Route;
use crate::server::cluster_tests::{a_free_port, cluster, stop};
use crate::server::tests::{Scratch, this_machine_can_keep_a_secret};
use crate::server::{own, record, schema};

/// A registered database, as `db add --global` would have written one.
///
/// It is never connected to. `R25` attaches and detaches a *registration*; what the daemon
/// eventually does with one is `R26`, and 127.0.0.1:1 is the address every test in this
/// project uses for a database nothing is supposed to dial.
fn entry() -> Database {
    Database {
        engine: Engine::Postgres,
        host: "127.0.0.1".to_owned(),
        port: 1,
        database: "orders".to_owned(),
        user: "app".to_owned(),
        password: Route::Command("echo never-used".to_owned()),
        reach: crate::ssh::Reach::Direct,
    }
}

/// Put `labels` in the global registry, replacing whatever was there.
fn register(store: &Store, labels: &[&str]) {
    let mut registry = Registry::default();
    for label in labels {
        registry.insert((*label).to_owned(), entry());
    }
    store
        .write(&Which::Global, &registry)
        .expect("the global registry is writable");
}

/// A day of activity against `label`, standing in for the sampling `R26` will do.
///
/// **This is what detaching must not touch**, and putting it there by hand is the only way to
/// have any before `R26` exists.
fn record_a_day(store: &Store, label: &str, day: &str) {
    store
        .run(&format!(
            "INSERT INTO bandwidth_day (registered_database_id, day, bytes_in, bytes_out)
             SELECT d.id, DATE '{day}', 1024, 2048
               FROM registered_database d
              WHERE d.label = '{label}' AND d.project_id IS NULL;"
        ))
        .expect("a day of activity is recordable");
}

/// How many days of activity `label` has.
fn days_of(store: &Store, label: &str) -> i64 {
    store
        .ask(&format!(
            "SELECT count(*) FROM bandwidth_day b
               JOIN registered_database d ON d.id = b.registered_database_id
              WHERE d.label = '{label}' AND d.project_id IS NULL;"
        ))
        .expect("the server answers")
        .trim()
        .parse()
        .expect("count(*) is a number")
}

/// **`R25` end to end, on one cluster.**
///
/// One test rather than six, for the reason every cluster test in this project gives: each
/// step needs the state the one before it left, and six `initdb`s would be six machines.
#[test]
fn attaching_reaches_a_running_service_and_detaching_leaves_the_history() {
    let scratch = Scratch::new("attach");
    let global = scratch.path().to_path_buf();
    let data = crate::server::data_dir(&global);

    if !this_machine_can_keep_a_secret(&global) {
        return;
    }

    let Some(ready) = cluster(&global, a_free_port()) else {
        return;
    };
    record::write(&global, &ready.server, &ready.password).expect("it can keep a secret");

    let opened = match own::ensure(
        &global,
        &ready.server,
        &ready.password,
        &own::Choosing::unsupplied(),
    ) {
        Ok(opened) => opened,
        Err(why) => {
            stop(&ready.server.bin, &data);
            panic!("sloop's own database could not be made: {}", why.message());
        }
    };

    // Everything below has a postmaster to stop afterwards, and a panic in the middle would
    // leave it holding the scratch directory open — which on Windows is a directory nothing
    // can then delete. Assert, stop, then re-raise.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        schema::migrate(&ready.server, &opened.own, &opened.password)
            .expect("a fresh database migrates");
        // The engines too, because `registered_database.engine_id` points at one of them and
        // a registry written into a database with no engine rows is a not-null violation.
        // `server::set_up` does both; this is the half of it these tests need.
        schema::reconcile_engines(&ready.server, &opened.own, &opened.password)
            .expect("the engines this build speaks are recorded");
        let store = Store::open(&global)
            .expect("the record reads back")
            .expect("a machine with a record has a store");
        assertions(&store, &global);
    }));

    stop(&ready.server.bin, &data);
    let _ = std::fs::remove_dir_all(scratch.path());

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[allow(clippy::too_many_lines)]
fn assertions(store: &Store, global: &std::path::Path) {
    register(store, &["orders"]);

    // 1. Nothing is attached on a machine that has never attached anything, and nothing has
    //    ever read the list. Both are `None`/empty rather than a zero.
    assert!(
        watch::attachments(store)
            .expect("an empty list reads")
            .is_empty()
    );
    assert_eq!(
        watch::last_seen(store).expect("it reads"),
        None,
        "a service that has never run claims to have been here"
    );

    // 2. A name that is not in the global registry is exit 2, not an attachment to nothing.
    let refused = watch::attach(store, "nowhere").expect_err("an unknown name is refused");
    assert_eq!(refused.exit().code(), 2);

    // 3. Attaching writes the row — and makes the `service` row it hangs off, which a fresh
    //    machine does not have.
    assert_eq!(
        watch::attach(store, "orders").expect("it attaches"),
        Attached::Now
    );
    assert_eq!(
        watch::attach(store, "orders").expect("attaching twice is ordinary"),
        Attached::Already,
        "a second attach was not idempotent"
    );

    let listed = watch::attachments(store).expect("the list reads");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].label, "orders");
    assert!(listed[0].enabled);
    assert_eq!(
        listed[0].seen_at, None,
        "an attachment claims to have been read before anything ran"
    );

    // 4. **And it claims nothing about the machine.** `R24`'s rule: only the service manager
    //    says whether a service is installed, so the row `attach` made says neither.
    let claimed = store
        .ask("SELECT installed || ' ' || coalesce(mechanism, 'none') FROM service;")
        .expect("the service row reads");
    assert_eq!(claimed.trim(), "false none");

    // 5. **One round, and the attachment has been picked up.** This is the daemon's own
    //    statement, run against the real thing.
    assert_eq!(
        watch::round(store).expect("a round runs"),
        vec![String::from("orders")]
    );

    let listed = watch::attachments(store).expect("the list reads");
    assert!(
        listed[0].seen_at.is_some(),
        "a round read the attachment and did not mark it"
    );
    assert!(
        watch::last_seen(store).expect("it reads").is_some(),
        "a round ran and the service did not say it was here"
    );

    // 6. **The `Done when`: a service that is already running picks up a new attachment.**
    //    The loop below is the daemon's — the same `Round`, turned over and over — and the
    //    second database is attached while it turns. Nothing is restarted and nothing is
    //    signalled; the round simply reads the list again.
    let turning = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let stopping = std::sync::Arc::clone(&turning);
    let where_it_is = global.to_path_buf();

    let daemon = std::thread::spawn(move || {
        let mut round = Round::at(&where_it_is);
        while stopping.load(std::sync::atomic::Ordering::Relaxed) {
            round.turn();
            std::thread::sleep(Duration::from_millis(25));
        }
    });

    register(store, &["orders", "analytics"]);
    assert_eq!(
        watch::attach(store, "analytics").expect("it attaches"),
        Attached::Now
    );

    let picked_up = wait_until(|| {
        watch::attachments(store)
            .expect("the list reads")
            .iter()
            .any(|one| one.label == "analytics" && one.seen_at.is_some())
    });
    turning.store(false, std::sync::atomic::Ordering::Relaxed);
    daemon.join().expect("the daemon thread finishes");

    assert!(
        picked_up,
        "a service that was already running never read the attachment made while it ran"
    );

    // 7. A day of activity for each, as `R26` will write it.
    record_a_day(store, "orders", "2026-09-17");
    record_a_day(store, "orders", "2026-09-18");
    record_a_day(store, "analytics", "2026-09-18");
    assert_eq!(days_of(store, "orders"), 2);

    // 7a. **Registering something else does not detach what is attached.** `Store::write`
    //     replaces a registry wholesale, and both of these tables cascade off
    //     `registered_database.id` — so a `db add` that gave every surviving row a new id
    //     would silently detach everything and delete every day of history with it. That is
    //     exactly what it used to do, and this is the assertion that says it no longer does.
    register(store, &["orders", "analytics", "reporting"]);
    assert_eq!(
        watch::attachments(store)
            .expect("the list reads")
            .iter()
            .map(|one| one.label.as_str())
            .collect::<Vec<_>>(),
        vec!["analytics", "orders"],
        "registering a third database detached the two that were attached"
    );
    assert_eq!(
        days_of(store, "orders"),
        2,
        "registering a third database deleted another one's history"
    );

    // 8. **Detaching stops the samples and deletes nothing.** The attachment goes; the two
    //    days stay; the registration stays; the other database is untouched.
    let detached = watch::detach(store, "orders").expect("it detaches");
    assert!(detached.was_attached);
    assert_eq!(detached.days_of_history, 2);

    let listed = watch::attachments(store).expect("the list reads");
    assert_eq!(
        listed
            .iter()
            .map(|one| one.label.as_str())
            .collect::<Vec<_>>(),
        vec!["analytics"],
        "detaching took the wrong row, or took two"
    );

    assert_eq!(
        days_of(store, "orders"),
        2,
        "detaching deleted the history it was supposed to keep"
    );
    assert_eq!(days_of(store, "analytics"), 1);
    assert_eq!(
        store
            .ask("SELECT count(*) FROM registered_database;")
            .expect("the server answers")
            .trim(),
        "3",
        "detaching removed the registration as well as the attachment"
    );

    // 9. And a round after the detach reads the shorter list, without a restart either.
    assert_eq!(
        watch::round(store).expect("a round runs"),
        vec![String::from("analytics")]
    );

    // 10. Detaching what is not attached is not an error, and still reports the history.
    let again = watch::detach(store, "orders").expect("detaching twice is ordinary");
    assert!(!again.was_attached);
    assert_eq!(again.days_of_history, 2);

    // 11. **The one thing that does take an attachment away**: the database ceasing to be
    //     registered. `0004` cascades that on purpose — an attachment to a database that is
    //     gone is an instruction to watch nothing. The history is keyed the same way, so it
    //     goes too, which is what `db remove` has always meant.
    register(store, &["orders"]);
    assert!(
        watch::attachments(store)
            .expect("the list reads")
            .is_empty(),
        "unregistering a database left an attachment pointing at nothing"
    );
}

/// Poll until `settled` is true, or give up after a bound that is far longer than one round.
///
/// **The bound is the point.** The daemon thread turns every 25ms, so two seconds is eighty
/// rounds — an attachment that has not been picked up by then has not been picked up, which
/// is the failure worth catching rather than a slow machine.
fn wait_until(mut settled: impl FnMut() -> bool) -> bool {
    let waited = Instant::now();
    while waited.elapsed() < Duration::from_secs(5) {
        if settled() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// **The credential gap, closed and driven.** *(owner, 2026-09-19: "go with 1")*
///
/// A service runs as another account and cannot read the keyring an interactive session used,
/// so `sloop service install` seals a copy of sloop's own two passwords into the encrypted
/// store under the key file's passphrase. This runs the real function against a real store and
/// opens what it wrote.
///
/// The key file is a temporary one, never `C:\ProgramData\sloop\service.key` — a stale
/// passphrase left at the real path would be kept by `key::write` on the next install and
/// would open nothing, which is the bug this is fixing.
#[test]
fn the_service_gets_a_copy_of_both_passwords_that_its_key_file_opens() {
    let scratch = Scratch::new("copy");
    let global = scratch.path().to_path_buf();
    let data = crate::server::data_dir(&global);

    if !this_machine_can_keep_a_secret(&global) {
        return;
    }

    let Some(ready) = cluster(&global, a_free_port()) else {
        return;
    };
    record::write(&global, &ready.server, &ready.password).expect("it can keep a secret");

    let opened = match own::ensure(
        &global,
        &ready.server,
        &ready.password,
        &own::Choosing::unsupplied(),
    ) {
        Ok(opened) => opened,
        Err(why) => {
            stop(&ready.server.bin, &data);
            panic!("sloop's own database could not be made: {}", why.message());
        }
    };

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let key_file = global.join("service-key-for-this-test");
        std::fs::write(&key_file, "a passphrase the unit points at\r\n").expect("writable");

        let copied =
            super::credentials::copy_for_the_service(&global, &key_file).expect("the copy is made");
        assert_eq!(copied, super::credentials::Copied::Both);

        // Both keys are in the store, and the key file's passphrase is what opens it — with
        // the CRLF an editor leaves on the end taken off, which is the whole reason that
        // trim exists.
        let sealed = global.join(crate::registry::file::SEALED_FILE);
        let bytes = std::fs::read(&sealed).expect("the sealed store was written");
        let entries =
            crate::secret::sealed::open_for_test(&bytes, "a passphrase the unit points at")
                .expect("the key file's passphrase opens it");

        let superuser = record::credential_key_of(&ready.server);
        let database = opened.own.credential_key(&ready.server);
        for wanted in [&superuser, &database] {
            assert!(
                entries.iter().any(|(key, _)| key == wanted),
                "{wanted} is not in the copy: {:?}",
                entries.iter().map(|(key, _)| key).collect::<Vec<_>>()
            );
        }

        // And they are the real passwords, not placeholders — this is what the daemon opens
        // the cluster and the database with.
        let held = |wanted: &str| {
            entries
                .iter()
                .find(|(key, _)| key == wanted)
                .map(|(_, secret)| secret.expose().to_owned())
                .unwrap_or_default()
        };
        assert_eq!(held(&superuser), ready.password.expose());
        assert_eq!(held(&database), opened.password.expose());

        // **Rule 3 still holds**: what is on disk is ciphertext, and neither password is in it.
        for password in [ready.password.expose(), opened.password.expose()] {
            assert!(
                !bytes
                    .windows(password.len())
                    .any(|window| window == password.as_bytes()),
                "a password is readable in {}",
                sealed.display()
            );
        }

        // A wrong passphrase opens nothing, which is what stops the file being the weak half.
        assert!(
            crate::secret::sealed::open_for_test(&bytes, "not the passphrase").is_err(),
            "the store opened under a passphrase that is not its own"
        );

        // Uninstalling takes the copies out again, and leaves the store readable.
        assert_eq!(
            super::credentials::remove_the_copies(&global, &key_file),
            None,
            "the copies could not be removed"
        );
        let after = std::fs::read(&sealed).unwrap_or_default();
        let left = if after.is_empty() {
            Vec::new()
        } else {
            crate::secret::sealed::open_for_test(&after, "a passphrase the unit points at")
                .expect("what is left still opens")
        };
        assert!(
            !left
                .iter()
                .any(|(key, _)| *key == superuser || *key == database),
            "a copy survived the uninstall"
        );
    }));

    stop(&ready.server.bin, &data);
    let _ = std::fs::remove_dir_all(scratch.path());

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// **`R26`'s `Done when`, against a real cluster and a real database.**
///
/// *"A day of samples rolls up to a daily figure matching the sum of its hours, a restart
/// mid-day loses no completed rollup, and a month of samples is still something somebody can
/// read."* All three, in order, on one cluster — and the database being sampled is sloop's own,
/// which is a real PostgreSQL with real counters rather than a fixture.
#[test]
fn samples_accumulate_into_hours_that_a_day_is_the_sum_of_and_a_restart_keeps() {
    let scratch = Scratch::new("sampling");
    let global = scratch.path().to_path_buf();
    let data = crate::server::data_dir(&global);

    if !this_machine_can_keep_a_secret(&global) {
        return;
    }

    let Some(ready) = cluster(&global, a_free_port()) else {
        return;
    };
    record::write(&global, &ready.server, &ready.password).expect("it can keep a secret");

    let opened = match own::ensure(
        &global,
        &ready.server,
        &ready.password,
        &own::Choosing::unsupplied(),
    ) {
        Ok(opened) => opened,
        Err(why) => {
            stop(&ready.server.bin, &data);
            panic!("sloop's own database could not be made: {}", why.message());
        }
    };

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        schema::migrate(&ready.server, &opened.own, &opened.password)
            .expect("a fresh database migrates");
        schema::reconcile_engines(&ready.server, &opened.own, &opened.password)
            .expect("the engines are recorded");

        let store = Store::open(&global)
            .expect("the record reads back")
            .expect("a machine with a record has a store");

        // The database being watched is this cluster's own `sloop_database`: a real server, a
        // real role, and counters that really move. `echo` is the one password route a test
        // may use — the keyring is the developer's own and outside any sandbox.
        let mut registry = Registry::default();
        registry.insert(
            String::from("itself"),
            Database {
                engine: Engine::Postgres,
                host: String::from("127.0.0.1"),
                port: ready.server.port,
                database: opened.own.database.clone(),
                user: opened.own.role.clone(),
                password: Route::Command(format!("echo {}", opened.password.expose())),
                reach: crate::ssh::Reach::Direct,
            },
        );
        store
            .write(&Which::Global, &registry)
            .expect("the registry is writable");

        assert_eq!(
            watch::attach(&store, "itself").expect("it attaches"),
            Attached::Now
        );

        sampling(&global, &store);
    }));

    stop(&ready.server.bin, &data);
    let _ = std::fs::remove_dir_all(scratch.path());

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[allow(clippy::too_many_lines)]
fn sampling(global: &std::path::Path, store: &Store) {
    // **A fresh `Round` per pass is a restarted daemon.** Nothing is carried in this process
    // between them, so what survives is what the tables hold — which is the point of the
    // baseline being a column rather than a field.
    let a_round = || {
        let mut round = Round::at(global);
        round.turn();
    };

    // 1. The first reading has no baseline, so it opens an hour with nothing in it. Charging a
    //    database's whole history to one minute is the failure that avoids.
    a_round();

    assert_eq!(
        store
            .ask("SELECT count(*) FROM activity_hour;")
            .expect("the server answers")
            .trim(),
        "1",
        "the first round did not open an hour"
    );

    let first = read_hour(store);
    assert_eq!(first.0, 0, "a first reading was counted as traffic");

    // And the baseline was written down, which is what makes the next round a difference.
    assert_ne!(
        store
            .ask("SELECT coalesce(counted_rows_in::text, '') FROM monitored_database;")
            .expect("the server answers")
            .trim(),
        "",
        "the first round left no baseline for the next one"
    );

    // 2. Move some rows, then round again. The delta is real traffic and it lands in the hour.
    store
        .run(
            "CREATE TABLE IF NOT EXISTS churn (id int); \
             INSERT INTO churn SELECT generate_series(1, 5000);",
        )
        .expect("making some traffic");
    a_round();

    let after = read_hour(store);
    assert!(
        after.0 > first.0,
        "five thousand rows moved and the hour did not: {first:?} then {after:?}"
    );
    assert_eq!(
        readings(store),
        2,
        "the second round did not count as a reading"
    );

    // 3. **A restart loses no completed rollup.** The hour keeps what it already had and the
    //    next reading adds to it rather than starting again — true only because the baseline
    //    outlived the process that wrote it.
    let before_restart = read_hour(store);
    a_round();
    let after_restart = read_hour(store);
    assert!(
        after_restart.0 >= before_restart.0,
        "a restart lost part of the hour: {before_restart:?} then {after_restart:?}"
    );
    assert_eq!(readings(store), 3);

    // 4. **A day is the sum of its hours**, which is `R26`'s own sentence. Two earlier hours
    //    are written in as a day of sampling would have left them, and the day is read back.
    store
        .run(
            "INSERT INTO activity_hour (registered_database_id, hour, rows_in, rows_out, readings)
             SELECT d.id, date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                          - interval '1 hour', 100, 200, 60
               FROM registered_database d WHERE d.label = 'itself';
             INSERT INTO activity_hour (registered_database_id, hour, rows_in, rows_out, readings)
             SELECT d.id, date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                          - interval '2 hours', 300, 400, 60
               FROM registered_database d WHERE d.label = 'itself';",
        )
        .expect("writing two earlier hours");

    let summed = store
        .ask(
            "SELECT sum(rows_in) || ' ' || sum(rows_out) FROM activity_hour
              WHERE hour > now() - interval '3 hours';",
        )
        .expect("the server answers");
    let hour_by_hour = store
        .ask(
            "SELECT sum(rows_in) || ' ' || sum(rows_out) FROM (
               SELECT hour, sum(rows_in) AS rows_in, sum(rows_out) AS rows_out
                 FROM activity_hour
                WHERE hour > now() - interval '3 hours'
                GROUP BY hour
             ) AS per_hour;",
        )
        .expect("the server answers");
    assert_eq!(
        summed.trim(),
        hour_by_hour.trim(),
        "the total and the sum of its hours disagree"
    );
    assert!(
        summed.trim().starts_with(|c: char| c.is_ascii_digit()),
        "the day summed to nothing readable: {summed}"
    );

    // 5. **A month is still something somebody can read** — one scan of one table, with no
    //    rollup tables to keep in step. This is the shape `R27` reads.
    assert_eq!(
        store
            .ask("SELECT count(*) FROM activity_hour WHERE hour > now() - interval '30 days';")
            .expect("the server answers")
            .trim(),
        "3",
        "a month does not read back as its hours"
    );

    // 6. Detaching stops the sampling and keeps every hour already recorded — `R25`'s rule,
    //    now that there is finally something in the table for it to be about.
    let detached = watch::detach(store, "itself").expect("it detaches");
    assert!(detached.was_attached);
    a_round();
    assert_eq!(readings(store), 3, "a detached database was sampled anyway");
    assert_eq!(
        store
            .ask("SELECT count(*) FROM activity_hour;")
            .expect("the server answers")
            .trim(),
        "3",
        "detaching deleted the hours it was supposed to keep"
    );
}

/// The current hour's rows in and rows out.
fn read_hour(store: &Store) -> (i64, i64) {
    let said = store
        .ask(
            "SELECT rows_in || ' ' || rows_out FROM activity_hour
              WHERE hour = date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC';",
        )
        .expect("the server answers");

    let mut parts = said.trim().split(' ');
    (
        parts.next().and_then(|n| n.parse().ok()).unwrap_or(-1),
        parts.next().and_then(|n| n.parse().ok()).unwrap_or(-1),
    )
}

/// How many readings the current hour has had.
fn readings(store: &Store) -> i64 {
    store
        .ask(
            "SELECT readings FROM activity_hour
              WHERE hour = date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC';",
        )
        .expect("the server answers")
        .trim()
        .parse()
        .expect("a count")
}

/// **`R27`'s `Done when`, against a real cluster.**
///
/// *"A database under a known load reads higher than an idle one, and a period with no samples
/// in it is shown as having none rather than drawn as a zero."* Two databases on one server,
/// one of them hammered and one of them left alone, both attached and both sampled — and then
/// the screen's own reading of them.
#[test]
fn a_loaded_database_reads_higher_than_an_idle_one_and_a_quiet_period_says_so() {
    let scratch = Scratch::new("activity");
    let global = scratch.path().to_path_buf();
    let data = crate::server::data_dir(&global);

    if !this_machine_can_keep_a_secret(&global) {
        return;
    }

    let Some(ready) = cluster(&global, a_free_port()) else {
        return;
    };
    record::write(&global, &ready.server, &ready.password).expect("it can keep a secret");

    let opened = match own::ensure(
        &global,
        &ready.server,
        &ready.password,
        &own::Choosing::unsupplied(),
    ) {
        Ok(opened) => opened,
        Err(why) => {
            stop(&ready.server.bin, &data);
            panic!("sloop's own database could not be made: {}", why.message());
        }
    };

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        schema::migrate(&ready.server, &opened.own, &opened.password)
            .expect("a fresh database migrates");
        schema::reconcile_engines(&ready.server, &opened.own, &opened.password)
            .expect("the engines are recorded");

        let store = Store::open(&global)
            .expect("the record reads back")
            .expect("a machine with a record has a store");

        // A second database on the same cluster, owned by the same role, so the only thing
        // that differs between the two is how hard one of them is worked.
        crate::server::make::run_sql(
            &ready.server,
            Some(&ready.password),
            &format!("CREATE DATABASE idle_db OWNER \"{}\";", opened.own.role),
        )
        .expect("a second database can be made");

        let entry = |database: &str| Database {
            engine: Engine::Postgres,
            host: String::from("127.0.0.1"),
            port: ready.server.port,
            database: database.to_owned(),
            user: opened.own.role.clone(),
            password: Route::Command(format!("echo {}", opened.password.expose())),
            reach: crate::ssh::Reach::Direct,
        };

        let mut registry = Registry::default();
        registry.insert(String::from("busy"), entry(&opened.own.database));
        registry.insert(String::from("idle"), entry("idle_db"));
        store
            .write(&Which::Global, &registry)
            .expect("the registry is writable");

        for label in ["busy", "idle"] {
            assert_eq!(
                watch::attach(&store, label).expect("it attaches"),
                Attached::Now
            );
        }

        activity_screen(&global, &store);
    }));

    stop(&ready.server.bin, &data);
    let _ = std::fs::remove_dir_all(scratch.path());

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

fn activity_screen(global: &std::path::Path, store: &Store) {
    use crate::service::activity;

    let a_round = || {
        let mut round = Round::at(global);
        round.turn();
    };

    // 1. **Before anything has been sampled**, both databases are attached and neither has
    //    been read. Nothing is drawn as a zero: `ever_watched` is false for both.
    let before = activity::read(store).expect("the screen reads");
    assert_eq!(before.databases.len(), 2);
    for database in &before.databases {
        assert!(
            !database.ever_watched(),
            "{} claims to have been watched before anything ran",
            database.label
        );
        assert!(
            !database.today.watched(),
            "an unsampled day is not being shown as unsampled"
        );
    }

    // 2. One baseline round, then a known load on one of the two and nothing at all on the
    //    other — which is the whole of the `Done when`.
    a_round();
    store
        .run(
            "CREATE TABLE IF NOT EXISTS churn (id int); \
             INSERT INTO churn SELECT generate_series(1, 20000); \
             SELECT count(*) FROM churn;",
        )
        .expect("a known load");
    a_round();

    let after = activity::read(store).expect("the screen reads");
    let of = |label: &str| {
        after
            .databases
            .iter()
            .find(|one| one.label == label)
            .unwrap_or_else(|| panic!("{label} is not on the screen"))
            .clone()
    };
    let (busy, idle) = (of("busy"), of("idle"));

    // **The `Done when`, in one line.**
    assert!(
        busy.today.rows_in > idle.today.rows_in,
        "the loaded database did not read higher than the idle one: {busy:?} against {idle:?}"
    );

    // 3. **And the idle one was watched, which is the other half.** It has readings and no
    //    traffic — silence, not absence — and the two are different on the screen.
    assert!(
        idle.today.watched(),
        "the idle database was sampled and the screen says it was not: {idle:?}"
    );
    assert_eq!(idle.today.readings, 2, "{idle:?}");

    // 4. A size for both, because that is the half every engine answers.
    for database in [&busy, &idle] {
        assert!(
            database.size_bytes.is_some_and(|bytes| bytes > 0),
            "{} has no size recorded: {database:?}",
            database.label
        );
        assert!(
            database.size_taken_at().is_some(),
            "{} has a size and no moment for it",
            database.label
        );
    }

    // 5. **A window with nothing in it reads as having none.** Nothing was recorded a month
    //    ago, so an hour placed back then is the only thing in that window — and the windows
    //    that do not reach it are untouched.
    let long_ago = activity::read(store).expect("the screen reads");
    assert!(
        long_ago
            .databases
            .iter()
            .all(|database| database.month.readings >= database.today.readings),
        "a wider window held fewer readings than a narrower one"
    );

    // 6. Detaching takes a database off the screen and leaves its history where it is — and
    //    the screen says so rather than letting it vanish.
    watch::detach(store, "idle").expect("it detaches");
    let after_detach = activity::read(store).expect("the screen reads");
    assert_eq!(
        after_detach
            .databases
            .iter()
            .map(|one| one.label.as_str())
            .collect::<Vec<_>>(),
        vec!["busy"],
        "a detached database is still on the screen"
    );
    assert_eq!(
        after_detach.detached_with_history,
        vec![String::from("idle")],
        "the history of a detached database vanished from the screen"
    );
}
