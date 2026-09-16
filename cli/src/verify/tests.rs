//! The rules, without a database: which difference is drift, which is a failure, and what
//! an estimate is allowed to decide.
//!
//! The parts that need two real databases are in `cluster_tests.rs`.

use super::{Comparison, Finding, Mode, Side};
use crate::engine::{Engine, Table, TableCount};
use crate::exit::Exit;

fn counts(engine: Engine, database: &str, tables: &[(&str, &str, u64)]) -> Side {
    Side {
        engine,
        database: database.to_owned(),
        counts: tables
            .iter()
            .map(|&(schema, name, rows)| TableCount {
                table: Table {
                    schema: schema.to_owned(),
                    name: name.to_owned(),
                },
                rows,
            })
            .collect(),
    }
}

fn postgres(database: &str, tables: &[(&str, &str, u64)]) -> Side {
    counts(Engine::Postgres, database, tables)
}

fn finding(comparison: &Comparison, table: &str) -> Finding {
    comparison
        .rows
        .iter()
        .find(|row| row.table == table)
        .unwrap_or_else(|| panic!("{table} is not in the comparison: {:?}", comparison.rows))
        .finding
}

/// `SLOOP_VERIFY=fast` and nothing else. An unknown value reads as the answer that is true,
/// because a variable named this generally will eventually be set by something else.
#[test]
fn only_the_one_value_asks_for_estimates() {
    assert_eq!(Mode::from_value(Some("fast")), Mode::Fast);
    assert_eq!(Mode::from_value(Some("FAST")), Mode::Fast);
    assert_eq!(Mode::from_value(Some(" fast ")), Mode::Fast);

    for other in [
        Some("exact"),
        Some("1"),
        Some("true"),
        Some("faster"),
        Some(""),
        None,
    ] {
        assert_eq!(Mode::from_value(other), Mode::Exact, "{other:?}");
    }

    assert!(Mode::Fast.is_estimate());
    assert!(!Mode::Exact.is_estimate());
    assert!(Mode::Fast.describe().contains("not proof"));
}

/// The whole of R10's "Done when", as a table.
#[test]
fn drift_is_described_and_a_table_that_did_not_arrive_is_a_failure() {
    let source = postgres(
        "shop",
        &[
            ("app", "widgets", 500),
            ("app", "orders", 40),
            ("public", "notes", 7),
            ("public", "legacy", 5),
            ("public", "empty_on_both", 0),
        ],
    );
    let destination = postgres(
        "shop_copy",
        &[
            // Landed exactly.
            ("app", "widgets", 500),
            // Rows were inserted into the source while the dump ran.
            ("app", "orders", 42),
            // Nothing arrived, although the table itself is there.
            ("public", "notes", 0),
            // The table is not there at all.
            ("public", "empty_on_both", 0),
            // Something the source has never had.
            ("public", "extra", 3),
        ],
    );

    let comparison = Comparison::of(Mode::Exact, &source, &destination);

    assert_eq!(
        finding(&comparison, "app.widgets"),
        Finding::Matched { rows: 500 }
    );
    assert_eq!(
        finding(&comparison, "app.orders"),
        Finding::Drifted {
            source: 40,
            destination: 42
        }
    );
    assert_eq!(
        finding(&comparison, "public.notes"),
        Finding::Empty { source: 7 }
    );
    assert_eq!(
        finding(&comparison, "public.legacy"),
        Finding::Missing { source: 5 }
    );
    assert_eq!(
        finding(&comparison, "public.extra"),
        Finding::OnlyInDestination { rows: 3 }
    );
    // Empty on both sides is a match, not a failure: nothing was supposed to arrive.
    assert_eq!(
        finding(&comparison, "public.empty_on_both"),
        Finding::Matched { rows: 0 }
    );

    // Drift does not fail the run. Nothing arriving does.
    assert!(!comparison.landed());
    assert_eq!(comparison.exit(), Exit::Mismatch);
    assert_eq!(comparison.exit().code(), 6);

    let failed: Vec<&str> = comparison
        .failures()
        .map(|row| row.table.as_str())
        .collect();
    assert_eq!(failed, ["public.legacy", "public.notes"]);

    let drifted: Vec<&str> = comparison.drifted().map(|row| row.table.as_str()).collect();
    assert_eq!(drifted, ["app.orders"]);
}

