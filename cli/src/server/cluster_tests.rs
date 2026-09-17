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
use super::{Origin, make, record};

/// A port per test, and none of them 5433: a developer running these should never collide
/// with the cluster their own `sloop setup` made, and cargo runs these three at once.
const MADE_PORT: u16 = 55987;
const AUTH_PORT: u16 = 55988;
const AGAIN_PORT: u16 = 55989;

/// The variable that says which PostgreSQL to build the cluster out of, shared with the
/// engine's own cluster tests so one machine sets one variable.
const BIN_DIR_VAR: &str = "SLOOP_TEST_PG_BIN";

fn exe(name: &str) -> String {
    format!("{name}{}", if cfg!(windows) { ".exe" } else { "" })
}

/// Where the server programs are, or `None` if this machine has none.
///
/// `SLOOP_TEST_PG_BIN` first, and when it is set and wrong this **panics** rather than
/// skipping: a test that quietly skips because a variable was mistyped is a green tick that
/// proved nothing.
fn binaries() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os(BIN_DIR_VAR) {
        let directory = PathBuf::from(named);
        assert!(
            directory.join(exe("initdb")).is_file(),
            "{BIN_DIR_VAR} is set to {} but there is no {} in it",
            directory.display(),
            exe("initdb")
        );
        return Some(directory);
    }

    // `PATH`, then the same install directories `tools` knows about — `initdb` ships in the
    // server package and distributions leave it off `PATH`.
    let mut looked: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    looked.extend(crate::tools::installed_directories(
        crate::engine::Engine::Postgres,
    ));

    let found = looked.into_iter().find(|directory| {
        directory.join(exe("initdb")).is_file() && directory.join(exe("pg_ctl")).is_file()
    });

    if found.is_none() {
        eprintln!("skipping the sloop-cluster tests: this machine has no initdb");
    }
    found
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
    let Some(bin) = binaries() else { return };

    let scratch = Scratch::new("cluster");
    let global = scratch.path().to_path_buf();
    let data = super::data_dir(&global);

    let built = make::with_binaries(&global, bin.clone(), MADE_PORT);
    let ready = match built {
        Ok(ready) => ready,
        Err(why) => {
            stop(&bin, &data);
            panic!("the cluster could not be built: {}", why.message());
        }
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
    let Some(bin) = binaries() else { return };

    let scratch = Scratch::new("auth");
    let global = scratch.path().to_path_buf();
    let data = super::data_dir(&global);

    let built = make::with_binaries(&global, bin.clone(), AUTH_PORT);
    let ready = match built {
        Ok(ready) => ready,
        Err(why) => {
            stop(&bin, &data);
            panic!("the cluster could not be built: {}", why.message());
        }
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
    let Some(bin) = binaries() else { return };

    let scratch = Scratch::new("again");
    let global = scratch.path().to_path_buf();
    let data = super::data_dir(&global);

    let built = make::with_binaries(&global, bin.clone(), AGAIN_PORT);
    let ready = match built {
        Ok(ready) => ready,
        Err(why) => {
            stop(&bin, &data);
            panic!("the cluster could not be built: {}", why.message());
        }
    };
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
