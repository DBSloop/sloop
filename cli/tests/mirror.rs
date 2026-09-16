//! `sloop mirror`, through the real binary, in a sandbox of its own.
//!
//! **No server and no keyring**, on the same terms as `tests/db.rs` and `tests/backup.rs`:
//! both would make these tests depend on the machine, and the keyring is real, shared and
//! outside any sandbox. What is checked here is everything a mirror settles *before* it
//! opens a socket — which destination it is going to, what it refuses, and what it says when
//! there is nobody there to ask.
//!
//! That is most of `R14a`. The destination being optional is a question about flags, names
//! and rule 4, and rule 4 is the half that has to hold with stdin closed: **every run in this
//! file has no terminal**, because `Command::output` closes standard input, which is exactly
//! what a cron line looks like. A copy that actually lands is driven against a real
//! PostgreSQL built for the purpose.
//!
//! `127.0.0.1:1` is the unreachable server throughout. Nothing listens on port 1, so any
//! command that gets as far as connecting fails there and nowhere earlier — which is what
//! makes "it never got that far" a testable claim.

mod support;

use support::Sandbox;

/// One registered database on a server nothing listens on.
fn registered(sandbox: &Sandbox, name: &str, url: &str) {
    sandbox
        .sloop(&["db", "add", name, "--url", url, "--env", "PW"])
        .expect_code(0);
}

/// A sandbox with `live` in it, pointed at nothing.
fn with_a_source(label: &str) -> Sandbox {
    let sandbox = Sandbox::new(label);
    registered(&sandbox, "live", "postgres://app@127.0.0.1:1/orders");
    sandbox
}

