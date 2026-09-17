//! What can be proved about the schema without a PostgreSQL to run it against.
//!
//! The migrations' *shape* — numbered from one, never edited after shipping, one documented
//! query per table, no column that could hold a password or a row of somebody's data. What
//! needs a running server is `server::cluster_tests`, which migrates a real database twice
//! and runs every documented query against it.

use super::{MIGRATIONS, Migration, READINGS};

/// Every table name the migrations create, in the order they create them.
///
/// A scan rather than a parser, and the assertion below is what keeps the scan honest: every
/// `CREATE TABLE` in these files is written on one line, ending in `(`, so the name is the
/// word before it.
fn tables_created() -> Vec<String> {
    let mut found = Vec::new();

    for migration in MIGRATIONS {
        for line in migration.sql.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("CREATE TABLE ") else {
                continue;
            };
            let rest = rest.strip_prefix("IF NOT EXISTS ").unwrap_or(rest);
            let name = rest
                .split(|character: char| character.is_whitespace() || character == '(')
                .next()
                .unwrap_or_default();

            assert!(
                !name.is_empty(),
                "migration {} has a CREATE TABLE this scan cannot read: {line}",
                migration.version
            );
            found.push(name.to_owned());
        }
    }

    found
}

/// Every type these migrations declare a column with.
///
/// **A closed set, and that is what makes the scan below reliable.** A column line is a
/// lowercase identifier followed by one of these words; anything else inside a `CREATE TABLE`
/// is a constraint, a comment, or the continuation of a multi-line `CHECK` — all of which can
/// also begin with a lowercase identifier, which is why the type rather than the name is what
/// the scan keys on. A migration that reaches for a type not on this list makes
/// [`the_column_scan_actually_reads_the_migrations`] fail rather than quietly shrinking what
/// the two rule tests below look at.
const TYPES: [&str; 9] = [
    "BIGSERIAL",
    "BIGINT",
    "INTEGER",
    "TEXT",
    "BOOLEAN",
    "DATE",
    "TIMESTAMPTZ",
    "DOUBLE",
    "BYTEA",
];

/// Every column declared by the migrations, as `(table, column)`.
///
/// **Both ways a column arrives**: inside a `CREATE TABLE`, and on an `ALTER TABLE … ADD
/// COLUMN` written on one line. The second is not a nicety — the two rule tests below are
/// the schema's guard against a credential or a row of somebody's data creeping in, and a
/// scan that read only `CREATE TABLE` would stop guarding the moment a migration added a
/// column to a table that already existed. `0006` is the first one that does.
fn columns_declared() -> Vec<(String, String)> {
    let mut found = Vec::new();

    for migration in MIGRATIONS {
        let mut table: Option<String> = None;

        for line in migration.sql.lines() {
            let line = line.trim();

            if let Some(rest) = line.strip_prefix("ALTER TABLE ") {
                let mut words = rest.split_whitespace();
                let (Some(altered), Some("ADD"), Some("COLUMN"), Some(name), Some(kind)) = (
                    words.next(),
                    words.next(),
                    words.next(),
                    words.next(),
                    words.next(),
                ) else {
                    continue;
                };
                assert!(
                    TYPES.contains(&kind.trim_end_matches(&[',', ';'][..])),
                    "migration {} adds {altered}.{name} with a type this scan does not know: \
                     {kind}",
                    migration.version
                );
                found.push((altered.to_owned(), name.to_owned()));
                continue;
            }

            if let Some(rest) = line.strip_prefix("CREATE TABLE ") {
                let rest = rest.strip_prefix("IF NOT EXISTS ").unwrap_or(rest);
                table = rest
                    .split(|character: char| character.is_whitespace() || character == '(')
                    .next()
                    .map(str::to_owned);
                continue;
            }

            // `);` and nothing else ends a table. **Not a bare `)`** — a multi-line `CHECK`
            // closes with one of those, and treating it as the end of the table was a defect
            // that silently stopped four tables being checked at all.
            if line.starts_with(");") {
                table = None;
                continue;
            }

            let Some(inside) = table.as_ref() else {
                continue;
            };

            let mut words = line.split_whitespace();
            let (Some(name), Some(kind)) = (words.next(), words.next()) else {
                continue;
            };
            // The comma comes off: a column with no constraint after its type ends the line
            // with one, and `TIMESTAMPTZ,` is not `TIMESTAMPTZ`.
            if !name
                .chars()
                .all(|character| character.is_ascii_lowercase() || character == '_')
                || !TYPES.contains(&kind.trim_end_matches(','))
            {
                continue;
            }

            found.push((inside.clone(), name.to_owned()));
        }
    }

    found
}

