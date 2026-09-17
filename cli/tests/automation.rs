//! The automation contract — `R17`.
//!
//! **Two claims, both about a run nobody is watching.** Two runs against one database produce
//! one success and one exit `7`; and every command run with standard input closed either
//! finishes or exits `2` — none of them hangs.
//!
//! The second is the one that needs saying out loud, because a hang is the failure this tool
//! can least afford and the only one a test can miss by passing: a command that waits forever
//! on a question nobody will read does not fail, it simply never comes back, and a scheduler
//! notices some hours later when the next run piles up behind it. So every command in this
//! file is run with a deadline, and overrunning it is the assertion.

mod support;

use std::time::Duration;

use support::{Sandbox, a_postgres_client_is_installed, skipping};

/// Long enough for a real command on a slow machine, short enough that a hung test is a
/// failed test rather than a build nobody can cancel.
const PATIENCE: Duration = Duration::from_secs(60);

/// A sandbox with one database registered against a server nothing listens on.
fn with_one(label: &str) -> Sandbox {
    let sandbox = Sandbox::new(label);
    sandbox
        .sloop(&[
            "db",
            "add",
            "orders",
            "--url",
            "postgres://app@127.0.0.1:1/orders",
            "--password-from",
            "echo whatever",
        ])
        .expect_code(0);
    // A registry with no keypair makes one on the first backup and then refuses to go on
    // until it has been copied somewhere — which is `R11`, and is exit 2 before a connection
    // is ever opened. These tests are about what happens *after* that, so it is done here.
    sandbox.sloop(&["key", "export"]).expect_code(0);
    sandbox
}

/// **The `Done when`: two concurrent runs, one success and one `7`.**
///
/// The lock is taken before anything is contacted, so this needs no server: both runs reach
/// the lock, one takes it, and the other is refused. Which of them wins is the operating
/// system's business and not something to assert — what is asserted is that exactly one did.
#[test]
fn two_runs_against_one_database_produce_one_success_and_one_seven() {
    let sandbox = with_one("lock-race");
    let store = sandbox.global_dir();

    // `backup` is the command the rule was written for. The one that is refused gets 7
    // without contacting anything.
    let held = sloop_lib_lock(&store, "orders");

    let refused = sandbox.sloop(&["backup", "orders"]);
    refused
        .expect_code(7)
        .expect_said("another sloop run is working on orders")
        .expect_said("did not start");

    drop(held);

    // With the lock free, the same command gets past it. **Not** which code it then fails
    // with: the server is not there either way, and whether that is a refused connection or
    // a missing client is the machine's business and not what this test is about.
    sandbox.sloop(&["backup", "orders"]).expect_not_code(7);
}

/// Take the lock the way another process would, using the binary's own directory layout.
///
/// A second `sloop` process would do this for real; a file held open here is the same lock,
/// because the lock belongs to the operating system rather than to sloop.
fn sloop_lib_lock(store: &std::path::Path, label: &str) -> std::fs::File {
    let locks = store.join("locks");
    std::fs::create_dir_all(&locks).expect("a locks directory");

    let file = std::fs::File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(locks.join(format!("{label}.lock")))
        .expect("a lockfile");
    file.try_lock().expect("nobody else has it");
    std::fs::write(
        locks.join(format!("{label}.who")),
        "`sloop backup`, process 1, since now\n",
    )
    .expect("a note beside it");
    file
}

/// Two different databases do not wait for each other, which is what "per database" means.
#[test]
fn two_databases_run_at_the_same_time() {
    let sandbox = with_one("lock-two");
    sandbox
        .sloop(&[
            "db",
            "add",
            "app",
            "--url",
            "postgres://app@127.0.0.1:1/app",
            "--password-from",
            "echo whatever",
        ])
        .expect_code(0);

    let held = sloop_lib_lock(&sandbox.global_dir(), "orders");

    // `orders` is locked; `app` is not, so it gets past the lock and on with its run.
    sandbox.sloop(&["backup", "orders"]).expect_code(7);
    sandbox.sloop(&["backup", "app"]).expect_not_code(7);

    drop(held);
}

