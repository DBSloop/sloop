//! The bytes sloop itself moved — `R27a`, and the one figure here that really is bytes.
//!
//! **Two different things, and both are honest.** `R26` established that per-database bytes on
//! the wire do not exist: `pg_stat_database` counts rows, `blks_read` is disk, MySQL's
//! `Bytes_sent` is server-global. That finding has not changed. What this adds is the half
//! `R26` could not have, because sloop was not yet the thing doing the moving:
//!
//! ```text
//! a dump being read      bytes in   -- exact, because sloop read them
//! a restore being written bytes out -- exact, because sloop wrote them
//! everything else        rows and size, from the server's own counters
//! ```
//!
//! **Every figure says which of the two it came from.** A number labelled "bytes" that was
//! really rows would be the one dishonest number in this tool, so `bandwidth_day` holds only
//! what sloop moved and `activity_hour` holds only what the server counted, and neither is ever
//! added to the other.
//!
//! **Recorded wherever the moving happens, not only on a schedule.** A backup somebody took by
//! hand moved exactly as many bytes as one the service took, and a screen that counted only the
//! scheduled ones would be answering a different question from the one it asks.

use crate::failure::Outcome;
use crate::registry::Scope;
use crate::registry::store::{Store, literal};

/// Add what one run moved to today's row.
///
/// **One row per database per UTC day, which is `0004`'s whole design** — a day, a week, a
/// month and a year are all `sum` over a range, with no rollup tables to keep in step.
///
/// A database the store has never heard of writes nothing rather than failing: a project
/// database is registered in its own scope and is as real as a global one, but a label that
/// matches nothing at all is a backup of something outside any registry, and there is no row
/// for it to hang off.
pub fn moved(
    store: &Store,
    scope: Scope,
    label: &str,
    bytes_in: i64,
    bytes_out: i64,
) -> Outcome<()> {
    if bytes_in == 0 && bytes_out == 0 {
        return Ok(());
    }

    let label = literal(label)?;
    // A project's databases and the global store's can share a label, so the scope has to be
    // part of the question — the same rule `registry::store::belongs_to` follows.
    let whose = match scope {
        Scope::Global => "d.project_id IS NULL",
        Scope::Project => "d.project_id IS NOT NULL",
    };

    store.run(&format!(
        "INSERT INTO bandwidth_day (registered_database_id, day, bytes_in, bytes_out)
         SELECT d.id, (now() AT TIME ZONE 'UTC')::date, {bytes_in}, {bytes_out}
           FROM registered_database d
          WHERE d.label = {label} AND {whose}
         ON CONFLICT ON CONSTRAINT one_row_per_database_per_day DO UPDATE
            SET bytes_in   = bandwidth_day.bytes_in  + EXCLUDED.bytes_in,
                bytes_out  = bandwidth_day.bytes_out + EXCLUDED.bytes_out,
                updated_at = now();"
    ))
}

/// What sloop moved for one database, over a day, a week, a month and a year.
///
/// One scan of one table, which is what keying it per day bought.
pub const MOVED_BY_DATABASE: &str = "SELECT coalesce(json_agg(json_build_object(
          'label', d.label,
          'in_today',  coalesce(b.in_today,  0),
          'out_today', coalesce(b.out_today, 0),
          'in_week',   coalesce(b.in_week,   0),
          'out_week',  coalesce(b.out_week,  0),
          'in_month',  coalesce(b.in_month,  0),
          'out_month', coalesce(b.out_month, 0)
        ) ORDER BY d.label), '[]')
   FROM registered_database d
   JOIN (
     SELECT registered_database_id,
            sum(bytes_in)  FILTER (WHERE day  = (now() AT TIME ZONE 'UTC')::date)   AS in_today,
            sum(bytes_out) FILTER (WHERE day  = (now() AT TIME ZONE 'UTC')::date)   AS out_today,
            sum(bytes_in)  FILTER (WHERE day >  (now() AT TIME ZONE 'UTC')::date - 7)  AS in_week,
            sum(bytes_out) FILTER (WHERE day >  (now() AT TIME ZONE 'UTC')::date - 7)  AS out_week,
            sum(bytes_in)  FILTER (WHERE day >  (now() AT TIME ZONE 'UTC')::date - 30) AS in_month,
            sum(bytes_out) FILTER (WHERE day >  (now() AT TIME ZONE 'UTC')::date - 30) AS out_month
       FROM bandwidth_day
      GROUP BY registered_database_id
   ) AS b ON b.registered_database_id = d.id;";
