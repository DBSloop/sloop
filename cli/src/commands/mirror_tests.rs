//! The guard, which is the part of a mirror that has to be right before anything runs.
//!
//! **A self-mirror is the one way this command can destroy data it was asked to copy.** It
//! clears the destination and then restores a dump of the source into it — so if the two are
//! the same connection, the source is emptied and refilled from a dump of itself, which
//! works right up until the dump fails halfway. Everything else about a mirror needs two
//! live servers and is exercised against a real cluster.

use super::{refuse_a_self_mirror, same_host};
use crate::engine::Engine;
use crate::exit::Exit;
use crate::registry::file::Database;
use crate::secret::Route;

/// A record, as a registry would hold it.
fn record(host: &str, port: u16, database: &str) -> Database {
    Database {
        engine: Engine::Postgres,
        host: host.to_owned(),
        port,
        database: database.to_owned(),
        user: "app".to_owned(),
        password: Route::Keyring,
    }
}

/// Two names, one database. Refused.
#[test]
fn the_same_connection_under_two_names_is_refused() {
    let from = record("db.internal", 5432, "orders");
    let into = record("db.internal", 5432, "orders");

    let failure =
        refuse_a_self_mirror("live", &from, "copy", &into).expect_err("that is the same database");

    assert_eq!(failure.exit(), Exit::Usage);
    let said = failure.message();
    assert!(said.contains("live") && said.contains("copy"), "{said}");
    assert!(said.contains("db.internal:5432/orders"), "{said}");
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("destroy the source")),
        "it has to say what would have happened: {:?}",
        failure.hint_text()
    );
}

/// **Host, port and database together**, which is the `Done when` of `R14`. Any one of them
/// differing is a real copy and has to go through.
#[test]
fn one_difference_in_any_of_the_three_is_allowed() {
    let from = record("db.internal", 5432, "orders");

    for into in [
        record("other.internal", 5432, "orders"),
        record("db.internal", 5433, "orders"),
        record("db.internal", 5432, "orders_staging"),
    ] {
        refuse_a_self_mirror("live", &from, "copy", &into)
            .expect("a different host, port or database is a real copy");
    }
}

/// **A guard that can be walked past by spelling the host differently is not a guard.**
#[test]
fn localhost_and_the_loopback_address_are_the_same_host() {
    for (one, two) in [
        ("localhost", "127.0.0.1"),
        ("127.0.0.1", "localhost"),
        ("LOCALHOST", "127.0.0.1"),
        ("::1", "localhost"),
        ("[::1]", "127.0.0.1"),
        ("DB.internal", "db.internal"),
    ] {
        assert!(same_host(one, two), "{one} and {two} are the same machine");
        refuse_a_self_mirror(
            "live",
            &record(one, 5432, "app"),
            "copy",
            &record(two, 5432, "app"),
        )
        .expect_err("{one} and {two} are the same database");
    }
}

/// And two machines that merely look alike are not the same machine.
#[test]
fn different_hosts_stay_different() {
    for (one, two) in [
        ("localhost", "db.internal"),
        ("127.0.0.1", "127.0.0.2"),
        ("db.internal", "db2.internal"),
        ("", "localhost"),
    ] {
        assert!(
            !same_host(one, two),
            "{one} and {two} are not the same host"
        );
    }
}

/// A database name differing only in case is a different database on most of the platforms
/// this runs against, so it is not caught by the guard — and must not be.
#[test]
fn database_names_are_compared_exactly() {
    refuse_a_self_mirror(
        "live",
        &record("db.internal", 5432, "Orders"),
        "copy",
        &record("db.internal", 5432, "orders"),
    )
    .expect("two names that differ in case are two databases");
}