/// **The `Done when`: nothing hangs with standard input closed.**
///
/// Every command in the tree, in the shape somebody would actually schedule, each with a
/// deadline. Anything that finishes is fine whatever it exits with; anything that does not is
/// the failure this test exists for.
#[test]
fn no_command_hangs_with_stdin_closed() {
    let sandbox = with_one("no-hang");
    let project = sandbox.make_dir("demo");

    let commands: &[&[&str]] = &[
        &["init"],
        &["doctor", "--offline"],
        &["doctor"],
        &["db", "list"],
        &["db", "test"],
        &["db", "test", "orders"],
        &["db", "add", "second", "--url", "postgres://a@h/d"],
        &["db", "create", "made", "--engine", "postgres"],
        &["db", "edit", "orders", "--host", "elsewhere"],
        &["db", "rename", "orders", "renamed"],
        &["db", "remove", "orders"],
        &["db", "drop", "orders"],
        &["backup", "orders"],
        &["backup", "--all"],
        &["backups", "list"],
        &["backups", "prune"],
        &["backups", "prune", "--keep", "7"],
        &["restore", "orders"],
        &["mirror", "orders"],
        &["mirror", "orders", "--to", "orders"],
        &["mirror", "orders", "--create", "fresh"],
        &["sync", "orders"],
        &["sync", "orders", "--to", "orders"],
        &["key", "export"],
        &["key", "import"],
        &["uninstall"],
        &[],
    ];

    for command in commands {
        let named = format!("sloop {}", command.join(" "));
        let started = std::time::Instant::now();

        // `Sandbox` closes standard input for every run, which is exactly what a cron line
        // gives a command. A deadline around it turns a hang into a failure rather than a
        // test suite that never returns.
        let run = sandbox.sloop_in(&project, command);
        let took = started.elapsed();

        assert!(
            took < PATIENCE,
            "`{named}` took {took:?} with nothing to read from standard input — that is a hang"
        );
        assert!(
            run.code().is_some(),
            "`{named}` did not exit with a code at all"
        );
    }
}

/// **A question that cannot be asked exits 2 naming the flag that answers it.** That is rule
/// 4, and it is the half of "does not hang" that is worth something: a command that exits 1
/// saying "cannot continue" is not hanging either, and is no use to whoever wrote the cron
/// line.
#[test]
fn a_question_with_nobody_to_ask_names_the_flag_instead() {
    let sandbox = with_one("no-hang-flags");
    sandbox
        .sloop(&[
            "db",
            "add",
            "second",
            "--url",
            "postgres://app@127.0.0.1:1/other",
            "--password-from",
            "echo whatever",
        ])
        .expect_code(0);

    for (command, flag) in [
        (vec!["db", "drop", "orders"], "--confirm orders"),
        (vec!["restore", "orders"], "--confirm orders"),
        (vec!["mirror", "orders", "--to", "second"], "--confirm"),
        (vec!["db", "remove", "orders"], "--yes"),
        (vec!["mirror", "orders"], "--to <NAME>"),
        (vec!["sync", "orders"], "--to <NAME>"),
        (vec!["sync", "orders", "--to", "second"], "--confirm"),
        (
            vec!["db", "create", "made", "--engine", "postgres"],
            "--role-password-stdin",
        ),
    ] {
        let run = sandbox.sloop(&command);
        run.expect_code(2).expect_said(flag);
    }
}

/// **The frozen exit codes, checked against what actually produces them.** Rule 7 says they
/// are frozen at 1.0; a test that names them is how a change becomes a deliberate one.
#[test]
fn the_exit_codes_are_what_automation_was_promised() {
    let sandbox = with_one("exit-codes");

    // 0 — it worked.
    sandbox.sloop(&["db", "list"]).expect_code(0);

    // 2 — bad usage, and an unknown name.
    sandbox.sloop(&["definitely-not-a-command"]).expect_code(2);
    sandbox
        .sloop(&["backup", "no-such-database"])
        .expect_code(2);

    // 3 — the server could not be reached. It takes a client to be refused by a server: with
    // no `psql` at all the run cannot reach a server to be refused *by*, and sloop says so
    // and exits 2, which is a different promise and is checked above.
    if a_postgres_client_is_installed() {
        sandbox.sloop(&["db", "test", "orders"]).expect_code(3);
    } else {
        skipping("the exit 3 half of the contract");
    }

    // 7 — somebody else has it.
    let held = sloop_lib_lock(&sandbox.global_dir(), "orders");
    sandbox.sloop(&["backup", "orders"]).expect_code(7);
    drop(held);

    // 8 — `doctor` found something that will break a backup. Not provoked here: it needs a
    // live server with a role short of a grant, which `engine::cluster_tests` builds.
}

/// Nothing writes a spinner, a progress bar or a carriage return into a pipe. A log full of
/// `\r` is a log nobody can read, and the destination here is never a terminal.
#[test]
fn nothing_animates_into_a_pipe() {
    let sandbox = with_one("no-spinners");

    for command in [
        vec!["db", "list"],
        vec!["doctor", "--offline"],
        vec!["backups", "list"],
        vec!["db", "test", "orders"],
    ] {
        let run = sandbox.sloop(&command);
        for stream in [run.stdout(), run.stderr()] {
            assert!(
                !stream.contains('\r'),
                "`sloop {}` wrote a carriage return into a pipe",
                command.join(" ")
            );
            assert!(
                !stream.contains('\u{8}'),
                "`sloop {}` wrote a backspace into a pipe",
                command.join(" ")
            );
        }
    }
}
