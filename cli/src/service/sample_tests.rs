//! What can be proved about a reading without a database to take one from.
//!
//! The statement, and the three cases that decide what a cumulative counter means. What needs
//! a real server — a reading taken off `pg_stat_database`, an hour that is the sum of its
//! readings, a restart that loses none of it — is `service::cluster_tests` and
//! `engine::cluster_tests`.

use super::{Taken, record};
use crate::engine::Activity;

/// A reading with every number in it.
fn full() -> Activity {
    Activity {
        rows_in: Some(1_000),
        rows_out: Some(2_000),
        size_bytes: Some(3_000),
        note: None,
    }
}

/// **Rule 3's neighbour, and the one a user can reach.** A label is whatever somebody typed at
/// `db add`, and the only thing between it and a statement is `literal`.
#[test]
fn a_quote_in_a_label_cannot_close_the_literal() {
    let sql = record("'; DROP TABLE activity_hour; --", &full()).unwrap();

    assert!(
        sql.contains("'''; DROP TABLE activity_hour; --'"),
        "the label was not doubled: {sql}"
    );
    assert_eq!(
        sql.matches('\'').count() % 2,
        0,
        "an unbalanced quote: {sql}"
    );
}

/// A NUL is refused rather than stored short.
#[test]
fn a_zero_byte_in_a_label_is_refused() {
    let failure = record("orders\0evil", &full()).expect_err("a NUL is not storable");
    assert_eq!(failure.exit().code(), 2);
}

/// **The three cases a cumulative counter has, and each is a different sentence.**
///
/// No baseline means record nothing — charging a database's whole history to one minute is the
/// failure worth avoiding. A counter that went backwards was reset, so what it reads now is
/// what has moved since. Otherwise it is a subtraction.
#[test]
fn the_statement_covers_a_first_reading_a_reset_and_an_ordinary_one() {
    let sql = record("orders", &full()).unwrap();

    assert!(
        sql.contains("was_in IS NULL THEN 0"),
        "a first reading is not counted as nothing: {sql}"
    );
    assert!(
        sql.contains("< was_in THEN 1000"),
        "a counter that went backwards is not read as a reset: {sql}"
    );
    assert!(
        sql.contains("ELSE 1000 - was_in"),
        "an ordinary reading is not a subtraction: {sql}"
    );
}

/// **A reading the engine would not answer adds nothing and disturbs no baseline.** `None` is
/// not zero: a MariaDB with `performance_schema` off says nothing about rows, and recording
/// that as "no rows moved" would be inventing the one number this entry exists to be honest
/// about.
#[test]
fn a_reading_with_no_rows_in_it_records_nothing_and_keeps_the_baseline() {
    let quiet = Activity {
        rows_in: None,
        rows_out: None,
        size_bytes: Some(4_096),
        note: Some(String::from(
            "rows are not counted: this role cannot read performance_schema",
        )),
    };
    let sql = record("orders", &quiet).unwrap();

    assert!(
        sql.contains("CASE WHEN NULL IS NULL OR was_in IS NULL THEN 0"),
        "a missing reading is not treated as nothing moving: {sql}"
    );
    // The baseline is only replaced by a number that exists.
    assert!(
        sql.contains("counted_rows_in  = coalesce(NULL,  m.counted_rows_in)"),
        "a missing reading overwrote the baseline: {sql}"
    );
    // And the size it *did* give is still recorded.
    assert!(sql.contains("4096"), "{sql}");
}

/// The size is a level, so the newest reading wins — and a reading that gave none leaves the
/// last one that did, rather than blanking an hour that had a real figure in it.
#[test]
fn the_size_is_a_level_rather_than_a_total() {
    let sql = record("orders", &full()).unwrap();

    assert!(
        sql.contains("size_bytes = coalesce(EXCLUDED.size_bytes, activity_hour.size_bytes)"),
        "the size is added up, or blanked: {sql}"
    );
    // Rows are the opposite: they accumulate across the hour.
    assert!(
        sql.contains("rows_in    = activity_hour.rows_in  + EXCLUDED.rows_in"),
        "rows do not accumulate: {sql}"
    );
}

/// **The hour is UTC**, for the reason every path in this project is: it sorts, and it survives
/// a clock going back. Every display converts.
#[test]
fn the_hour_is_the_utc_hour() {
    let sql = record("orders", &full()).unwrap();

    assert!(
        sql.contains("date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'"),
        "the hour is bucketed in the server's local time: {sql}"
    );
}

/// Only what is attached, and only the global registry — the same predicate `R25` reads the
/// list with, because a sample of something nobody attached is a sample nobody asked for.
#[test]
fn only_an_attached_global_database_is_sampled() {
    let sql = record("orders", &full()).unwrap();

    assert!(sql.contains("s.name = 'sloop'"), "{sql}");
    assert!(sql.contains("m.enabled"), "{sql}");
    assert!(sql.contains("d.project_id IS NULL"), "{sql}");
}

/// A round reports what it read and what it could not, separately: one database being down is
/// not the same as a round that recorded nothing, and the daemon says different things about
/// them.
#[test]
fn a_round_keeps_what_it_read_apart_from_what_it_missed() {
    let taken = Taken {
        read: vec![String::from("orders")],
        missed: vec![String::from("analytics: connection refused")],
        notes: vec![String::from("orders: rows are not counted")],
    };

    assert_eq!(taken.read.len(), 1);
    assert_eq!(taken.missed.len(), 1);
    // A database that answered but could not say everything is neither read-and-fine nor
    // missed: it is a third thing, and the daemon says a different sentence about it.
    assert_eq!(taken.notes.len(), 1);
    assert_ne!(taken, Taken::default());
}

/// The clamp is a clamp and not a refusal — rule 0d. Somebody who asks for one second wants
/// readings as often as possible, and a round per second per database is a process per second.
#[test]
fn a_very_short_interval_is_clamped_rather_than_refused() {
    use crate::service::daemon::{INTERVAL_SECONDS, interval};

    assert_eq!(interval(0).as_secs(), 5);
    assert_eq!(interval(1).as_secs(), 5);
    assert_eq!(interval(5).as_secs(), 5);
    assert_eq!(interval(60).as_secs(), 60);
    assert_eq!(interval(3_600).as_secs(), 3_600);
    assert_eq!(interval(INTERVAL_SECONDS).as_secs(), 60);
}
