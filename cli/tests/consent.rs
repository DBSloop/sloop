//! The consent contract, through the real binary, with nothing to ask at.
//!
//! **This is rule 4's test, and it is an integration test on purpose.** Every run here has
//! stdin closed — which is what a crontab, a systemd timer and a CI job all give a process —
//! and what has to be true is that no command hangs and every one of them names the flag
//! that would have answered for it. A unit test can check the branch; only the binary can
//! prove that nothing waits.
//!
//! **No server.** `db drop` is refused here before it opens a socket, which is the whole
//! point of checking the paperwork first; what it does once it is allowed to run is in
//! `engine::cluster_tests`.

mod support;

use support::Sandbox;

/// Register one, with a password route that keeps nothing.
fn registered(sandbox: &Sandbox, name: &str, url: &str) {
    sandbox
        .sloop(&["db", "add", name, "--url", url, "--env", "PW"])
        .expect_code(0);
}

/// One thing for `prune` to find, without a server: a directory from 2020 holding a dump
/// and no manifest, which is exactly what a run that was killed leaves behind. Old enough
/// that the grace period for a backup that may still be being written does not cover it.
fn seeded_backup(sandbox: &Sandbox, label: &str) {
    let directory = sandbox
        .global_dir()
        .join("backups")
        .join("postgres")
        .join(label)
        .join("20200101T000000Z");
    std::fs::create_dir_all(&directory).expect("a backup directory");
    std::fs::write(directory.join("dump"), b"not a finished backup").expect("a dump");
}

/// Every destructive command, with no terminal and no flag: each exits `2`, each names the
/// flag, and **none of them hangs**, which is the part that would make a scheduled run a
/// silent failure rather than a loud one.
#[test]
fn nothing_hangs_with_stdin_closed_and_every_refusal_names_its_flag() {
    let sandbox = Sandbox::new("consent-none");
    registered(
        &sandbox,
        "orders",
        "postgres://app@db.internal:5432/orders_live",
    );

    seeded_backup(&sandbox, "orders");

    let cases: &[(&[&str], &str)] = &[
        (&["db", "remove", "orders"], "--yes"),
        (&["db", "drop", "orders"], "--confirm orders_live"),
        (
            &["backups", "prune", "--keep", "0", "--include-broken"],
            "--yes",
        ),
    ];

    for (argv, flag) in cases {
        let run = sandbox.sloop(argv);
        run.expect_code(2);
        assert!(
            run.stderr().contains(flag),
            "`sloop {}` exited 2 without naming {flag}:\n{}",
            argv.join(" "),
            run.stderr()
        );
    }
}

/// `--yes` answers the questions that are questions.
#[test]
fn yes_answers_a_question_from_anywhere_on_the_line() {
    for argv in [
        ["db", "remove", "orders", "-y"],
        ["-y", "db", "remove", "orders"],
    ] {
        let sandbox = Sandbox::new("consent-yes");
        registered(
            &sandbox,
            "orders",
            "postgres://app@db.internal:5432/orders_live",
        );

        sandbox.sloop(&argv).expect_code(0);
        // And it is actually gone, not merely reported as gone.
        sandbox.sloop(&["db", "test", "orders"]).expect_code(2);
    }
}

/// **The rule that makes a cron line safe to read.** `-y` is not permission to destroy a
/// named thing, however many times it is passed.
#[test]
fn a_bare_yes_never_destroys_a_database() {
    let sandbox = Sandbox::new("consent-yes-not-enough");
    registered(
        &sandbox,
        "orders",
        "postgres://app@db.internal:5432/orders_live",
    );

    let run = sandbox.sloop(&["db", "drop", "orders", "--yes", "--force"]);

    run.expect_code(2);
    assert!(
        run.stderr().contains("--confirm orders_live"),
        "neither --yes nor --force may stand in for the name:\n{}",
        run.stderr()
    );
    // The record is untouched, which is what "nothing was changed" has to mean.
    let still = sandbox.sloop(&["db", "list"]);
    still.expect_code(0);
    assert!(still.stdout().contains("orders_live"), "{}", still.stdout());
}

/// A `--confirm` that names something else is a scheduled run pointed at the wrong
/// database. It is refused before anything is contacted.
#[test]
fn a_confirm_naming_the_wrong_database_is_refused_before_anything_is_contacted() {
    let sandbox = Sandbox::new("consent-wrong-name");
    registered(
        &sandbox,
        "orders",
        "postgres://app@db.internal:5432/orders_live",
    );

    let run = sandbox.sloop(&["db", "drop", "orders", "--confirm", "orders_staging"]);

    run.expect_code(2);
    let said = run.stderr();
    assert!(said.contains("orders_staging"), "{said}");
    assert!(said.contains("orders_live"), "{said}");
    assert!(said.contains("nothing was contacted"), "{said}");
}

/// The label is not the name. `db drop orders` destroys `orders_live`, and `--confirm` is
/// about the database rather than about what sloop files it under.
#[test]
fn the_label_is_not_the_name_that_has_to_be_typed() {
    let sandbox = Sandbox::new("consent-label");
    registered(
        &sandbox,
        "orders",
        "postgres://app@db.internal:5432/orders_live",
    );

    let run = sandbox.sloop(&["db", "drop", "orders", "--confirm", "orders"]);

    run.expect_code(2);
    assert!(
        run.stderr().contains("orders_live"),
        "it has to say which name it wanted:\n{}",
        run.stderr()
    );
}

/// `--force` is a separate concept, and the refusal it overrides says so.
#[test]
fn force_overrides_a_refusal_and_yes_does_not() {
    let sandbox = Sandbox::new("consent-force");
    let url = "postgres://app@db.internal:5432/orders_live";
    registered(&sandbox, "orders", url);

    // Registering the same name again is refused, and the refusal names --force.
    let refused = sandbox.sloop(&["db", "add", "orders", "--url", url, "--env", "PW"]);
    refused.expect_code(2);
    assert!(refused.stderr().contains("--force"), "{}", refused.stderr());

    // `--yes` is not that flag.
    sandbox
        .sloop(&["db", "add", "orders", "--url", url, "--env", "PW", "-y"])
        .expect_code(2);

    // `--force` is.
    sandbox
        .sloop(&[
            "db", "add", "orders", "--url", url, "--env", "PW", "--force",
        ])
        .expect_code(0);
}

/// The three flags are global, so they work before the subcommand as well as after it —
/// which is what lets a wrapper script put them in one place.
#[test]
fn the_consent_flags_are_global() {
    let sandbox = Sandbox::new("consent-global");

    for argv in [
        ["--help"].as_slice(),
        ["db", "--help"].as_slice(),
        ["db", "drop", "--help"].as_slice(),
        ["backups", "prune", "--help"].as_slice(),
    ] {
        let run = sandbox.sloop(argv);
        run.expect_code(0);
        let help = run.stdout();
        for flag in ["--yes", "--force", "--confirm"] {
            assert!(
                help.contains(flag),
                "`sloop {}` never mentioned {flag}",
                argv.join(" ")
            );
        }
    }
}

/// `--dry-run` is not consent and does not need any: it changes nothing, so it runs
/// unattended without a flag saying so.
#[test]
fn a_dry_run_needs_no_permission_at_all() {
    let sandbox = Sandbox::new("consent-dry-run");

    sandbox
        .sloop(&["backups", "prune", "--keep", "1", "--dry-run"])
        .expect_code(0);
}