/// A copy that landed says so and exits 0, drift and all.
#[test]
fn a_live_source_that_moved_underneath_the_dump_still_passes() {
    let source = postgres("shop", &[("app", "widgets", 500), ("public", "notes", 7)]);
    let destination = postgres("copy", &[("app", "widgets", 498), ("public", "notes", 9)]);

    let comparison = Comparison::of(Mode::Exact, &source, &destination);

    assert!(comparison.landed(), "drift is not a failure");
    assert_eq!(comparison.exit(), Exit::Success);
    assert_eq!(comparison.drifted().count(), 2, "both directions are drift");

    // And it is said in those words rather than left to be inferred from two numbers.
    let said = comparison.describe().join("\n");
    assert!(said.contains("drift"), "{said}");
    assert!(said.contains("500 → 498"), "{said}");
    assert!(said.contains("2 drifted"), "{said}");
}

/// An estimate is a glance. It is labelled as one, and it is never allowed to call a
/// perfect copy a disaster — which is exactly what it would do on a restored database,
/// where every table's statistics read zero until something collects them.
#[test]
fn an_estimate_never_fails_a_run_on_a_number() {
    let source = postgres("shop", &[("app", "widgets", 500), ("public", "notes", 7)]);
    let restored_but_unanalysed =
        postgres("copy", &[("app", "widgets", 0), ("public", "notes", 0)]);

    let fast = Comparison::of(Mode::Fast, &source, &restored_but_unanalysed);
    assert!(
        fast.landed(),
        "an estimate of zero on a restored table failed the run"
    );
    assert_eq!(fast.exit(), Exit::Success);

    let said = fast.describe().join("\n");
    assert!(said.contains("estimates"), "{said}");
    assert!(said.contains("not a proof"), "{said}");

    // The same two databases, counted properly, are a failure — which is the difference
    // between the two modes and the reason the exact one is the default.
    let exact = Comparison::of(Mode::Exact, &source, &restored_but_unanalysed);
    assert!(!exact.landed());
    assert_eq!(exact.exit(), Exit::Mismatch);

    // A table that is genuinely absent is a fact from the catalogue rather than from a
    // statistic, so even a fast run calls it a failure.
    let missing = Comparison::of(
        Mode::Fast,
        &source,
        &postgres("copy", &[("app", "widgets", 500)]),
    );
    assert!(!missing.landed());
    assert_eq!(missing.exit(), Exit::Mismatch);
}

/// MySQL has no schemas: a database *is* the schema. Comparing the qualified names of
/// `shop.orders` and `shop_staging.orders` would report every table as missing, which is
/// the opposite of the truth.
#[test]
fn a_mysql_database_copied_under_another_name_still_matches_table_for_table() {
    let source = counts(
        Engine::Mysql,
        "shop",
        &[("shop", "orders", 12), ("shop", "items", 3)],
    );
    let destination = counts(
        Engine::Mysql,
        "shop_staging",
        &[("shop_staging", "orders", 12), ("shop_staging", "items", 3)],
    );

    let comparison = Comparison::of(Mode::Exact, &source, &destination);
    assert!(comparison.landed(), "{:?}", comparison.rows);
    assert_eq!(
        finding(&comparison, "orders"),
        Finding::Matched { rows: 12 }
    );
    assert_eq!(comparison.rows.len(), 2);

    // Between two engines that disagree about what a schema is, the bare name is the only
    // thing both sides mean the same way — and the report says so rather than leaving
    // somebody to wonder why `public.orders` is listed as `orders`.
    let across = Comparison::of(
        Mode::Exact,
        &postgres("shop", &[("public", "orders", 12)]),
        &counts(Engine::Mysql, "shop", &[("shop", "orders", 12)]),
    );
    assert!(across.landed(), "{:?}", across.rows);
    assert!(
        across.describe().join("\n").contains("matched by name"),
        "{:?}",
        across.describe()
    );
}

/// Two PostgreSQL databases keep their schemas, because there a schema is a real thing and
/// two tables of the same name in different schemas are two different tables.
#[test]
fn postgres_keeps_its_schemas_apart() {
    let source = postgres("shop", &[("app", "items", 1), ("public", "items", 2)]);
    let destination = postgres("copy", &[("app", "items", 1), ("public", "items", 2)]);

    let comparison = Comparison::of(Mode::Exact, &source, &destination);
    assert_eq!(comparison.rows.len(), 2, "{:?}", comparison.rows);
    assert!(comparison.landed());
    assert_eq!(
        finding(&comparison, "app.items"),
        Finding::Matched { rows: 1 }
    );
    assert_eq!(
        finding(&comparison, "public.items"),
        Finding::Matched { rows: 2 }
    );
}

/// Two empty databases are a verified copy, not an error and not an empty report.
#[test]
fn nothing_on_either_side_is_a_pass() {
    let comparison = Comparison::of(Mode::Exact, &postgres("a", &[]), &postgres("b", &[]));

    assert!(comparison.landed());
    assert!(comparison.rows.is_empty());
    assert!(
        comparison.describe().join("\n").contains("0 of 0 tables"),
        "{:?}",
        comparison.describe()
    );
}
