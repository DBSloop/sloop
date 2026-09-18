//! What the service has recorded, read back out — `R27`.
//!
//! **Called *activity* and not *bandwidth*, because that is what the numbers are.** Rows the
//! server counted, and a size on disk. Per-database bytes on the wire do not exist —
//! `pg_stat_database` counts rows, `blks_read` is disk, MySQL's `Bytes_sent` is server-global —
//! and a figure labelled "bytes" that was really rows would be the one dishonest number in this
//! tool. Every line below says which it is. See *"Bandwidth is rows, because bytes do not exist
//! per database"* in `docs/OWNER-DECISIONS.md`.
//!
//! **A period with no readings in it is shown as having none, never as a zero.** That is
//! `R27`'s own *Done when*, and it is the reason every window carries its count of readings as
//! well as its totals: *nothing moved* and *nobody was watching* are different facts, and a
//! monitoring screen that drew them the same way would be worse than no screen.
//!
//! **Local time throughout, and the day boundary is local too.** The hours are stored in UTC
//! because UTC sorts and survives a clock going back; "today" means the day the person reading
//! it is having. The cut-off is worked out here and handed to the query as an instant, so there
//! is one calendar in this project and it is [`crate::backup::stamp`].

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;

use serde::Deserialize;

use crate::backup::stamp::Stamp;
use crate::failure::Outcome;
use crate::registry::store::Store;

use super::{traffic, watch};

/// Seconds in a day, for the local midnight the windows are measured from.
const DAY: i64 = 86_400;

/// One window of time, and what was recorded in it.
///
/// **The count of readings is not decoration.** It is what tells a quiet period from an
/// unwatched one, which is the whole of `R27`'s *Done when*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct Window {
    /// Rows written into the database over this window.
    pub rows_in: i64,
    /// Rows read out of it.
    pub rows_out: i64,
    /// How many readings went into those two numbers. Zero means nobody was watching.
    pub readings: i64,
}

impl Window {
    /// Was anything recorded at all in this window?
    #[must_use]
    pub const fn watched(&self) -> bool {
        self.readings > 0
    }
}

/// One attached database, across the three windows.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Activity {
    /// What it is registered as.
    pub label: String,
    /// Since local midnight.
    pub today: Window,
    /// The last seven days.
    pub week: Window,
    /// The last thirty days.
    pub month: Window,
    /// How big it was at the most recent reading that said. `None` where none has.
    pub size_bytes: Option<i64>,
    /// When that size was read, as seconds since the epoch.
    pub size_at: Option<i64>,
    /// When it was attached.
    pub attached_at: i64,
    /// When a running service last read the attachment, if one ever has.
    pub seen_at: Option<i64>,
}

impl Activity {
    /// Has anything ever been recorded about this database?
    #[must_use]
    pub const fn ever_watched(&self) -> bool {
        self.month.watched() || self.size_bytes.is_some()
    }

    /// The moment the size was read, as a stamp.
    #[must_use]
    pub fn size_taken_at(&self) -> Option<Stamp> {
        self.size_at.map(Stamp::from_unix_seconds)
    }
}

/// What sloop itself moved for one database — `R27a`, and really bytes.
///
/// **Kept apart from [`Activity`] on purpose.** These are exact, because sloop read or wrote
/// them; those are rows the server counted, because per-database bytes on the wire do not
/// exist. Adding the two together would produce the one dishonest number in this tool, and two
/// types is the cheapest way to make that impossible rather than merely discouraged.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Moved {
    /// What it is registered as.
    pub label: String,
    /// Dumps read today, in bytes.
    pub in_today: i64,
    /// Restores written today.
    pub out_today: i64,
    /// The last seven days.
    pub in_week: i64,
    /// The same.
    pub out_week: i64,
    /// The last thirty.
    pub in_month: i64,
    /// The same.
    pub out_month: i64,
}

impl Moved {
    /// Has sloop moved anything at all for this database?
    #[must_use]
    pub const fn anything(&self) -> bool {
        self.in_month > 0 || self.out_month > 0
    }
}

/// Everything the activity screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    /// The attached databases, in label order.
    pub databases: Vec<Activity>,
    /// Databases that are no longer attached but still have hours recorded against them.
    ///
    /// **Named rather than dropped**, because `R25` promises detaching keeps the history and a
    /// screen that showed none of it would make that promise look false.
    pub detached_with_history: Vec<String>,
    /// When the daemon last read the attachment list, if it ever has.
    pub last_seen: Option<Stamp>,
    /// What sloop itself moved, per database — real bytes, and never mixed with the rows.
    pub moved: Vec<Moved>,
}