/// The registry file as it stands, so "nothing was created" can be checked on the thing
/// that outlives the run rather than on what the run printed.
fn written(sandbox: &Sandbox) -> String {
    let path = sandbox.global_dir().join("registry.toml");
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// **`R14a`'s "Done when", the half about not hanging.** No destination, no terminal: the
/// two flags that would have answered, named together, and exit 2.
#[test]
fn with_no_destination_and_no_terminal_it_names_both_flags_and_stops() {
    let sandbox = with_a_source("mirror-nowhere");

    sandbox
        .sloop(&["mirror", "live"])
        .expect_code(2)
        .expect_said("no terminal")
        .expect_said("--to <NAME>")
        .expect_said("--create <NAME>");
}

/// **The other unattended shape.** `--create` with nobody there needs two passwords and only
/// one of them can come down a pipe, so both flags are named at once rather than one per run.
#[test]
fn creating_a_destination_unattended_names_every_flag_it_needs_at_once() {
    let sandbox = with_a_source("mirror-create-unattended");

    sandbox
        .sloop(&["mirror", "live", "--create", "staging"])
        .expect_code(2)
        .expect_said("--role-password-stdin")
        .expect_said("--superuser-password-command");

    assert!(
        !written(&sandbox).contains("staging"),
        "it refused and registered something anyway:\n{}",
        written(&sandbox)
    );
}

/// **A typo in `--to` stays a typo.** Quietly creating a database because a name was
/// misspelled is how `--to prod` becomes a new database called `prod` beside the one that
/// was meant — so the refusal stands, and names the flag that would have meant it.
#[test]
fn an_unregistered_destination_is_refused_and_names_the_flag_that_would_create_it() {
    let sandbox = with_a_source("mirror-typo");

    sandbox
        .sloop(&["mirror", "live", "--to", "stagin"])
        .expect_code(2)
        .expect_said("no database is registered as stagin")
        .expect_said("--create stagin");

    assert!(
        !written(&sandbox).contains("stagin"),
        "nothing should have been registered:\n{}",
        written(&sandbox)
    );
}

/// And the same mistake the other way round: `--create` on a name that is already taken.
#[test]
fn creating_a_name_that_is_already_registered_is_refused_and_names_to() {
    let sandbox = with_a_source("mirror-taken");

    sandbox
        .sloop(&["mirror", "live", "--create", "live"])
        .expect_code(2)
        .expect_said("live is already registered")
        .expect_said("--to live");
}

/// **The self-mirror guard fires on a database that does not exist yet.** `--create` names
/// the source's own database on the source's own server, spelled as `localhost` rather than
/// `127.0.0.1` — and it is refused before a superuser password is asked for, let alone used.
#[test]
fn creating_the_source_over_again_is_refused_before_anything_is_contacted() {
    let sandbox = Sandbox::new("mirror-itself");
    registered(&sandbox, "live", "postgres://app@localhost:1/orders");

    sandbox
        .sloop(&[
            "mirror",
            "live",
            "--create",
            "copy",
            "--host",
            "127.0.0.1",
            "--database",
            "orders",
            "--superuser-password-command",
            "echo never-used",
            "--role-password-stdin",
        ])
        .expect_code(2)
        .expect_said("are the same database")
        .expect_said("would destroy the source");
}

/// **The source is proved readable before anything is created.** With every flag a
/// `--create` needs and a server nothing listens on, the run fails where it should — on the
/// connection, exit 3 — and leaves no registry entry behind.
#[test]
fn a_source_that_cannot_be_read_leaves_no_new_database_registered() {
    let sandbox = with_a_source("mirror-create-unreachable");

    sandbox
        .command(
            sandbox.work(),
            &[
                "mirror",
                "live",
                "--create",
                "staging",
                "--superuser-password-command",
                "echo never-used",
                "--role-password-stdin",
            ],
        )
        .env("PW", "whatever")
        .stdin(b"a-password-for-the-new-role")
        .run()
        .expect_code(3);

    assert!(
        !written(&sandbox).contains("staging"),
        "a copy that never read its source registered a destination anyway:\n{}",
        written(&sandbox)
    );
}

/// **The flags that only mean something under `--create` say so.** A `--role` passed to a
/// copy that is not making anything is somebody expecting it to do something, and a value
/// silently ignored is the worst answer available.
///
/// Two shapes, because there are two ways to mean it wrong: on its own it is a missing
/// `--create`, and alongside `--to` it is a flag that cannot be there at all.
#[test]
fn the_creating_flags_only_mean_something_under_create() {
    let sandbox = with_a_source("mirror-orphan-flags");

    for flag in [
        ["--host", "db2.internal"],
        ["--port", "5433"],
        ["--superuser", "postgres"],
        ["--database", "orders_staging"],
        ["--role", "staging_app"],
    ] {
        sandbox
            .sloop(&["mirror", "live", flag[0], flag[1]])
            .expect_code(2)
            .expect_said("--create <NAME>");

        sandbox
            .sloop(&["mirror", "live", "--to", "live", flag[0], flag[1]])
            .expect_code(2)
            .expect_said("cannot be used with")
            .expect_said(flag[0]);
    }
}

/// `--to` and `--create` are two answers to one question, and giving both is a usage error
/// rather than a guess about which was meant.
#[test]
fn a_destination_cannot_be_both_registered_and_new() {
    let sandbox = with_a_source("mirror-both");

    sandbox
        .sloop(&["mirror", "live", "--to", "live", "--create", "staging"])
        .expect_code(2)
        .expect_said("cannot be used with");
}

/// `--help` has to carry the new half, because a flag nobody can find is a flag that does
/// not exist.
#[test]
fn the_help_says_the_destination_can_be_made() {
    let sandbox = Sandbox::new("mirror-help");

    sandbox
        .sloop(&["mirror", "--help"])
        .expect_code(0)
        .expect_said("--create <NAME>")
        .expect_said("Making the destination");

    sandbox
        .sloop(&["help", "mirror"])
        .expect_code(0)
        .expect_said("The destination does not have to exist")
        .expect_said("The engine is not asked for");
}

/// **R15a.** `--table` is repeatable, takes a glob, and refuses a pattern that names nothing.
/// What it does once it has a selection needs two live databases; what it refuses does not.
#[test]
fn table_narrows_a_mirror_and_says_so() {
    let sandbox = with_a_source("mirror-scoped");

    sandbox
        .sloop(&["help", "mirror"])
        .expect_code(0)
        .expect_said("--table narrows it to some of the tables")
        .expect_said("drops and recreates only the tables you named")
        .expect_said("--with-references");

    sandbox
        .sloop(&["mirror", "--help"])
        .expect_code(0)
        .expect_said("--table <PATTERN>")
        .expect_said("--with-references");

    // `--with-references` says nothing on its own: it modifies a selection.
    sandbox
        .sloop(&["mirror", "live", "--to", "live", "--with-references"])
        .expect_code(2)
        .expect_said("--table <PATTERN>");
}