/// Dense from one, so "a release older" is a number and not a guess.
#[test]
fn the_migrations_are_numbered_from_one() {
    assert!(!MIGRATIONS.is_empty(), "there has to be a schema");

    for (index, migration) in MIGRATIONS.iter().enumerate() {
        let expected = i64::try_from(index).expect("four migrations fit in an i64") + 1;
        assert_eq!(
            migration.version, expected,
            "migration {} is out of order or there is a gap before it",
            migration.name
        );
        assert!(
            !migration.name.is_empty() && !migration.sql.trim().is_empty(),
            "migration {expected} has no name or no SQL"
        );
    }
}

/// The checksum is what catches a migration edited after it shipped, so it has to be a real
/// hash of the real file and it has to differ between files.
#[test]
fn every_migration_hashes_to_its_own_sql() {
    let mut seen: Vec<String> = Vec::new();

    for migration in MIGRATIONS {
        let checksum = migration.checksum();
        assert_eq!(checksum.len(), 64, "{} is not a SHA-256", migration.name);
        assert!(
            checksum
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "{} is not lowercase hex: {checksum}",
            migration.name
        );
        assert!(
            !seen.contains(&checksum),
            "two migrations hash the same, which means one of them is the other"
        );
        seen.push(checksum);
    }

    // And the hash is of the SQL, not of the name: a changed file changes it.
    let edited = Migration {
        version: 1,
        name: MIGRATIONS[0].name,
        sql: "-- one more comment\n",
    };
    assert_ne!(edited.checksum(), MIGRATIONS[0].checksum());
}

/// The ledger comes first, and it is the only migration that may already be there.
#[test]
fn only_the_ledger_is_created_if_not_exists() {
    assert_eq!(MIGRATIONS[0].name, "ledger");
    assert!(
        MIGRATIONS[0]
            .sql
            .contains("CREATE TABLE IF NOT EXISTS schema_migration")
    );

    for migration in &MIGRATIONS[1..] {
        assert!(
            !migration.sql.contains("IF NOT EXISTS"),
            "migration {} skips a table it finds; every one after the ledger runs exactly \
             once, so finding one is a defect rather than a state to tolerate",
            migration.name
        );
    }
}

/// **The `Done when`, as a test.** Every table the migrations create has a documented query,
/// and there is no documented query for a table that does not exist.
#[test]
fn every_table_is_reachable_from_a_documented_query() {
    let created = tables_created();
    assert_eq!(
        created.len(),
        11,
        "the schema is eleven tables: {created:?}"
    );

    for table in &created {
        let reading = READINGS
            .iter()
            .find(|reading| reading.table == table)
            .unwrap_or_else(|| panic!("{table} has no documented query"));

        assert!(
            !reading.purpose.trim().is_empty(),
            "{table}'s query says nothing about what it answers"
        );
        assert!(
            reading.sql.contains(table.as_str()),
            "{table}'s documented query does not name it: {}",
            reading.sql
        );
        assert!(
            reading.sql.trim_end().ends_with(';'),
            "{table}'s documented query is not a whole statement"
        );
    }

    for reading in READINGS {
        assert!(
            created.iter().any(|table| table == reading.table),
            "{} is documented but no migration creates it",
            reading.table
        );
    }

    assert_eq!(
        READINGS.len(),
        created.len(),
        "one query per table, and no table twice"
    );
}

/// **Rule 3, in the schema.** A column that holds anything to do with a credential holds a
/// *route* and says so in its name — there is no `password` column anywhere in here, and a
/// later one cannot arrive quietly.
#[test]
fn no_column_could_hold_a_secret() {
    for (table, column) in columns_declared() {
        let suspect = ["password", "secret", "passphrase", "credential", "token"]
            .iter()
            .any(|word| column.contains(word));

        assert!(
            !suspect || column.ends_with("_route"),
            "{table}.{column} names a credential without being a route — a route is a \
             direction to go and ask, and it is the only thing this schema may hold"
        );
    }
}

/// **No user data, ever.** The tables are about databases, so nothing in them is named for
/// the contents of one.
#[test]
fn nothing_in_the_schema_is_named_for_somebody_elses_data() {
    for (table, column) in columns_declared() {
        for word in ["row_data", "value", "contents", "payload", "sample"] {
            assert!(
                !column.contains(word),
                "{table}.{column} sounds like it holds what was inside a user's database. \
                 This schema holds counts, sizes and addresses and nothing else."
            );
        }
    }
}

