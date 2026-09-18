//! `sloop server connection` — opening sloop's own database yourself.
//!
//! **The half of `R19f` that has to be proved through the real binary.** What a password
//! reaches is a property of a *process*: which streams are terminals, what `--log-file`
//! ended up holding, what a `--json` document contains. None of that exists inside a unit
//! test, so every refusal here is exercised by running `sloop` and reading what came back.
//!
//! Every run is a pipe, because [`support::Run`] captures both streams — which is the case
//! the refusal exists for, and the reason `--force` appears in most of these.

mod support;

use support::Sandbox;

/// The password every sandbox's record points at. It is a fixture on a throwaway cluster,
/// written in `support::cluster`; the record itself holds `${SLOOP_TEST_DB_PW}` and never
/// this value, which is rule 3 holding inside the harness too.
const PASSWORD: &str = "sloopTestClusterPw0";

/// **The `Done when`: DataGrip from what sloop told them.** Host, port, database and user,
/// each on its own line, plus the URL for anything that takes one.
#[test]
fn it_says_everything_a_client_asks_for() {
    let sandbox = Sandbox::new("connection");
    if !sandbox.has_a_registry() {
        eprintln!("skipped: no PostgreSQL server on this machine");
        return;
    }

    let run = sandbox.sloop(&["server", "connection"]);
    run.expect_code(0);

    for detail in [
        "Host",
        "127.0.0.1",
        "Port",
        "Database",
        "User",
        "sloop_db_admin",
    ] {
        run.expect_said(detail);
    }
    run.expect_said("postgres://sloop_db_admin@127.0.0.1:");
    run.expect_said("--show-password");

    assert!(
        !run.said().contains(PASSWORD),
        "the password was printed by a command nobody asked it of:\n{}",
        run.said()
    );
}

/// Asked for plainly, and into a pipe: refused, and the refusal names what would answer it.
#[test]
fn the_password_is_refused_where_nobody_is_reading() {
    let sandbox = Sandbox::new("connection-pipe");
    if !sandbox.has_a_registry() {
        eprintln!("skipped: no PostgreSQL server on this machine");
        return;
    }

    let run = sandbox.sloop(&["server", "connection", "--show-password"]);
    run.expect_code(2);
    run.expect_said("--force");

    assert!(
        !run.said().contains(PASSWORD),
        "a refused run printed the password anyway:\n{}",
        run.said()
    );
}

/// `--force` is what says it in as many words, and then the password arrives — on standard
/// error, so a run whose standard output is being piped somewhere does not pipe it along.
#[test]
fn force_prints_it_on_standard_error() {
    let sandbox = Sandbox::new("connection-force");
    if !sandbox.has_a_registry() {
        eprintln!("skipped: no PostgreSQL server on this machine");
        return;
    }

    let run = sandbox.sloop(&["server", "connection", "--show-password", "--force"]);
    run.expect_code(0);

    assert!(
        run.stderr().contains(PASSWORD),
        "--force was given and the password never arrived:\n{}",
        run.stderr()
    );
    assert!(
        !run.stdout().contains(PASSWORD),
        "the password went to standard output, where a pipe would carry it:\n{}",
        run.stdout()
    );
}

/// **Never in `--json`, and never in `--quiet`** — both say this run is not being read by a
/// person, and a password is only ever printed for a person to read. Neither is overridable.
#[test]
fn a_run_that_is_not_being_read_never_gets_it() {
    let sandbox = Sandbox::new("connection-machine");
    if !sandbox.has_a_registry() {
        eprintln!("skipped: no PostgreSQL server on this machine");
        return;
    }

    for flags in [
        vec![
            "--json",
            "server",
            "connection",
            "--show-password",
            "--force",
        ],
        vec![
            "--quiet",
            "server",
            "connection",
            "--show-password",
            "--force",
        ],
    ] {
        let run = sandbox.sloop(&flags);
        run.expect_code(2);
        assert!(
            !run.said().contains(PASSWORD),
            "{flags:?} printed the password:\n{}",
            run.said()
        );
    }
}

/// The document a `--json` run prints carries the *route*, which is a word like `keyring`.
#[test]
fn the_document_says_where_the_password_is_and_never_what_it_is() {
    let sandbox = Sandbox::new("connection-json");
    if !sandbox.has_a_registry() {
        eprintln!("skipped: no PostgreSQL server on this machine");
        return;
    }

    let run = sandbox.sloop(&["--json", "server", "connection"]);
    run.expect_code(0);

    let document: serde_json::Value =
        serde_json::from_str(&run.stdout()).expect("a --json run prints one document");
    let result = &document["result"];

    assert_eq!(result["host"], "127.0.0.1");
    assert_eq!(result["role"], "sloop_db_admin");
    assert_eq!(result["password_route"], "${SLOOP_TEST_DB_PW}");
    assert!(
        !run.stdout().contains(PASSWORD),
        "the document carried the password:\n{}",
        run.stdout()
    );
}

/// **Rule 3, at the one place `R19f` could have broken it.** A sloop log is meant to be
/// pasteable into a public issue, so the line carrying a password is the one line in this
/// program with no logging path behind it.
#[test]
fn the_password_never_reaches_a_log_file() {
    let sandbox = Sandbox::new("connection-log");
    if !sandbox.has_a_registry() {
        eprintln!("skipped: no PostgreSQL server on this machine");
        return;
    }

    let log = sandbox.work().join("run.log");
    let run = sandbox.sloop(&[
        "--log-file",
        &log.display().to_string(),
        "server",
        "connection",
        "--show-password",
        "--force",
    ]);
    run.expect_code(0);

    let written = std::fs::read_to_string(&log).expect("the log was opened and written");
    assert!(
        written.contains("sloop_db_admin"),
        "the log did not get the run at all, so it proves nothing:\n{written}"
    );
    assert!(
        !written.contains(PASSWORD),
        "the password was written to --log-file:\n{written}"
    );
}

/// A machine that has not been set up is told what to run, rather than crashing.
#[test]
fn a_machine_with_no_record_is_told_to_set_up() {
    let sandbox = Sandbox::new("connection-unset");
    let record = sandbox.global_dir().join("server.toml");
    let _ = std::fs::remove_file(&record);

    let run = sandbox.sloop(&["server", "connection"]);
    run.expect_code(2);
    run.expect_said("sloop setup");
}