impl Recorded {
    /// What sloop moved for one database.
    #[must_use]
    pub fn moved_for(&self, label: &str) -> Option<&Moved> {
        self.moved.iter().find(|one| one.label == label)
    }
}

/// Read it all back.
pub fn read(store: &Store) -> Outcome<Recorded> {
    let now = Stamp::now();
    let midnight = local_midnight(now);

    let databases: Vec<Activity> = store.json(&query(now, midnight))?;
    let detached_with_history: Vec<String> = store.json(STRANDED)?;

    Ok(Recorded {
        databases,
        detached_with_history,
        last_seen: watch::last_seen(store)?,
        moved: store.json(traffic::MOVED_BY_DATABASE)?,
    })
}

/// The instant the reader's own day began, in UTC.
///
/// **The offset is looked up at `now` rather than assumed**, which is what makes this right on
/// the day a clock goes back: `Stamp::local` asks the operating system about the moment in
/// question, and the whole of the local rendering in this project goes through it.
fn local_midnight(now: Stamp) -> Stamp {
    let offset = i64::from(now.local().offset_seconds());

    // Floor division, not truncating: a negative second count belongs to the day before, and
    // `-1 / 86_400` is `0` in Rust. The same arithmetic `Stamp::parts` does, for the same
    // reason.
    let local_days = (now.unix_seconds() + offset).div_euclid(DAY);
    Stamp::from_unix_seconds(local_days * DAY - offset)
}

/// Three windows and a size, per attached database.
///
/// **Correlated subqueries rather than three joins**, because each window is a different range
/// of the same table and a reader should be able to see which is which. One row per database,
/// whatever it has in it — a database with nothing recorded still appears, with zeroes that the
/// `readings` counts say are silence rather than stillness.
fn query(now: Stamp, midnight: Stamp) -> String {
    let window = |since: i64, name: &str| {
        format!(
            "'{name}', (SELECT json_build_object(
                          'rows_in',  coalesce(sum(a.rows_in),  0),
                          'rows_out', coalesce(sum(a.rows_out), 0),
                          'readings', coalesce(sum(a.readings), 0))
                         FROM activity_hour a
                        WHERE a.registered_database_id = d.id
                          AND a.hour >= to_timestamp({since}))"
        )
    };

    format!(
        "SELECT coalesce(json_agg(json_build_object(
                  'label', d.label,
                  {today},
                  {week},
                  {month},
                  'size_bytes', (SELECT a.size_bytes FROM activity_hour a
                                  WHERE a.registered_database_id = d.id
                                    AND a.size_bytes IS NOT NULL
                                  ORDER BY a.hour DESC LIMIT 1),
                  'size_at', (SELECT floor(extract(epoch FROM a.hour))::bigint FROM activity_hour a
                               WHERE a.registered_database_id = d.id
                                 AND a.size_bytes IS NOT NULL
                               ORDER BY a.hour DESC LIMIT 1),
                  'attached_at', floor(extract(epoch FROM m.attached_at))::bigint,
                  'seen_at', floor(extract(epoch FROM m.seen_at))::bigint
                ) ORDER BY d.label), '[]')
           FROM monitored_database m, service s, registered_database d
          WHERE m.service_id = s.id AND s.name = 'sloop' AND m.enabled
            AND m.registered_database_id = d.id;",
        today = window(midnight.unix_seconds(), "today"),
        week = window(now.unix_seconds() - 7 * DAY, "week"),
        month = window(now.unix_seconds() - 30 * DAY, "month"),
    )
}

/// Databases with hours recorded against them that nothing is watching any more.
///
/// `R25` keeps the history when a database is detached, so it is still there — and a screen
/// that listed only what is attached would quietly imply it had gone.
const STRANDED: &str = "SELECT coalesce(json_agg(DISTINCT d.label), '[]')
                          FROM activity_hour a
                          JOIN registered_database d ON d.id = a.registered_database_id
                         WHERE NOT EXISTS (
                               SELECT 1 FROM monitored_database m, service s
                                WHERE m.service_id = s.id AND s.name = 'sloop'
                                  AND m.enabled
                                  AND m.registered_database_id = d.id);";
