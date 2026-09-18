//! The guard, which is the part of a mirror that has to be right before anything runs.
//!
//! **A self-mirror is the one way this command can destroy data it was asked to copy.** It
//! clears the destination and then restores a dump of the source into it — so if the two are
//! the same connection, the source is emptied and refilled from a dump of itself, which
//! works right up until the dump fails halfway. Everything else about a mirror needs two
//! live servers and is exercised against a real cluster.

use super::{New, db, proposed, refuse_a_self_mirror, same_host};
use crate::engine::Engine;
use crate::exit::Exit;
use crate::registry::file::Database;
use crate::secret::Route;

/// The names settled, as [`db::name_it`] returns them.
///
/// **No terminal here**, which is the state these tests need: `name_it` asks only when there
/// is somebody to ask, so under a test harness it fills the defaults in exactly as an
/// unattended run does.
fn named(label: &str, database: Option<&str>, role: Option<&str>) -> db::Naming {
    db::name_it(label, database, role).expect("nothing is asked for without a terminal")
}

/// A record, as a registry would hold it.
fn record(host: &str, port: u16, database: &str) -> Database {
    Database {
        engine: Engine::Postgres,
        host: host.to_owned(),
        port,
        database: database.to_owned(),
        user: "app".to_owned(),
        password: Route::Keyring,
        reach: crate::ssh::Reach::Direct,
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

// ---------------------------------------------------------------------------------------
// R14a — the destination that does not exist yet
// ---------------------------------------------------------------------------------------

/// **The source's own server, and its port with it.** A staging copy beside the live
/// database is the case `--create` exists for, and the defaults are what make that one flag
/// rather than four.
#[test]
fn a_new_destination_lands_beside_its_source_by_default() {
    let from = record("db.internal", 5433, "orders");

    let made = proposed(&from, &named("staging", None, None), &New::default());

    assert_eq!(made.host, "db.internal");
    assert_eq!(
        made.port, 5433,
        "the source's port, not the engine's default"
    );
    assert_eq!(made.database, "staging", "the label names the database");
    assert_eq!(made.role, "staging", "and the database names its owner");
}

/// **The port follows the host.** A source listening on 5433 says nothing about what any
/// other machine listens on, so pointing `--host` somewhere else drops back to the engine's
/// own default rather than carrying a port that was only ever true of one server.
#[test]
fn another_host_gets_the_engines_default_port_rather_than_the_sources() {
    let from = record("db.internal", 5433, "orders");

    let elsewhere = proposed(
        &from,
        &named("staging", None, None),
        &New {
            host: Some("db2.internal"),
            ..New::default()
        },
    );
    assert_eq!(elsewhere.host, "db2.internal");
    assert_eq!(elsewhere.port, 5432);

    // Spelled differently, still the same machine — so still the source's port.
    let here = proposed(
        &record("localhost", 5433, "orders"),
        &named("staging", None, None),
        &New {
            host: Some("127.0.0.1"),
            ..New::default()
        },
    );
    assert_eq!(here.port, 5433);

    // And an explicit --port outranks both.
    let told = proposed(
        &from,
        &named("staging", None, None),
        &New {
            host: Some("db2.internal"),
            port: Some(6000),
            ..New::default()
        },
    );
    assert_eq!(told.port, 6000);
}

/// Everything that was said is used, and only what was not said is filled in.
#[test]
fn what_was_named_is_what_gets_made() {
    let from = record("db.internal", 5432, "orders");

    let made = proposed(
        &from,
        &named("staging", Some("orders_staging"), None),
        &New::default(),
    );
    assert_eq!(made.database, "orders_staging");
    assert_eq!(
        made.role, "orders_staging",
        "the role follows the database's name, not the label"
    );

    let owned = proposed(
        &from,
        &named("staging", Some("orders_staging"), Some("staging_app")),
        &New::default(),
    );
    assert_eq!(owned.role, "staging_app");
}

/// **The guard fires before the database exists**, which is the whole reason it takes three
/// values rather than a second record: `--create` has nothing to compare against yet.
#[test]
fn creating_the_source_over_again_is_refused_before_anything_is_made() {
    let from = record("localhost", 5432, "orders");

    // `--create orders --database orders` on the same server is the source.
    let made = proposed(
        &from,
        &named("copy", Some("orders"), None),
        &New {
            host: Some("127.0.0.1"),
            ..New::default()
        },
    );
    let failure = super::refuse_the_same_connection(
        "live",
        &from,
        "copy",
        &super::Onto {
            host: &made.host,
            port: made.port,
            database: &made.database,
            reach: &crate::ssh::Reach::Direct,
        },
        "would destroy the source",
    )
    .expect_err("that names the source");

    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure.message().contains("the same database"),
        "{}",
        failure.message()
    );

    // A different name on the same server is a real copy, and goes through.
    let beside = proposed(&from, &named("staging", None, None), &New::default());
    super::refuse_the_same_connection(
        "live",
        &from,
        "staging",
        &super::Onto {
            host: &beside.host,
            port: beside.port,
            database: &beside.database,
            reach: &crate::ssh::Reach::Direct,
        },
        "would destroy the source",
    )
    .expect("a second database on one server is a copy");
}

// ---------------------------------------------------------------------------------------
// what a copy compares, now that two databases can share an address — R19e
// ---------------------------------------------------------------------------------------

/// **The refusal `R19e` would have broken.** Every tunnelled database is registered at the
/// address its own server sees — almost always `127.0.0.1:5432` — so two behind two
/// different bastions are identical on host, port and name and are not remotely the same
/// database. A guard comparing only those three would refuse the ordinary copy that is the
/// whole reason somebody reaches for this feature.
#[test]
fn two_databases_behind_two_servers_are_not_the_same_database() {
    let over = |host: &str| {
        let mut record = record("127.0.0.1", 5432, "orders");
        record.reach = crate::ssh::Reach::Over(Box::new(crate::ssh::Through {
            server: crate::ssh::Server {
                host: host.to_owned(),
                port: 22,
                user: None,
                identity: None,
            },
            secret: None,
        }));
        record
    };

    let from = over("one.test");
    let into = over("other.test");
    refuse_a_self_mirror("live", &from, "copy", &into)
        .expect("two bastions is two databases, however the addresses read");

    // And the same server still is the same database, which is what the guard is for.
    let itself = over("one.test");
    refuse_a_self_mirror("live", &from, "copy", &itself)
        .expect_err("one server, one address, one database");

    // A direct database and a tunnelled one at the same address are two, as well: the local
    // `127.0.0.1:5432` and the one on the far side of a bastion are different machines.
    let here = record("127.0.0.1", 5432, "orders");
    refuse_a_self_mirror("live", &from, "copy", &here)
        .expect("one of them is on this machine and the other is not");
}

/// The identity is part of which login a tunnel is, so it is part of which database a
/// record names — the one place that decides it is `Server::credential_key`.
#[test]
fn the_same_server_under_two_keys_is_still_one_server_to_copy_between() {
    let with = |identity: Option<&str>| {
        let mut record = record("127.0.0.1", 5432, "orders");
        record.reach = crate::ssh::Reach::Over(Box::new(crate::ssh::Through {
            server: crate::ssh::Server {
                host: "one.test".to_owned(),
                port: 22,
                user: Some("deploy".to_owned()),
                identity: identity.map(std::path::PathBuf::from),
            },
            secret: None,
        }));
        record
    };

    // Two different keys are two different logins, so sloop treats them as two ways in and
    // does not refuse. Same key, same login, same database: refused.
    refuse_a_self_mirror("live", &with(Some("/a/key")), "copy", &with(Some("/b/key")))
        .expect("two keys are two logins");
    refuse_a_self_mirror("live", &with(Some("/a/key")), "copy", &with(Some("/a/key")))
        .expect_err("one key, one login, one database");
}