/// The routes the schema accepts are the four `secret::Route` accepts, spelled the way
/// `Route::as_field` spells them — the CHECK in SQL and the parser in Rust have to agree or
/// one of them is decorative.
#[test]
fn the_route_check_matches_the_routes_rust_writes() {
    // Every migration that declares a `_route` column, so a later one cannot add a
    // credential column with a looser CHECK than the first one has.
    let with_routes: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|migration| migration.sql.contains("_route"))
        .collect();
    assert_eq!(
        with_routes.len(),
        2,
        "the registry migration and the ssh one are the two that hold routes"
    );

    for migration in with_routes {
        for route in [
            crate::secret::Route::Keyring,
            crate::secret::Route::EncryptedFile,
        ] {
            assert!(
                migration.sql.contains(&format!("'{}'", route.as_field())),
                "{} is a route sloop writes and {} does not accept it",
                route.as_field(),
                migration.name
            );
        }

        // The two that carry a value are matched by pattern rather than by name, so the
        // CHECK is asserted to be there at all.
        assert!(
            migration.sql.contains(r"^\$\{[^$[:space:]]+\}$"),
            "{} accepts no ${{VARIABLE}} route",
            migration.name
        );
        assert!(
            migration.sql.contains("^command:[[:space:]]*[^[:space:]]"),
            "{} accepts no command: route",
            migration.name
        );
    }
}

/// A literal is a literal, including the one character that could end one early.
#[test]
fn a_quote_in_a_literal_is_doubled_and_never_escaped() {
    assert_eq!(super::literal("ledger"), "'ledger'");
    assert_eq!(super::literal("it's"), "'it''s'");
    // A backslash stays a backslash. `standard_conforming_strings` has been on by default
    // since 9.1 and `initdb` leaves it on, so nothing here is an escape character.
    assert_eq!(super::literal(r"back\slash"), r"'back\slash'");
}

/// A run that applied nothing says so, which is what Setup prints on a second run.
#[test]
fn a_run_that_applied_nothing_says_it_changed_nothing() {
    let nothing = super::Applied {
        from: 4,
        to: 4,
        ran: Vec::new(),
    };
    assert!(!nothing.changed_anything());

    let something = super::Applied {
        from: 3,
        to: 4,
        ran: vec!["service"],
    };
    assert!(something.changed_anything());
}

/// The `engine` table is filled from `Engine::ALL`, so every engine this build speaks has a
/// proper name to put in it.
#[test]
fn every_engine_has_a_name_for_a_person_and_a_name_for_a_url() {
    for engine in crate::engine::Engine::ALL {
        assert!(!engine.proper_name().is_empty());
        assert_ne!(
            engine.proper_name(),
            engine.scheme(),
            "{} would be written the same way in a URL and on a screen, which means one of \
             the two is wrong",
            engine.scheme()
        );
    }

    assert_eq!(crate::engine::Engine::Postgres.proper_name(), "PostgreSQL");
    assert_eq!(crate::engine::Engine::Mariadb.proper_name(), "MariaDB");
}

/// The two tests above scan the migration files, and a scan that found nothing would pass
/// them both while proving nothing at all. This is what says it found the schema.
#[test]
fn the_column_scan_actually_reads_the_migrations() {
    let columns = columns_declared();

    // Every table, and a plausible number of columns in total.
    let tables: std::collections::BTreeSet<&str> =
        columns.iter().map(|(table, _)| table.as_str()).collect();
    assert_eq!(tables.len(), 11, "the scan missed a table: {tables:?}");
    assert!(
        columns.len() > 70,
        "the scan found only {} columns, which is not this schema",
        columns.len()
    );

    // And it reads the columns it is supposed to read, including the one the rule-3 test is
    // about and the one the bandwidth test is about.
    for wanted in [
        ("registered_database", "password_route"),
        ("registered_database", "project_id"),
        ("backup_key", "private_key_route"),
        ("bandwidth_day", "bytes_in"),
        ("schema_migration", "checksum"),
        // Added by an `ALTER TABLE`, which is the half of the scan `0006` needed and
        // nothing before it exercised.
        ("registered_database", "ssh_host"),
        ("registered_database", "ssh_secret_route"),
    ] {
        assert!(
            columns
                .iter()
                .any(|(table, column)| (table.as_str(), column.as_str()) == wanted),
            "the scan did not find {}.{}",
            wanted.0,
            wanted.1
        );
    }
}
