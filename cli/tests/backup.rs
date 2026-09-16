//! `sloop backup`, through the real binary, in a sandbox of its own.
//!
//! **No server and no keyring**, for the reasons in `tests/db.rs`: both would make these
//! tests depend on the machine, and the keyring is real and shared. So every registration
//! here uses `--env`, and what is checked is the part that is entirely sloop's — which
//! database it refuses to back up and why, what `--all` does when things fail, and the
//! exit code a scheduler reads afterwards.
//!
//! `127.0.0.1:1` is the unreachable server throughout. Nothing listens on port 1, so any
//! command that gets as far as opening a connection fails there and nowhere earlier.
//!
//! A backup that actually lands is in `commands::backup::cluster_tests`, against a real
//! PostgreSQL built for the test.

mod support;

use support::Sandbox;

/// Register one that can be resolved but never reached.
fn registered(sandbox: &Sandbox, name: &str, url: &str) {
    sandbox
        .sloop(&["db", "add", name, "--url", url, "--env", "PW"])
        .expect_code(0);
}

/// Nothing anywhere under the store, which is how "it wrote nothing" is checked.
fn backups_dir(sandbox: &Sandbox) -> std::path::PathBuf {
    sandbox.global_dir().join("backups")
}

/// Take the backup key out once.
///
/// **The first encrypted backup refuses until this has happened**, which is R11 working
/// rather than something in the way: a key that exists only on the machine being backed up
/// is a key that dies with it. Every test below that is about something else gets it out of
/// the way first, exactly as a person would.
fn keyed(sandbox: &Sandbox) {
    sandbox.sloop(&["key", "export"]).expect_code(0);
}

/// Rule 4 has nothing to catch here — `backup` asks no questions — but a command with no
/// database named still has to say which of the two things it wanted.
#[test]
fn backup_with_nothing_named_says_what_it_needed() {
    let sandbox = Sandbox::new("backup-none");

    sandbox
        .sloop(&["backup"])
        .expect_code(2)
        .expect_said("needs a database")
        .expect_said("--all");

    // And with a registry that has something in it, the answer is the same: this is about
    // the command line, not about what is registered.
    registered(&sandbox, "orders", "postgres://app@127.0.0.1:1/orders");
    sandbox.sloop(&["backup"]).expect_code(2);

    assert!(!backups_dir(&sandbox).exists(), "it wrote something");
}

#[test]
fn an_unknown_name_is_usage_and_nothing_is_written() {
    let sandbox = Sandbox::new("backup-unknown");
    registered(&sandbox, "orders", "postgres://app@127.0.0.1:1/orders");

    sandbox
        .sloop(&["backup", "nope"])
        .expect_code(2)
        .expect_said("no database is registered as nope");

    assert!(!backups_dir(&sandbox).exists(), "it wrote something");
}

/// `--all` over an empty registry is not a quiet success. A scheduled run that believes it
/// is backing up seven databases and is actually backing up none has to be told.
#[test]
fn all_with_nothing_registered_is_a_failure_that_names_the_registry() {
    let sandbox = Sandbox::new("backup-empty");

    sandbox
        .sloop(&["backup", "--all"])
        .expect_code(2)
        .expect_said("nothing is registered")
        .expect_said("--all has nothing to back up");
}

/// A server that cannot be reached is a connection failure, and nothing is left behind.
#[test]
fn an_unreachable_server_is_code_three_and_leaves_no_directory() {
    let sandbox = Sandbox::new("backup-dead");
    registered(&sandbox, "orders", "postgres://app@127.0.0.1:1/orders");

    keyed(&sandbox);

    sandbox
        .command(sandbox.work(), &["backup", "orders"])
        .env("PW", "whatever")
        .run()
        .expect_code(3)
        .expect_said("orders");

    assert!(
        !backups_dir(&sandbox).exists(),
        "a failed backup left {} behind",
        backups_dir(&sandbox).display()
    );
}

/// The half of R9's "Done when" that `--all` exists for: it does not stop, it names every
/// failure, and it exits with the code they share.
#[test]
fn all_runs_to_the_end_and_names_every_database_that_failed() {
    let sandbox = Sandbox::new("backup-all-dead");
    registered(&sandbox, "alpha", "postgres://app@127.0.0.1:1/alpha");
    registered(&sandbox, "beta", "postgres://app@127.0.0.1:1/beta");
    registered(&sandbox, "gamma", "postgres://app@127.0.0.1:1/gamma");
    keyed(&sandbox);

    let run = sandbox
        .command(sandbox.work(), &["backup", "--all"])
        .env("PW", "whatever")
        .run();

    run.expect_code(3)
        .expect_said("backed up 0 of 3")
        .expect_said("3 failed: alpha, beta, gamma");

    // Every one of them was tried, rather than the run stopping at the first.
    for name in ["alpha", "beta", "gamma"] {
        run.expect_said(&format!("{name}: psql"));
    }

    assert!(!backups_dir(&sandbox).exists(), "it wrote something");
}

/// Failures that do not agree become `1`, because no single specific code would be true.
///
/// One server unreachable and one password that could not be fetched at all are different
/// problems with different answers, and a monitor told "3" would go looking for a server
/// that is perfectly fine.
#[test]
fn all_exits_one_when_the_failures_do_not_agree() {
    let sandbox = Sandbox::new("backup-all-mixed");

    // Reachable as far as the registry goes, unreachable on the wire: code 3.
    registered(&sandbox, "alpha", "postgres://app@127.0.0.1:1/alpha");
    // Its password lives in a variable nothing sets, so it never gets as far as a socket:
    // code 2.
    sandbox
        .sloop(&[
            "db",
            "add",
            "beta",
            "--url",
            "postgres://app@127.0.0.1:1/beta",
            "--env",
            "NOTHING_SETS_THIS",
        ])
        .expect_code(0);
    keyed(&sandbox);

    sandbox
        .command(sandbox.work(), &["backup", "--all"])
        .env("PW", "whatever")
        .run()
        .expect_code(1)
        .expect_said("backed up 0 of 2")
        .expect_said("NOTHING_SETS_THIS is not set")
        .expect_said("2 failed: alpha, beta");
}

/// `--all` backs up everything, so a name alongside it is a contradiction rather than a
/// preference. clap refuses it before anything runs.
#[test]
fn a_name_and_all_together_are_refused() {
    let sandbox = Sandbox::new("backup-both");
    registered(&sandbox, "orders", "postgres://app@127.0.0.1:1/orders");

    sandbox
        .sloop(&["backup", "orders", "--all"])
        .expect_code(2)
        .expect_said("cannot be used with");

    assert!(!backups_dir(&sandbox).exists(), "it wrote something");
}

/// The two things somebody scheduling this needs from `--help`: where a backup goes, and
/// what the exit code will mean.
#[test]
fn backup_help_says_where_it_puts_things_and_what_it_exits_with() {
    let sandbox = Sandbox::new("backup-help");

    sandbox
        .sloop(&["backup", "--help"])
        .expect_code(0)
        .expect_said("backups/<engine>/<label>/<utc-timestamp>/")
        .expect_said("manifest.json")
        .expect_said("--all runs to the end whatever happens")
        .expect_said("Everything printed is in local time");
}
