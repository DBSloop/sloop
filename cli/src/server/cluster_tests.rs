//! Building a real cluster, with a real `initdb`, and proving what came out of it.
//!
//! **Never the machine's own PostgreSQL.** Everything here happens inside a temporary global
//! store that is deleted afterwards, on a port nothing else uses, and the only thing it
//! touches outside that directory is whichever secret store the machine keeps passwords in —
//! under a key naming the port, which is why the port is not 5433.
//!
//! **Whatever PostgreSQL is on this machine, not necessarily 18.** What is being tested is
//! the sequence — initialise with `trust`, start on loopback, set the password over a pipe,
//! switch to `scram-sha-256`, prove the password opens it and nothing else does — and that
//! sequence is the same on every major. Which major sloop *wants* is `WANTED_MAJOR`, and
//! `find`'s tests cover that separately.
//!
//! Skipped with a printed reason where there is no `initdb`, which is the honest answer on a
//! machine that cannot host a database at all.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::tests::Scratch;
use super::{Origin, make, own, record, schema};

/// A port per test, and none of them 5433: a developer running these should never collide
/// with the cluster their own `sloop setup` made, and cargo runs these three at once.
const MADE_PORT: u16 = 55987;
const AUTH_PORT: u16 = 55988;
const AGAIN_PORT: u16 = 55989;
const OWN_PORT: u16 = 55990;
const SCHEMA_PORT: u16 = 55991;

/// The variable that says which PostgreSQL to build the cluster out of, shared with the
/// engine's own cluster tests so one machine sets one variable.
const BIN_DIR_VAR: &str = "SLOOP_TEST_PG_BIN";

fn exe(name: &str) -> String {
    format!("{name}{}", if cfg!(windows) { ".exe" } else { "" })
}

/// Why these tests have a server to build out of at all.
///
/// The distinction decides what a failure means, and it is `engine::cluster_tests`' rule
/// rather than a new one: asked for a PostgreSQL by name and it will not run, that is a red
/// test, because somebody wanted that one exercised. Found one lying around and it will not
/// run, that is a skip — nobody promised this machine could host a database, and a runner
/// that ships a client without a server is exactly such a machine.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// `SLOOP_TEST_PG_BIN` named it.
    Explicitly,
    /// It turned up on `PATH` or in a package manager's directory.
    ByLookingAround,
}

/// Where the server programs are, or `None` if this machine has none.
///
/// `SLOOP_TEST_PG_BIN` first, and when it is set and wrong this **panics** rather than
/// skipping: a test that quietly skips because a variable was mistyped is a green tick that
/// proved nothing.
fn binaries() -> Option<(PathBuf, Asked)> {
    if let Some(named) = std::env::var_os(BIN_DIR_VAR) {
        let directory = PathBuf::from(named);
        assert!(
            directory.join(exe("initdb")).is_file(),
            "{BIN_DIR_VAR} is set to {} but there is no {} in it",
            directory.display(),
            exe("initdb")
        );
        return Some((directory, Asked::Explicitly));
    }

    // `PATH`, then the same install directories `tools` knows about — the server programs
    // ship in the server package and distributions leave them off `PATH`.
    let mut looked: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    looked.extend(crate::tools::installed_directories(
        crate::engine::Engine::Postgres,
    ));

    // **All three, which is what `find::version_of` means by a server.** Checking only
    // `initdb` and `pg_ctl` was a bug with a shape: a runner given `postgresql-client` has
    // some of them and no `postgres` at all, so these tests started building a cluster on a
    // machine that could not run one, and failed where they should never have begun.
    let found = looked.into_iter().find(|directory| {
        ["initdb", "pg_ctl", "postgres"]
            .iter()
            .all(|name| directory.join(exe(name)).is_file())
    });

    if found.is_none() {
        eprintln!("skipping the sloop-cluster tests: this machine has no PostgreSQL server");
    }
    found.map(|directory| (directory, Asked::ByLookingAround))
}

