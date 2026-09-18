//! What can be proved about the activity query without a database to run it against.
//!
//! The statement, and the local day boundary it is measured from. What needs a real server —
//! a database under load reading higher than an idle one, and a period with no readings coming
//! back as none — is `service::cluster_tests`.

use super::{Activity, Window, local_midnight, query};
use crate::backup::stamp::Stamp;

/// **`R27`'s `Done when`, as a type.** An idle period and an unwatched one are different facts
/// and nothing may draw them the same: zero rows with readings behind them is silence, zero
/// rows with none is nobody looking.
#[test]
fn a_window_with_no_readings_is_not_a_window_with_no_traffic() {
    let quiet = Window {
        rows_in: 0,
        rows_out: 0,
        readings: 60,
    };
    let unwatched = Window {
        rows_in: 0,
        rows_out: 0,
        readings: 0,
    };

    assert!(quiet.watched(), "an idle hour was read as an unwatched one");
    assert!(!unwatched.watched());
    assert_ne!(quiet, unwatched);
}

/// A database with nothing recorded anywhere says so, rather than showing three zeroes.
#[test]
fn a_database_nothing_has_been_recorded_about_says_so() {
    let nothing = Activity {
        label: String::from("orders"),
        today: Window::default(),
        week: Window::default(),
        month: Window::default(),
        size_bytes: None,
        size_at: None,
        attached_at: 1_789_000_000,
        seen_at: None,
    };
    assert!(!nothing.ever_watched());

    // A size on its own is enough to have been watched: the engine answered, even if no rows
    // moved in thirty days.
    let sized = Activity {
        size_bytes: Some(4_096),
        size_at: Some(1_789_000_000),
        ..nothing.clone()
    };
    assert!(sized.ever_watched());

    // And so is a month with readings in it but no traffic.
    let idle = Activity {
        month: Window {
            rows_in: 0,
            rows_out: 0,
            readings: 1_440,
        },
        ..nothing
    };
    assert!(idle.ever_watched());
}

/// **Local midnight, not UTC midnight.** The hours are stored in UTC because UTC sorts; "today"
/// means the day the person reading it is having, and getting that wrong shows yesterday's
/// traffic as today's for everybody east or west of Greenwich.
#[test]
fn today_begins_at_the_readers_own_midnight() {
    let now = Stamp::now();
    let midnight = local_midnight(now);
    let offset = i64::from(now.local().offset_seconds());

    assert!(midnight <= now, "midnight is in the future");
    assert!(
        now.unix_seconds() - midnight.unix_seconds() < 86_400,
        "midnight is more than a day ago"
    );

    // It lands exactly on a local day boundary: shifted by the offset, it is a whole number
    // of days since the epoch.
    assert_eq!(
        (midnight.unix_seconds() + offset).rem_euclid(86_400),
        0,
        "midnight is not on a local day boundary"
    );

    // And it reads as midnight on the clock, which is the assertion somebody would make by
    // eye — the two agree only if the arithmetic and the rendering share a calendar.
    assert!(
        midnight.local().readable().contains(" 00:00:00 "),
        "{}",
        midnight.local().readable()
    );
}

/// The three windows are three ranges of one table, and each is measured from its own instant.
#[test]
fn the_three_windows_are_three_ranges_of_the_one_table() {
    let now = Stamp::from_unix_seconds(1_789_000_000);
    let midnight = local_midnight(now);
    let sql = query(now, midnight);

    for name in ["'today'", "'week'", "'month'"] {
        assert!(sql.contains(name), "{name} is not in the query: {sql}");
    }
    assert_eq!(
        sql.matches("FROM activity_hour a").count(),
        5,
        "the windows and the size do not all read the one table: {sql}"
    );

    // A week is seven days back and a month is thirty, from now rather than from midnight —
    // a rolling window, so "30 days" never means "one day" on the first of the month.
    assert!(
        sql.contains(&format!(
            "to_timestamp({})",
            now.unix_seconds() - 7 * 86_400
        )),
        "{sql}"
    );
    assert!(
        sql.contains(&format!(
            "to_timestamp({})",
            now.unix_seconds() - 30 * 86_400
        )),
        "{sql}"
    );
    assert!(
        sql.contains(&format!("to_timestamp({})", midnight.unix_seconds())),
        "today is not measured from local midnight: {sql}"
    );
}

/// Only what is attached, and only the global registry — the same predicate `R25` and `R26`
/// read with. There is no user-supplied text in this statement at all, which is why it takes
/// no label: the windows are instants and the rest is fixed.
#[test]
fn only_attached_global_databases_are_shown() {
    let now = Stamp::now();
    let sql = query(now, local_midnight(now));

    assert!(sql.contains("s.name = 'sloop'"), "{sql}");
    assert!(sql.contains("m.enabled"), "{sql}");
    assert_eq!(
        sql.matches('\'').count() % 2,
        0,
        "an unbalanced quote: {sql}"
    );
}

/// The size is the newest one recorded, not a sum — it is a level, and adding up a database's
/// size at each reading would produce a number with no meaning at all.
#[test]
fn the_size_is_the_newest_reading_rather_than_a_total() {
    let now = Stamp::now();
    let sql = query(now, local_midnight(now));

    assert!(
        sql.contains("ORDER BY a.hour DESC LIMIT 1"),
        "the size is not the newest reading: {sql}"
    );
    assert!(
        !sql.contains("sum(a.size_bytes)"),
        "the size is being added up: {sql}"
    );
    // And an hour whose reading said nothing about the size is skipped rather than read as a
    // database that shrank to nothing.
    assert!(sql.contains("a.size_bytes IS NOT NULL"), "{sql}");
}

/// A count somebody can read at a glance, at every size and on both sides of zero.
#[test]
fn a_count_is_grouped_so_it_can_be_read_at_a_glance() {
    use crate::commands::activity::grouped;

    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(7), "7");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_000), "1,000");
    assert_eq!(grouped(1_250_000), "1,250,000");
    assert_eq!(grouped(10_350_000), "10,350,000");
    assert_eq!(grouped(i64::MAX), "9,223,372,036,854,775,807");

    // Nothing recorded can be negative — the columns have CHECKs — but a number that arrived
    // wrong should read as wrong rather than as an enormous positive one.
    assert_eq!(grouped(-1_250), "-1,250");
    assert_eq!(grouped(i64::MIN), "-9,223,372,036,854,775,808");
}
