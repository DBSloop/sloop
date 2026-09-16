//! `sloop sync`, through the real binary, in a sandbox of its own.
//!
//! **No server and no keyring**, on the same terms as `tests/mirror.rs`. What is checked here
//! is everything a sync settles before it opens a socket: what it refuses, and what it says
//! with nobody there to answer. The merge itself — the order, the keeping, the sequences —
//! needs two live databases and is driven against a real PostgreSQL.
//!
//! **Every run in this file has no terminal**, because `Command::output` closes standard
//! input. That is exactly what a cron line looks like, and rule 4 is the rule that has to
//! hold in it.
//!
//! `127.0.0.1:1` is the unreachable server throughout.

mod support;

use support::Sandbox;

/// One registered database on a server nothing listens on.
fn registered(sandbox: &Sandbox, name: &str, url: &str) {
    sandbox
        .sloop(&["db", "add", name, "--url", url, "--env", "PW"])
        .expect_code(0);
}

/// A sandbox with a source and a destination in it, both pointed at nothing.
fn with_two(label: &str) -> Sandbox {
    let sandbox = Sandbox::new(label);
    registered(&sandbox, "live", "postgres://app@127.0.0.1:1/orders");
    registered(
        &sandbox,
        "staging",
        "postgres://app@127.0.0.1:1/orders_staging",
    );
    sandbox
}

/// **Rule 5 holds in a crontab.** A sync replaces rows in a database somebody is using, so
/// the name is typed — and with no terminal, `--confirm` is the only way to type it.
#[test]
fn replacing_rows_needs_the_name_and_says_which_flag_gives_it() {
    let sandbox = with_two("sync-confirm");

    sandbox
        .sloop(&["sync", "live", "--to", "staging"])
        .expect_code(2)
        .expect_said("replacing rows in a database")
        .expect_said("no terminal")
        .expect_said("--confirm orders_staging");
}

/// A `--confirm` naming the wrong database is a scheduled run pointed at something it did
/// not mean, and nothing is contacted before it is caught.
#[test]
fn a_confirm_naming_the_wrong_database_is_refused_before_anything_is_contacted() {
    let sandbox = with_two("sync-wrong-name");

    sandbox
        .sloop(&["sync", "live", "--to", "staging", "--confirm", "orders"])
        .expect_code(2)
        .expect_said("--confirm says orders")
        .expect_said("nothing was contacted");
}

/// `-y` answers questions. It has never been a way to name what is being changed.
#[test]
fn yes_is_not_a_typed_name() {
    let sandbox = with_two("sync-yes");

    sandbox
        .sloop(&["sync", "live", "--to", "staging", "-y"])
        .expect_code(2)
        .expect_said("needs its name typed");
}

/// **Syncing a database into itself is refused**, on host, port and database together, and
/// `localhost` counts as `127.0.0.1` — the same guard `mirror` uses, with the consequence
/// worded for this command.
#[test]
fn syncing_a_database_into_itself_is_refused() {
    let sandbox = Sandbox::new("sync-itself");
    registered(&sandbox, "live", "postgres://app@localhost:1/orders");
    registered(&sandbox, "copy", "postgres://other@127.0.0.1:1/orders");

    sandbox
        .sloop(&["sync", "live", "--to", "copy"])
        .expect_code(2)
        .expect_said("are the same database")
        .expect_said("merge the source into itself");
}

/// Two engines is `R29`'s question, and until then it is a refusal that says so.
#[test]
fn across_engines_is_refused_and_names_where_that_gets_decided() {
    let sandbox = Sandbox::new("sync-engines");
    registered(&sandbox, "live", "postgres://app@127.0.0.1:1/orders");
    registered(&sandbox, "other", "mysql://app@127.0.0.1:1/orders_copy");

    sandbox
        .sloop(&["sync", "live", "--to", "other"])
        .expect_code(2)
        .expect_said("live is postgres and other is mysql");
}

/// An unknown name is a usage error, before a password is fetched for a connection that was
/// never going to be made.
#[test]
fn an_unknown_database_is_a_usage_error() {
    let sandbox = with_two("sync-unknown");

    sandbox
        .sloop(&["sync", "live", "--to", "nope"])
        .expect_code(2)
        .expect_said("no database is registered as nope");
}