/// Build the cluster, or say why these tests are not going to run on this machine.
///
/// The one place the rule above is applied, so no test in this file can forget it.
fn cluster(global: &Path, port: u16) -> Option<super::Ready> {
    let (bin, asked) = binaries()?;
    let data = super::data_dir(global);

    match make::with_binaries(global, bin.clone(), port) {
        Ok(ready) => Some(ready),
        Err(why) => {
            stop(&bin, &data);
            assert!(
                asked == Asked::ByLookingAround,
                "{BIN_DIR_VAR} named a PostgreSQL that cannot build a cluster: {}
{}",
                why.message(),
                // The hint carries the end of the server log, which is the only thing that
                // says *why* — and on a CI runner the directory is gone before anybody looks.
                why.hint_text().unwrap_or("no hint")
            );
            eprintln!(
                "skipping the sloop-cluster tests: this machine cannot host a database: {}",
                why.message()
            );
            None
        }
    }
}

/// Stop a cluster these tests started, so the temporary directory can actually be removed.
fn stop(bin: &Path, data: &Path) {
    let _ = Command::new(bin.join(exe("pg_ctl")))
        .arg("-D")
        .arg(data)
        .args(["-m", "immediate", "-w", "stop"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Run `psql` against the cluster with whatever password is given, and say what happened.
fn connect_with(bin: &Path, port: u16, password: Option<&str>, sql: &str) -> (bool, String) {
    let mut command = Command::new(bin.join(exe("psql")));
    command
        .args(["-h", super::LOOPBACK])
        .args(["-p", &port.to_string()])
        .args(["-U", super::SUPERUSER])
        .args(["-d", "postgres"])
        .args(["-v", "ON_ERROR_STOP=1"])
        .arg("--no-psqlrc")
        // The same reason the real one passes it: without this, the case below where no
        // password is given at all makes `psql` open the console and wait forever.
        .arg("--no-password")
        .args(["--tuples-only", "--no-align"])
        .args(["-c", sql])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match password {
        Some(value) => command.env("PGPASSWORD", value),
        None => command.env_remove("PGPASSWORD"),
    };

    let output = command.output().expect("psql should run");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// **The whole of `R19c1`'s making half, end to end.**
///
/// One test rather than five, because every step needs the cluster the step before it built
/// and five tests would mean five `initdb`s of the same thing.
#[test]
fn a_cluster_is_made_started_and_closed_behind_a_password() {
    let scratch = Scratch::new("cluster");
    let global = scratch.path().to_path_buf();
    let data = super::data_dir(&global);

    let Some(ready) = cluster(&global, MADE_PORT) else {
        return;
    };

    // Written here because `ensure` is what writes it in a real run, and assertion 3 below
    // has to read a file that actually exists. Skipped where the machine has nowhere to keep
    // a password — see `tests::this_machine_can_keep_a_secret`.
    if super::tests::this_machine_can_keep_a_secret(&global) {
        record::write(&global, &ready.server, &ready.password).expect("it just said it could");
    }

    let password = ready.password.expose().to_owned();
    let bin_used = ready.server.bin.clone();

    // 1. It is sloop's own cluster, under the global store, and nowhere near the machine's.
    assert_eq!(ready.server.origin, Origin::Sloops);
    assert_eq!(ready.server.data.as_deref(), Some(data.as_path()));
    assert!(ready.made_now, "this run is what made it");
    assert!(
        data.join("PG_VERSION").is_file(),
        "no cluster at {}",
        data.display()
    );
    assert!(
        data.starts_with(scratch.path()),
        "the cluster escaped the sandbox: {}",
        data.display()
    );

    // 2. The password is the generated alphabet, so nothing it is pasted into needs escaping.
    assert_eq!(password.chars().count(), 28);
    assert!(password.chars().all(|c| c.is_ascii_alphanumeric()));

    // 3. Nothing about it reached the disk in plaintext — not the record, not `pg_hba.conf`,
    //    not the log the server writes.
    for file in [
        global.join(record::FILE),
        data.join("pg_hba.conf"),
        data.join("server.log"),
        data.join("postgresql.conf"),
    ] {
        if let Ok(text) = std::fs::read_to_string(&file) {
            assert!(
                !text.contains(&password),
                "the password is in {}",
                file.display()
            );
        }
    }

    // 4. `trust` is gone from pg_hba.conf, which is what makes step 5 mean anything.
    let hba = std::fs::read_to_string(data.join("pg_hba.conf")).expect("pg_hba.conf is there");
    for line in hba.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        assert!(
            !line.ends_with("trust"),
            "a trust rule survived hardening: {line}"
        );
    }

    stop(&bin_used, &data);
    let _ = std::fs::remove_dir_all(scratch.path());
}

/// The same sequence, checked from the outside: the password opens it and nothing else does.
///
/// Its own test because it needs the cluster left *running*, and the one above tears its
/// cluster down as the last thing it does.
#[test]
fn the_password_opens_it_and_an_empty_one_does_not() {
    let scratch = Scratch::new("auth");
    let global = scratch.path().to_path_buf();
    let data = super::data_dir(&global);

    let Some(ready) = cluster(&global, AUTH_PORT) else {
        return;
    };

    let password = ready.password.expose().to_owned();
    let at = |secret: Option<&str>| connect_with(&ready.server.bin, AUTH_PORT, secret, "SELECT 1;");

    let (ok, said) = at(Some(&password));
    assert!(ok, "the generated password should open it: {said}");
    assert_eq!(said.trim(), "1");

    let (ok, said) = at(Some("not the password"));
    assert!(!ok, "a wrong password opened it: {said}");
    assert!(
        said.contains("password authentication failed"),
        "it refused for the wrong reason: {said}"
    );

    let (ok, said) = at(None);
    assert!(!ok, "no password at all opened it: {said}");

    stop(&ready.server.bin, &data);
    let _ = std::fs::remove_dir_all(scratch.path());
}

/// Setup has to be re-runnable, and a cluster that is already there is never initialised
/// over: that directory is a database.
#[test]
fn a_cluster_that_is_already_there_is_never_initialised_over() {
    let scratch = Scratch::new("again");
    let global = scratch.path().to_path_buf();
    let data = super::data_dir(&global);

    let Some(ready) = cluster(&global, AGAIN_PORT) else {
        return;
    };
    let bin = ready.server.bin.clone();
    stop(&ready.server.bin, &data);

    let before = std::fs::read_to_string(data.join("PG_VERSION")).expect("PG_VERSION is there");

    // A second attempt with no record beside it — `server.toml` deleted, or a Setup that
    // stopped halfway. It is refused, and the cluster is exactly as it was.
    let failure = make::with_binaries(&global, bin, AGAIN_PORT)
        .expect_err("an existing cluster with no record is not something to overwrite");
    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure.message().contains("already a PostgreSQL cluster"),
        "{}",
        failure.message()
    );
    assert_eq!(
        std::fs::read_to_string(data.join("PG_VERSION")).expect("still there"),
        before
    );

    let _ = std::fs::remove_dir_all(scratch.path());
}

// ---------------------------------------------------------------------------------------
// R19c2 — the database, the role, and where its password lives
// ---------------------------------------------------------------------------------------

/// **The whole `Done when`: sloop reconnects on a second run with nothing typed, and neither
/// password is in any file in plaintext.**
///
/// One test, because the second half is only meaningful against the state the first half
/// left — and a second `initdb` of the same cluster would be testing a different machine.
#[test]
fn sloops_own_database_is_made_once_and_reopened_with_nothing_typed() {
    let scratch = Scratch::new("own");
    let global = scratch.path().to_path_buf();
    let data = super::data_dir(&global);

    if !super::tests::this_machine_can_keep_a_secret(&global) {
        return;
    }

    let Some(ready) = cluster(&global, OWN_PORT) else {
        return;
    };
    record::write(&global, &ready.server, &ready.password).expect("it can keep a secret");

    // 1. The first run makes the role and the database, and the role's password opens it.
    let (first, password) = match own::ensure(
        &global,
        &ready.server,
        &ready.password,
        &own::Choosing::unsupplied(),
    ) {
        Ok(made) => (made.own, made.password),
        Err(why) => {
            stop(&ready.server.bin, &data);
            panic!("sloop's own database could not be made: {}", why.message());
        }
    };

    assert_eq!(first.database, own::DATABASE);
    assert_eq!(first.role, own::ROLE);
    assert_eq!(password.expose().chars().count(), 28);
    assert!(password.expose().chars().all(|c| c.is_ascii_alphanumeric()));

    // 2. It is a database, owned by that role, reached as that role — not as the superuser.
    let opened = own::opens(&ready.server, &first, &password);
    assert!(
        opened.as_ref().is_ok_and(|yes| *yes),
        "the owning role could not open it: {opened:?}"
    );

    // 3. **Neither password is in any file in plaintext.** Every file sloop wrote, checked
    //    against both — the superuser's and the role's.
    let secrets = [
        ready.password.expose().to_owned(),
        password.expose().to_owned(),
    ];
    for file in [
        global.join(record::FILE),
        global.join(crate::registry::file::SEALED_FILE),
        data.join("pg_hba.conf"),
        data.join("postgresql.conf"),
        data.join("server.log"),
    ] {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for secret in &secrets {
            assert!(
                !text.contains(secret.as_str()),
                "a password is in {}",
                file.display()
            );
        }
    }

    // And what the record *does* hold for it is a route, which is the type that cannot be a
    // password — `Route::parse` refuses anything that is not one of the four.
    let route = record::database_route(&global)
        .expect("the record reads back")
        .expect("a route was written");
    assert!(matches!(
        route,
        crate::secret::Route::Keyring | crate::secret::Route::EncryptedFile
    ));

    // 4. **A second run, with nothing typed.** Same role, same database, same password —
    //    read back from wherever this machine keeps secrets rather than made again.
    let second = own::ensure(
        &global,
        &ready.server,
        &ready.password,
        &own::Choosing::unsupplied(),
    )
    .expect("a second run is ordinary");
    let (again, same) = (second.own, second.password);

    assert_eq!(again, first);
    assert_eq!(
        same.expose(),
        password.expose(),
        "the second run reset a password it should have read back"
    );

    // 5. And nothing was made twice: one database of that name, one role.
    let databases = make::query(
        &ready.server,
        Some(&ready.password),
        &format!(
            "SELECT count(*) FROM pg_database WHERE datname = '{}';",
            own::DATABASE
        ),
    )
    .expect("the server answers");
    assert_eq!(databases.trim(), "1");

    // 6. **`R19f`**: and then one the owner supplied, on the same cluster.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        a_supplied_password_outranks_the_one_already_there(&global, &data, &ready);
    }));

    stop(&ready.server.bin, &data);
    let _ = std::fs::remove_dir_all(scratch.path());

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// **`R19f`: a machine set up with a supplied password never generates one.**
///
/// The flag outranks the password already on the role, because that is how one gets rotated
/// — and the value here is full of the punctuation a real password has, which is what proves
/// `make::literal` escapes rather than refuses.
fn a_supplied_password_outranks_the_one_already_there(
    global: &std::path::Path,
    data: &std::path::Path,
    ready: &super::Ready,
) {
    // The other half of `R19f`'s `Done when`: the cluster is exactly as hardened afterwards
    // as it was before. `trust` being gone is checked where it is removed; what is checked
    // here is that nothing on this path puts it back.
    let hba = data.join("pg_hba.conf");
    let hardened = std::fs::read_to_string(&hba).expect("pg_hba.conf is there");

    // **Shell-safe, because the route is a shell command and the shell is not what is being
    // tested here.** `--db-password-command` is handed to `cmd /c` on Windows and `sh -c`
    // everywhere else, and the two disagree about quoting long before any of this reaches
    // PostgreSQL — a password full of punctuation delivered this way tests `sh`. What a
    // punctuation-heavy one does to the *statement* is the check below, which needs no shell.
    let chosen = "Supplied0Password9";
    let supplied = own::ensure(
        global,
        &ready.server,
        &ready.password,
        &own::Choosing {
            command: Some(&format!("echo {chosen}")),
            stdin: false,
        },
    )
    .expect("a supplied password is taken");

    assert!(
        !supplied.generated,
        "a supplied password was reported as one sloop invented"
    );
    assert_eq!(supplied.password.expose(), chosen);

    let opened = own::opens(&ready.server, &supplied.own, &supplied.password);
    assert!(
        opened.as_ref().is_ok_and(|yes| *yes),
        "the supplied password does not open the database: {opened:?}"
    );

    // And it is what a later run reads back — the record now points at the supplied one.
    let kept = record::database(global)
        .expect("the record reads back")
        .expect("a machine that has been set up has one");
    assert_eq!(kept.1.expose(), chosen);

    // Still nowhere in plaintext, supplied or not.
    for file in [
        global.join(record::FILE),
        global.join(crate::registry::file::SEALED_FILE),
    ] {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        assert!(
            !text.contains(chosen),
            "a supplied password is in {}",
            file.display()
        );
    }

    // **And a password full of the punctuation a real one has, against the real server.**
    // Set through the same statement Setup sets one with, so what is proved is that
    // `make::literal` escapes rather than refuses — and that what PostgreSQL stored is what
    // was handed to it, character for character. No shell anywhere in this half.
    let awkward =
        crate::secret::Secret::new("o'brien';DROP TABLE x;-- \\ $PATH \"q\" #1".to_owned());
    own::set_password(
        &ready.server,
        &ready.password,
        &supplied.own,
        &awkward,
        true,
    )
    .expect("a password with a quote in it is escaped into the statement");

    let opened = own::opens(&ready.server, &supplied.own, &awkward);
    assert!(
        opened.as_ref().is_ok_and(|yes| *yes),
        "a password full of punctuation did not survive the statement: {opened:?}"
    );

    assert_eq!(
        std::fs::read_to_string(&hba).expect("pg_hba.conf is still there"),
        hardened,
        "settling a password changed how the cluster authenticates"
    );
}

