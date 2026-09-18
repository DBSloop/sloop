//! `sloop service attach` and `detach`, through the real binary — `R25`.
//!
//! **What is checked here is the command**: the exit codes, what it says, what it leaves in
//! the table, and the two things somebody types twice by accident. What a *running* daemon
//! does with the list needs a loop and a clock, and is `service::cluster_tests` — where the
//! daemon's own round is turned against a real cluster while a second database is attached.
//!
//! **Every run here has no terminal**, because `Command::output` closes standard input. That
//! is rule 4's own condition, and it is the state a scheduled run is in: nothing below may
//! stop to ask anything, and a test that hung would be the failure.

mod support;

use support::Sandbox;

/// Register one globally, the way somebody would before attaching it.
///
/// `--global` explicitly: the working directory has no `.sloop`, so the global store is where
/// it would land anyway — but the service can only watch the global store, and a test that
/// relied on the default would stop proving that the day the default changed.
fn registered(sandbox: &Sandbox, name: &str) {
    sandbox
        .sloop(&[
            "db",
            "add",
            name,
            "--global",
            "--url",
            &format!("postgres://app@127.0.0.1:1/{name}"),
            "--env",
            "PW",
        ])
        .expect_code(0);
}

/// Which labels are attached, as the table holds them.
fn attached(sandbox: &Sandbox) -> Vec<String> {
    sandbox
        .ask(
            "SELECT d.label
               FROM monitored_database m
               JOIN service s ON s.id = m.service_id
               JOIN registered_database d ON d.id = m.registered_database_id
              WHERE s.name = 'sloop'
              ORDER BY d.label;",
        )
        .lines()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

/// **The whole command, both ways, twice each.**
///
/// One test and one sandbox: every step needs the state the one before it left, and a sandbox
/// is a database that `sloop setup` migrated.
#[test]
fn attaching_and_detaching_are_both_idempotent_and_leave_the_table_right() {
    let sandbox = Sandbox::new("service-attach");
    if !sandbox.has_a_registry() {
        support::skipping("sloop service attach: this machine has no PostgreSQL server");
        return;
    }

    registered(&sandbox, "orders");
    assert!(attached(&sandbox).is_empty());

    // 1. Attached, and it says what happens next rather than only that it worked.
    let run = sandbox.sloop(&["service", "attach", "orders"]);
    run.expect_code(0);
    run.expect_said("Attached");
    assert_eq!(attached(&sandbox), vec!["orders".to_owned()]);

    // 2. Twice is not an error — a provisioning script runs twice — and it says nothing
    //    changed rather than pretending something did.
    let again = sandbox.sloop(&["service", "attach", "orders"]);
    again.expect_code(0);
    again.expect_said("already attached");
    assert_eq!(attached(&sandbox), vec!["orders".to_owned()]);

    // 3. Detached, and the history line is printed even when there is no history yet.
    let off = sandbox.sloop(&["service", "detach", "orders"]);
    off.expect_code(0);
    off.expect_said("Detached");
    assert!(attached(&sandbox).is_empty());

    // 4. And detaching twice is not an error either.
    let off_again = sandbox.sloop(&["service", "detach", "orders"]);
    off_again.expect_code(0);
    off_again.expect_said("was not attached");

    // 5. The registration is untouched by either. Detaching stops sampling; it does not
    //    unregister anything.
    assert!(sandbox.registry_text().contains("orders"));
}

/// **Exit 2 and the registry it looked in**, because the likeliest reason a name is missing is
/// that it was registered into a project — and a service has no working directory to find one
/// from.
#[test]
fn a_name_the_global_registry_does_not_hold_is_a_usage_error_that_says_so() {
    let sandbox = Sandbox::new("service-unknown");
    if !sandbox.has_a_registry() {
        support::skipping("sloop service attach: this machine has no PostgreSQL server");
        return;
    }

    for command in [
        ["service", "attach", "nowhere"],
        ["service", "detach", "nowhere"],
    ] {
        let run = sandbox.sloop(&command);
        run.expect_code(2);
        run.expect_said("global registry");
        run.expect_said("sloop db list --global");
    }
}

/// A database registered to a *project* is not attachable, and the refusal is the same one —
/// which is the whole reason the message names the registry rather than only the name.
#[test]
fn a_project_database_cannot_be_attached() {
    let sandbox = Sandbox::new("service-project");
    if !sandbox.has_a_registry() {
        support::skipping("sloop service attach: this machine has no PostgreSQL server");
        return;
    }

    sandbox.sloop(&["init"]).expect_code(0);
    sandbox
        .sloop(&[
            "db",
            "add",
            "local-only",
            "--url",
            "postgres://app@127.0.0.1:1/local_only",
            "--env",
            "PW",
        ])
        .expect_code(0);

    let run = sandbox.sloop(&["service", "attach", "local-only"]);
    run.expect_code(2);
    run.expect_said("global registry");
}

/// **The one that a real machine would have hit first.** Attaching a database and then
/// registering another one must not quietly detach the first — `Store::write` replaces a
/// whole registry, and both of `R25`'s tables cascade off `registered_database.id`.
#[test]
fn registering_another_database_does_not_detach_the_ones_already_attached() {
    let sandbox = Sandbox::new("service-survives");
    if !sandbox.has_a_registry() {
        support::skipping("sloop service attach: this machine has no PostgreSQL server");
        return;
    }

    registered(&sandbox, "orders");
    sandbox
        .sloop(&["service", "attach", "orders"])
        .expect_code(0);

    registered(&sandbox, "reporting");
    assert_eq!(
        attached(&sandbox),
        vec!["orders".to_owned()],
        "registering a second database detached the first"
    );

    // Editing one is the same write, and the same question.
    sandbox
        .sloop(&["db", "edit", "orders", "--global", "--port", "5555"])
        .expect_code(0);
    assert_eq!(
        attached(&sandbox),
        vec!["orders".to_owned()],
        "editing a database detached it"
    );

    // Removing something else is too.
    sandbox
        .sloop(&["db", "remove", "reporting", "--global", "--yes"])
        .expect_code(0);
    assert_eq!(
        attached(&sandbox),
        vec!["orders".to_owned()],
        "removing another database detached this one"
    );
}

/// `status` answers about both halves, and a document a script can read comes out of `--json`
/// whatever the machine's service manager says.
#[test]
fn status_says_what_is_attached_and_that_nothing_has_read_it_yet() {
    let sandbox = Sandbox::new("service-status");
    if !sandbox.has_a_registry() {
        support::skipping("sloop service status: this machine has no PostgreSQL server");
        return;
    }

    registered(&sandbox, "orders");
    sandbox
        .sloop(&["service", "attach", "orders"])
        .expect_code(0);

    let plain = sandbox.sloop(&["service", "status"]);
    plain.expect_code(0);
    plain.expect_said("Watching");
    plain.expect_said("orders");
    plain.expect_said("not picked up yet");

    let run = sandbox.sloop(&["--json", "service", "status"]);
    run.expect_code(0);

    let document: serde_json::Value =
        serde_json::from_str(&run.stdout()).expect("--json produces one document");
    assert_eq!(document["command"], "service status");
    assert_eq!(document["ok"], true);

    let watching = document["result"]["watching"]
        .as_array()
        .expect("the result names what is watched");
    assert_eq!(watching.len(), 1);
    assert_eq!(watching[0]["label"], "orders");
    assert!(
        watching[0]["seen_at"].is_null(),
        "an attachment claims to have been read before anything ran: {}",
        watching[0]
    );
    assert_eq!(watching[0]["days_of_history"], 0);
}

/// **`status` never fails**, which is the property that lets it be the thing a monitoring
/// system runs. Not installed, not set up, nothing attached — all of it is an answer and none
/// of it is an error.
#[test]
fn status_answers_rather_than_failing_on_a_machine_that_was_never_set_up() {
    let sandbox = Sandbox::new("service-unset");
    let _ = std::fs::remove_file(sandbox.global_dir().join("server.toml"));

    let run = sandbox.sloop(&["service", "status"]);
    run.expect_code(0);
    run.expect_said("sloop setup");

    // Attaching, on the other hand, is a usage error that names the command to run — there is
    // nowhere to put the row.
    let attach = sandbox.sloop(&["service", "attach", "orders"]);
    attach.expect_code(2);
    attach.expect_said("sloop setup");
}

/// **`--dry-run` is a rehearsal, not a refusal.** It reads everything, checks everything and
/// writes nothing — so an unknown name is still exit `2` on a rehearsal, and a real attach
/// after one still attaches.
#[test]
fn a_dry_run_says_what_it_would_attach_and_changes_nothing() {
    let sandbox = Sandbox::new("service-dry");
    if !sandbox.has_a_registry() {
        support::skipping("sloop service attach: this machine has no PostgreSQL server");
        return;
    }

    registered(&sandbox, "orders");

    let run = sandbox.sloop(&["--dry-run", "service", "attach", "orders"]);
    run.expect_code(0);
    run.expect_said("would attach orders");
    assert!(
        attached(&sandbox).is_empty(),
        "a dry run attached it anyway"
    );

    // The checks still ran: an unknown name is refused on a rehearsal too.
    sandbox
        .sloop(&["--dry-run", "service", "attach", "nowhere"])
        .expect_code(2);

    // And for real, then a rehearsed detach that keeps it.
    sandbox
        .sloop(&["service", "attach", "orders"])
        .expect_code(0);

    let off = sandbox.sloop(&["--dry-run", "service", "detach", "orders"]);
    off.expect_code(0);
    off.expect_said("would stop the service watching orders");
    assert_eq!(
        attached(&sandbox),
        vec!["orders".to_owned()],
        "a dry run detached it anyway"
    );
}

/// **Renaming is still a delete and an insert, and this says so out loud.**
///
/// `Store::write` is handed the registry *after* the change, so it cannot tell a rename from a
/// remove-and-add — the old label is not in the list any more, its row goes, and
/// `monitored_database` and `bandwidth_day` cascade with it. Recorded in
/// `docs/OWNER-DECISIONS.md` under `R25` as the owner's to decide about, and asserted here so
/// that whoever changes it finds this test rather than finding out later.
#[test]
fn renaming_a_database_still_detaches_it() {
    let sandbox = Sandbox::new("service-rename");
    if !sandbox.has_a_registry() {
        support::skipping("sloop service attach: this machine has no PostgreSQL server");
        return;
    }

    registered(&sandbox, "orders");
    sandbox
        .sloop(&["service", "attach", "orders"])
        .expect_code(0);

    sandbox
        .sloop(&["db", "rename", "orders", "orders-prod", "--global"])
        .expect_code(0);

    assert!(
        attached(&sandbox).is_empty(),
        "a rename kept the attachment — if that is deliberate now, update R25 in \
         docs/OWNER-DECISIONS.md, which records that it does not"
    );
}