/// Both ends are required, and leaving one out says which.
#[test]
fn a_sync_needs_a_source_and_a_destination() {
    let sandbox = with_two("sync-args");

    sandbox
        .sloop(&["sync", "live"])
        .expect_code(2)
        .expect_said("--to <NAME>");

    sandbox
        .sloop(&["sync"])
        .expect_code(2)
        .expect_said("<SOURCE>");
}

/// With every flag it needs and a server nothing listens on, it fails on the connection —
/// exit 3 — rather than anywhere earlier or later.
#[test]
fn an_unreachable_source_is_a_connection_failure() {
    let sandbox = with_two("sync-unreachable");

    sandbox
        .command(
            sandbox.work(),
            &[
                "sync",
                "live",
                "--to",
                "staging",
                "--confirm",
                "orders_staging",
            ],
        )
        .env("PW", "whatever")
        .run()
        .expect_code(3);
}

/// `--help` has to explain what makes this command different from `mirror`, because choosing
/// the wrong one of the two is the expensive mistake here.
#[test]
fn the_help_says_what_a_merge_keeps() {
    let sandbox = Sandbox::new("sync-help");

    sandbox
        .sloop(&["sync", "--help"])
        .expect_code(0)
        .expect_said("--to <NAME>")
        .expect_said("--safe");

    sandbox
        .sloop(&["help", "sync"])
        .expect_code(0)
        .expect_said("A merge, not a copy")
        .expect_said("kept, and reported")
        .expect_said("no primary key is skipped")
        .expect_said("point at each other");
}

/// **The destination does not have to exist here either.** The owner asked for the same flow
/// in both copying commands: *"the same create db flow will be executed during sync or mirror
/// if db is not present with provided name"*.
#[test]
fn a_destination_that_is_not_there_yet_can_be_made() {
    let sandbox = with_two("sync-create");

    // The same two refusals mirror has, in sync's words.
    sandbox
        .sloop(&["sync", "live", "--create", "staging"])
        .expect_code(2)
        .expect_said("staging is already registered")
        .expect_said("--to staging goes into it");

    sandbox
        .sloop(&["sync", "live", "--to", "stagin"])
        .expect_code(2)
        .expect_said("no database is registered as stagin")
        .expect_said("--create stagin");

    // Unattended, `--create` needs the two password flags named together.
    sandbox
        .sloop(&["sync", "live", "--create", "fresh"])
        .expect_code(2)
        .expect_said("--role-password-stdin")
        .expect_said("--superuser-password-command");
}

/// With neither flag and no terminal it exits 2 naming both, rather than hanging.
#[test]
fn with_no_destination_and_no_terminal_it_names_both_flags_and_stops() {
    let sandbox = with_two("sync-nowhere");

    sandbox
        .sloop(&["sync", "live"])
        .expect_code(2)
        .expect_said("no terminal")
        .expect_said("--to <NAME>")
        .expect_said("--create <NAME>");
}

/// The creating flags only mean something under `--create`, exactly as on `mirror`.
#[test]
fn the_creating_flags_only_mean_something_under_create() {
    let sandbox = with_two("sync-orphan-flags");

    sandbox
        .sloop(&["sync", "live", "--role", "staging_app"])
        .expect_code(2)
        .expect_said("--create <NAME>");

    sandbox
        .sloop(&["sync", "live", "--to", "staging", "--role", "staging_app"])
        .expect_code(2)
        .expect_said("cannot be used with");
}

/// **R15a.** `sync` takes the same `--table` on the same terms.
#[test]
fn table_narrows_a_sync_on_the_same_terms() {
    let sandbox = with_two("sync-scoped");

    sandbox
        .sloop(&["help", "sync"])
        .expect_code(0)
        .expect_said("--table narrows it to some of the tables")
        .expect_said("--with-references")
        .expect_said("keep their order among themselves");

    sandbox
        .sloop(&["sync", "--help"])
        .expect_code(0)
        .expect_said("--table <PATTERN>");

    sandbox
        .sloop(&["sync", "live", "--to", "staging", "--with-references"])
        .expect_code(2)
        .expect_said("--table <PATTERN>");
}