// ---------------------------------------------------------------------------------------
// R19c3 — the schema
// ---------------------------------------------------------------------------------------

/// The fingerprint of a database's whole shape: every table, every column, every type.
///
/// **Two databases that print the same string have the same schema**, which is how the
/// "a release older" half of the `Done when` is checked — one migrated from nothing and one
/// migrated forward from version 3 have to end up indistinguishable, not merely both
/// working.
fn shape_of(server: &super::Server, own: &own::Own, password: &crate::secret::Secret) -> String {
    make::ask(
        server,
        &own.as_who(),
        Some(password),
        "SELECT table_name || '.' || column_name || ' ' || data_type
           FROM information_schema.columns
          WHERE table_schema = 'public'
          ORDER BY table_name, column_name;",
    )
    .expect("the migrated database answers")
}

/// Every non-empty line of an answer, trimmed.
fn rows(said: &str) -> Vec<&str> {
    said.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

/// **The whole `Done when`: migrations that run forwards on an empty database and on one a
/// release older, and every table reachable from a documented query.**
///
/// One test and one cluster, for the reason the `R19c2` one above gives: every step needs the
/// state the step before it left, and a second `initdb` would be testing a different machine.
#[test]
fn the_schema_migrates_forwards_from_nothing_and_from_a_release_older() {
    let scratch = Scratch::new("schema");
    let global = scratch.path().to_path_buf();
    let data = super::data_dir(&global);

    if !super::tests::this_machine_can_keep_a_secret(&global) {
        return;
    }

    let Some(ready) = cluster(&global, SCHEMA_PORT) else {
        return;
    };
    record::write(&global, &ready.server, &ready.password).expect("it can keep a secret");

    let (own, password) = match own::ensure(
        &global,
        &ready.server,
        &ready.password,
        &own::Choosing::unsupplied(),
    ) {
        Ok(made) => (made.own, made.password),
        Err(why) => {
            stop(&ready.server.bin, &data);
            panic!("sloop's own database could not be made: {}", why.message());
        }
    };

    // Everything from here has a running cluster to tear down, and a panic in the middle of
    // it would leave a postmaster holding the scratch directory open — on Windows that is a
    // directory nothing can delete. Run the assertions, stop the server, then re-raise.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assertions(&ready.server, &own, &password, &ready.password);
    }));

    stop(&ready.server.bin, &data);
    let _ = std::fs::remove_dir_all(scratch.path());

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// The assertions themselves, with the cluster running.
///
/// `password` opens sloop's own database as the role that owns it, which is how everything
/// here reads and writes. `superuser` is needed for exactly one statement — `CREATE DATABASE`
/// for the "a release older" copy — because making a database is not the owning role's to do.
#[allow(clippy::too_many_lines)]
fn assertions(
    server: &super::Server,
    own: &own::Own,
    password: &crate::secret::Secret,
    superuser: &crate::secret::Secret,
) {
    // 1. **From nothing.** An empty database gets every migration, in order, and lands on the
    //    newest version this build knows.
    // Driven from `MIGRATIONS` rather than from a number typed here, so the next migration
    // is a file and a `READINGS` entry rather than a hunt through this test for fours.
    let newest = i64::try_from(schema::MIGRATIONS.len()).expect("this many migrations fit");
    let every_name: Vec<&str> = schema::MIGRATIONS.iter().map(|one| one.name).collect();

    let applied = schema::migrate(server, own, password).expect("an empty database migrates");
    assert_eq!(applied.from, 0, "it was not empty to begin with");
    assert_eq!(applied.to, newest);
    assert_eq!(applied.ran, every_name);
    assert!(applied.changed_anything());

    // 2. Ten tables, and they are exactly the ten the module documents.
    let said = make::ask(
        server,
        &own.as_who(),
        Some(password),
        "SELECT table_name FROM information_schema.tables
          WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
          ORDER BY table_name;",
    )
    .expect("the migrated database answers");
    let mut found = rows(&said);
    found.sort_unstable();

    let mut documented: Vec<&str> = schema::READINGS
        .iter()
        .map(|reading| reading.table)
        .collect();
    documented.sort_unstable();
    assert_eq!(
        found, documented,
        "the database and the documentation disagree"
    );

    // 3. **Every `id` is a `BIGSERIAL`** — the owner's first sentence about this schema. In
    //    the catalogue that is a `bigint` with a sequence behind it, on all ten tables.
    let said = make::ask(
        server,
        &own.as_who(),
        Some(password),
        "SELECT table_name || ' ' || data_type || ' ' || coalesce(column_default, 'none')
           FROM information_schema.columns
          WHERE table_schema = 'public' AND column_name = 'id'
          ORDER BY table_name;",
    )
    .expect("the migrated database answers");
    let id_columns = rows(&said);
    assert_eq!(
        id_columns.len(),
        schema::READINGS.len(),
        "a table has no id: {id_columns:?}"
    );
    for column in &id_columns {
        assert!(column.contains("bigint"), "{column} is not a BIGSERIAL");
        assert!(
            column.contains("nextval"),
            "{column} has no sequence behind it"
        );
    }

    // 4. **Every table is reachable from a documented query.** Each one runs against the real
    //    thing, so a renamed column fails here rather than sitting in a comment being wrong.
    for reading in schema::READINGS {
        make::ask(server, &own.as_who(), Some(password), reading.sql).unwrap_or_else(|why| {
            panic!(
                "the documented query for {} ({}) does not run: {}",
                reading.table,
                reading.purpose,
                why.message()
            )
        });
    }

    // 5. The engines this build speaks are in the table, under their proper names — and
    //    reconciling twice does not double them, because Setup is re-runnable.
    schema::reconcile_engines(server, own, password).expect("the engines go in");
    schema::reconcile_engines(server, own, password).expect("a second run is ordinary");
    let said = make::ask(
        server,
        &own.as_who(),
        Some(password),
        "SELECT name || ' ' || display_name || ' ' || default_port
           FROM engine WHERE supported ORDER BY name;",
    )
    .expect("the engine table answers");
    assert_eq!(
        rows(&said),
        [
            "mariadb MariaDB 3306",
            "mysql MySQL 3306",
            "postgres PostgreSQL 5432"
        ]
    );

    // 6. **A second run applies nothing**, which is what makes Setup safe to run again.
    let again = schema::migrate(server, own, password).expect("a second run is ordinary");
    assert_eq!(again.from, newest);
    assert_eq!(again.to, newest);
    assert!(again.ran.is_empty(), "it ran {:?} a second time", again.ran);
    assert!(!again.changed_anything());

    let fresh_shape = shape_of(server, own, password);

    // 7. **A database a release older.** This release is the only one that exists, so one is
    //    built: the same runner through the first three migrations, which is exactly what a
    //    machine that had them and not the fourth looks like.
    let older = own::Own {
        database: "sloop_database_older".to_owned(),
        role: own.role.clone(),
    };
    make::run_sql(
        server,
        Some(superuser),
        &format!(
            "CREATE DATABASE \"{}\" OWNER \"{}\";",
            older.database, older.role
        ),
    )
    .expect("a second database can be made on sloop's own cluster");

    let one_short = schema::MIGRATIONS.len() - 1;
    let behind =
        schema::migrate_through(server, &older, password, &schema::MIGRATIONS[..one_short])
            .expect("every migration but the last applies");
    assert_eq!(behind.from, 0);
    assert_eq!(behind.to, newest - 1);
    assert_eq!(behind.ran, every_name[..one_short]);

    // It really is a release behind: what the last migration adds is not there yet.
    //
    // **A column, not a table, since `0006`.** This check moves with whatever the newest
    // migration happens to do, and pinning it to a table name was what made it need
    // changing — the point it makes is "the last one has not run here", and the thing to
    // look for is whatever that one puts in.
    let said = make::ask(
        server,
        &older.as_who(),
        Some(password),
        "SELECT count(*) FROM information_schema.columns
          WHERE table_schema = 'public'
            AND table_name = 'registered_database'
            AND column_name = 'ssh_host';",
    )
    .expect("the older database answers");
    assert_eq!(
        said.trim(),
        "0",
        "what the last migration adds is already there"
    );

    // 8. And the real `migrate` carries it forward — the one it is missing, not all four.
    let caught_up = schema::migrate(server, &older, password).expect("it migrates forwards");
    assert_eq!(caught_up.from, newest - 1);
    assert_eq!(caught_up.to, newest);
    assert_eq!(caught_up.ran, [every_name[one_short]]);

    // 9. **Ending indistinguishable from one migrated from nothing.** Not "both work" —
    //    identical, table for table and column for column.
    assert_eq!(
        shape_of(server, &older, password),
        fresh_shape,
        "a database brought forward is not the same shape as one built from scratch"
    );

    // 10. A migration edited after it shipped is refused by name, rather than being run again
    //     over a schema that already has it.
    make::script(
        server,
        &older.as_who(),
        Some(password),
        &format!(
            "UPDATE schema_migration SET checksum = '{}' WHERE version = 2;",
            "0".repeat(64)
        ),
    )
    .expect("the ledger can be written to");

    let refused = schema::migrate(server, &older, password)
        .expect_err("a migration that has changed since it was applied is not run over");
    assert_eq!(refused.exit().code(), 1);
    assert!(
        refused
            .message()
            .contains("migration 2 (registry) has changed"),
        "{}",
        refused.message()
    );

    // 11. And a database written by a newer sloop is never migrated by an older one.
    make::script(
        server,
        &older.as_who(),
        Some(password),
        &format!(
            "UPDATE schema_migration SET checksum = '{}' WHERE version = 2;
             INSERT INTO schema_migration (version, name, checksum, applied_by)
                  VALUES (99, 'from the future', '{}', '9.9.9');",
            schema::MIGRATIONS[1].checksum(),
            "1".repeat(64)
        ),
    )
    .expect("the ledger can be written to");

    let refused = schema::migrate(server, &older, password)
        .expect_err("an older sloop never migrates a newer schema");
    assert_eq!(refused.exit().code(), 2);
    assert!(
        refused.message().contains("schema version 99"),
        "{}",
        refused.message()
    );
}
