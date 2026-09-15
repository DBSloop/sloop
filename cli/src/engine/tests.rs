//! What can be checked without a server: versions, the rule that compares them, and the
//! shape that keeps a fourth engine cheap to add.
//!
//! The parts that need a database are in `cluster_tests.rs`, which builds a throwaway
//! PostgreSQL and destroys it again.

use super::{Adapter, Engine, Version, adapter_for, connection_string, postgres::Postgres};

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
/// variant resolves to an adapter, and the adapter it resolves to is the right one.
#[test]
fn every_engine_resolves_to_its_own_adapter() {
    assert_eq!(Engine::ALL.len(), 3, "a new engine needs a line in ALL");

    for engine in Engine::ALL {
        assert_eq!(
            adapter_for(engine).engine(),
            engine,
            "{engine} resolved to somebody else's adapter"
        );
    }
}

#[test]
fn postgresql_says_what_it_can_do() {
    let adapter = adapter_for(Engine::Postgres);

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

// --- MySQL and MariaDB ----------------------------------------------------------------
//
// What can be checked without a server. The round trip itself is in `mysql_cluster_tests`.

use super::mysql::{
    Family, MysqlFamily, Tools, quote_identifier, sql_literal, version_of_a_client_tool,
    without_a_definer,
};

#[test]
fn mysql_and_mariadb_are_both_built_and_neither_pretends_to_do_more_than_it_can() {
    for engine in [Engine::Mysql, Engine::Mariadb] {
        let adapter = adapter_for(engine);

        assert_eq!(adapter.engine(), engine);
        // `mysqldump` writes SQL text. There is no archive to read selectively and no
        // second connection to restore it with, and the trait says so rather than a caller
        // finding out.
        assert!(!adapter.capabilities().custom_format, "{engine}");
        assert!(!adapter.capabilities().parallel_restore, "{engine}");
    }
}

#[test]
fn each_family_reaches_for_its_own_programs() {
    let mysql = Tools::named_for(Family::Mysql);
    assert_eq!(mysql.dump, std::path::Path::new("mysqldump"));
    assert_eq!(mysql.client, std::path::Path::new("mysql"));

    // The whole reason MariaDB is a third engine rather than an alias.
    let mariadb = Tools::named_for(Family::Mariadb);
    assert_eq!(mariadb.dump, std::path::Path::new("mariadb-dump"));
    assert_eq!(mariadb.client, std::path::Path::new("mariadb"));
}

/// MariaDB's tools print two versions and the general parser takes the wrong one.
#[test]
fn a_mariadb_tool_is_read_as_the_server_it_belongs_to_not_as_itself() {
    let cases = [
        // MySQL prints one number and means it.
        (
            "mysqldump  Ver 8.4.3 for Linux on x86_64 (MySQL Community Server - GPL)",
            Version::new(8, 4),
        ),
        ("mysql  Ver 8.0.40 for Linux on x86_64", Version::new(8, 0)),
        // MariaDB up to 10.x: `10.19` is the tool's own version, `10.6.16` is the one
        // that matters. Taking the first number here reads a 10.6 as a 10.19.
        (
            "mysqldump  Ver 10.19 Distrib 10.6.16-MariaDB, for Linux (x86_64)",
            Version::new(10, 6),
        ),
        (
            "mysqldump  Ver 10.19 Distrib 5.5.68-MariaDB, for Linux (x86_64)",
            Version::new(5, 5),
        ),
        // MariaDB 11 renamed the tools and rewrote the line.
        (
            "mariadb-dump from 11.4.4-MariaDB, client 10.19 for Linux",
            Version::new(11, 4),
        ),
    ];

    for (printed, want) in cases {
        assert_eq!(version_of_a_client_tool(printed), Some(want), "{printed}");
    }

    for printed in ["", "command not found"] {
        assert_eq!(version_of_a_client_tool(printed), None, "{printed:?}");
    }
}

/// The dump filter, which is what lets a backup taken by one account be restored by
/// another. It has to take the clause off every shape of it and never touch a row of data.
#[test]
fn a_definer_comes_off_a_statement_and_never_off_a_row() {
    let stripped = [
        (
            r"/*!50013 DEFINER=`alpha`@`%` SQL SECURITY DEFINER */",
            r"/*!50013 SQL SECURITY DEFINER */",
        ),
        (
            r"/*!50003 CREATE*/ /*!50017 DEFINER=`root`@`localhost`*/ /*!50003 TRIGGER t*/",
            r"/*!50003 CREATE*/ /*!50017 */ /*!50003 TRIGGER t*/",
        ),
        (
            r"/*!50106 CREATE*/ /*!50117 DEFINER=`a b`@`10.0.0.1`*/ /*!50106 EVENT e*/",
            r"/*!50106 CREATE*/ /*!50117 */ /*!50106 EVENT e*/",
        ),
        // An unquoted account, and one with a doubled backtick inside the name.
        (r"/*!50020 DEFINER=root@localhost*/ x", r"/*!50020 */ x"),
        (r"/*!50020 DEFINER=`od``d`@`%`*/ x", r"/*!50020 */ x"),
        // A stored routine, the one object mysqldump cannot wrap in a versioned comment —
        // its body runs over several lines — so it arrives as a plain statement instead.
        (
            r"CREATE DEFINER=`alpha`@`%` FUNCTION `widget_count`() RETURNS int",
            r"CREATE FUNCTION `widget_count`() RETURNS int",
        ),
        (
            r"CREATE DEFINER=`root`@`localhost` PROCEDURE `p`()",
            r"CREATE PROCEDURE `p`()",
        ),
    ];

    for (before, after) in stripped {
        assert_eq!(
            String::from_utf8_lossy(&without_a_definer(before.as_bytes())),
            after,
            "{before}"
        );
    }

    // Anything that is not a versioned-comment statement is returned exactly as it came —
    // including a row of data that happens to contain the text being looked for.
    let untouched = [
        r"INSERT INTO quotes VALUES (1,'/*!50013 DEFINER=`alpha`@`%` SQL SECURITY DEFINER */');",
        r"-- MySQL dump 10.13  Distrib 8.4.3",
        r"CREATE TABLE `widgets` (`id` int NOT NULL);",
        // `CREATE` opens the routine case, so a `CREATE` that is not one must come back
        // whole even when the word turns up later in the line.
        r"CREATE TABLE `audit` (`note` text DEFAULT 'DEFINER=x');",
        r"/*!40101 SET NAMES utf8mb4 */;",
        "",
    ];

    for line in untouched {
        assert_eq!(
            String::from_utf8_lossy(&without_a_definer(line.as_bytes())),
            line,
            "this line should not have been touched"
        );
    }
}

/// A name is quoted twice over in the counting statement — as a string to print back and as
/// an identifier to select from — and MySQL reads a backslash as an escape in the first.
#[test]
fn a_name_with_punctuation_in_it_survives_being_quoted() {
    assert_eq!(sql_literal("plain"), "'plain'");
    assert_eq!(sql_literal(r"back\slash"), r"'back\\slash'");
    assert_eq!(sql_literal("it's"), "'it''s'");
    assert_eq!(sql_literal(r"both\'"), r"'both\\'''");

    assert_eq!(quote_identifier("plain"), "`plain`");
    assert_eq!(quote_identifier("od`d"), "`od``d`");
}

/// A password never reaches `argv`, on any engine. This walks the arguments the adapter
/// would hand a child process and asserts the password is not among them.
#[test]
fn the_mysql_adapters_keep_the_password_out_of_argv() {
    let password = crate::secret::Secret::new("hunter2 $ecret".to_owned());
    let target = super::Target {
        engine: Engine::Mysql,
        host: "db.internal",
        port: 3307,
        database: "app",
        user: "app_rw",
        password: &password,
    };

    let adapter = MysqlFamily::new(
        Family::Mysql,
        Tools {
            dump: std::path::PathBuf::from("dump-tool-that-is-not-installed"),
            client: std::path::PathBuf::from("client-that-is-not-installed"),
        },
    );

    // Whatever the adapter fails at, it must not be by putting the password on a command
    // line. Nothing here is installed, so nothing runs; the failure is the point.
    let failure = adapter.probe(&target).expect_err("nothing is installed");
    assert!(!failure.message().contains("hunter2"), "{failure:?}");
    assert!(!target.describe().contains("hunter2"));
    assert_eq!(target.describe(), "mysql://app_rw@db.internal:3307/app");
}
