//! What can be checked without a server: versions, the rule that compares them, and the
//! shape that keeps a fourth engine cheap to add.
//!
//! The parts that need a database are in `cluster_tests.rs`, which builds a throwaway
//! PostgreSQL and destroys it again.

use super::{Engine, Version, adapter_for, connection_string, postgres::Postgres};

#[test]
fn a_version_is_found_in_whatever_a_tool_printed() {
    let cases = [
        ("pg_dump (PostgreSQL) 17.2", Version::new(17, 2)),
        ("psql (PostgreSQL) 16.4", Version::new(16, 4)),
        ("pg_restore (PostgreSQL) 9.6.24", Version::new(9, 6)),
        ("psql (PostgreSQL) 18beta1", Version::new(18, 0)),
        ("pg_dump (PostgreSQL) 18rc1", Version::new(18, 0)),
        ("pg_dump (PostgreSQL) 20", Version::new(20, 0)),
        (
            "mysqldump  Ver 8.4.3 for Linux on x86_64",
            Version::new(8, 4),
        ),
        ("mariadb-dump from 11.4.4-MariaDB", Version::new(11, 4)),
    ];

    for (printed, want) in cases {
        assert_eq!(Version::from_tool_output(printed), Some(want), "{printed}");
    }
}

#[test]
fn nonsense_is_not_mistaken_for_a_version() {
    for printed in ["", "command not found", "pg_dump: error while loading"] {
        assert_eq!(Version::from_tool_output(printed), None, "{printed:?}");
    }
}

#[test]
fn server_version_num_reads_correctly_on_both_sides_of_ten() {
    // The encoding changed at 10, and getting it wrong turns 9.6 into 9.62.
    assert_eq!(
        Version::from_server_version_num(170_002),
        Version::new(17, 2)
    );
    assert_eq!(
        Version::from_server_version_num(180_000),
        Version::new(18, 0)
    );
    assert_eq!(
        Version::from_server_version_num(100_000),
        Version::new(10, 0)
    );
    assert_eq!(Version::from_server_version_num(90_624), Version::new(9, 6));
    assert_eq!(Version::from_server_version_num(90_405), Version::new(9, 4));
}

#[test]
fn versions_order_the_way_people_expect() {
    assert!(Version::new(9, 6) < Version::new(10, 0));
    assert!(Version::new(17, 2) < Version::new(18, 0));
    assert!(Version::new(17, 2) < Version::new(17, 10));
    assert!(Version::new(20, 0) > Version::new(9, 99));
}

/// The rule that stops a broken backup: `pg_dump` cannot read a server newer than itself.
///
/// This is checked against many pairs rather than against whichever PostgreSQL happens to
/// be installed, because the installed one only ever proves its own row of the table.
#[test]
fn a_client_older_than_the_server_is_refused_and_an_equal_one_is_not() {
    let allowed = [
        (Version::new(17, 2), Version::new(17, 0)),
        (Version::new(17, 0), Version::new(17, 6)),
        (Version::new(18, 0), Version::new(17, 2)),
        (Version::new(20, 1), Version::new(9, 6)),
        (Version::new(10, 0), Version::new(9, 6)),
    ];
    let refused = [
        (Version::new(17, 2), Version::new(18, 0)),
        (Version::new(16, 9), Version::new(17, 0)),
        (Version::new(9, 6), Version::new(10, 0)),
        (Version::new(17, 0), Version::new(20, 0)),
    ];

    for (client, server) in allowed {
        let adapter = Postgres::default().pretending_to_be(client);
        assert!(
            adapter.refuse_an_old_client(server).is_ok(),
            "client {client} should be allowed to dump server {server}"
        );
    }

    for (client, server) in refused {
        let adapter = Postgres::default().pretending_to_be(client);
        let failure = adapter
            .refuse_an_old_client(server)
            .expect_err(&format!("client {client} must not dump server {server}"));

        // Setup, not a dump failure: nothing was attempted and retrying will not help.
        assert_eq!(failure.exit().code(), 2, "{client} vs {server}");
        assert!(failure.message().contains(&client.to_string()));
        assert!(failure.message().contains(&server.to_string()));
        assert!(
            failure
                .hint_text()
                .is_some_and(|hint| hint.contains(&server.major.to_string())),
            "the hint has to say which version to install"
        );
    }
}

/// Adding an engine should be a small job. This is the check that the seam is real: every
/// variant resolves to something, and the one that is not built yet says so rather than
/// panicking or silently doing nothing.
#[test]
fn every_engine_resolves_to_an_adapter_or_to_a_clear_refusal() {
    assert_eq!(Engine::ALL.len(), 3, "a new engine needs a line in ALL");

    for engine in Engine::ALL {
        match adapter_for(engine) {
            Ok(adapter) => assert_eq!(adapter.engine(), engine),
            Err(failure) => {
                assert_eq!(failure.exit().code(), 2, "{engine}");
                assert!(failure.message().contains(engine.scheme()), "{engine}");
                assert!(failure.hint_text().is_some(), "{engine}");
            }
        }
    }
}

#[test]
fn postgresql_is_the_one_that_is_built() {
    let adapter = adapter_for(Engine::Postgres).expect("PostgreSQL is built");

    assert_eq!(adapter.engine(), Engine::Postgres);
    assert!(adapter.capabilities().custom_format);
    assert!(adapter.capabilities().parallel_restore);
}

#[test]
fn the_engines_keep_their_ports_and_schemes() {
    assert_eq!(Engine::Postgres.default_port(), 5432);
    assert_eq!(Engine::Mysql.default_port(), 3306);
    assert_eq!(Engine::Mariadb.default_port(), 3306);

    assert_eq!(Engine::Postgres.scheme(), "postgres");
    assert_eq!(Engine::Mysql.scheme(), "mysql");
    assert_eq!(Engine::Mariadb.scheme(), "mariadb");
}

#[test]
fn a_connection_string_names_everything_except_the_password() {
    let shown = connection_string(Engine::Postgres, "app_rw", "db.internal", 5433, "app");

    assert_eq!(shown, "postgres://app_rw@db.internal:5433/app");
    assert!(!shown.contains(':') || !shown.contains("//app_rw:"));
}
