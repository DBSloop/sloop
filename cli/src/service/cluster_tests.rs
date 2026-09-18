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
