//! One reading per attached database, every round — `R26`.
//!
//! **What is measured is rows and size, because per-database bytes do not exist.** PostgreSQL
//! publishes no per-database network counter: `pg_stat_database` carries rows returned,
//! fetched, inserted, updated and deleted; `blks_read` is disk blocks rather than network; and
//! `pg_database_size` is growth. MySQL's `Bytes_sent` and `Bytes_received` are server-global
//! status variables, and per *account* through `performance_schema` — never per database. So
//! what is recorded is rows in, rows out and size over time, labelled as exactly that. See
//! *"Bandwidth is rows, because bytes do not exist per database"* in `docs/OWNER-DECISIONS.md`.
//!
//! **A sample is a snapshot; what is stored is the difference.** Those counters are cumulative
//! and they reset — `pg_stat_reset()`, a fresh `initdb`, a MySQL restart. So each reading is
//! compared with the one before, and the one before is kept in `monitored_database` rather than
//! in this process: `R26`'s *"a restart mid-day loses no completed rollup"* is that column, and
//! without it the first reading after a restart would be counted as if a database's whole
//! history had happened in one minute.
//!
//! ```text
//! no reading yet     record nothing, and remember where the counter is now
//! counter went back  the server reset it, so what it reads now is what has moved since
//! otherwise          now minus then
//! ```
//!
//! **The hour is the grain and everything else is a `sum` over it.** A day, a week and a month
//! come out of `activity_hour` by range, with no rollup tables to keep in step — which is
//! `R27a`'s rule about `bandwidth_day`, one grain finer because `R26`'s own *Done when* asks
//! for a day that is the sum of its hours.
//!
//! **Nothing here opens a port and nothing here links a client.** Every reading is a spawned
//! `psql` or `mysql`, exactly as every other thing this tool does to a database.

#[cfg(test)]
#[path = "sample_tests.rs"]
mod tests;

use std::path::Path;

use crate::commands;
use crate::engine::{Activity, Target};
use crate::failure::{Failure, Outcome};
use crate::registry::store::{Store, literal};
use crate::registry::{Registries, Scope};
use crate::ssh::tunnel::Tunnels;

/// What one round of sampling managed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Taken {
    /// The databases a reading was recorded for.
    pub read: Vec<String>,
    /// The ones that could not be read, and why — one line each, already fit to print.
    pub missed: Vec<String>,
    /// Databases that answered, with something a person could do about a figure that is
    /// missing — an engine that counts rows somewhere this role cannot read, most of all.
    pub notes: Vec<String>,
}

/// Take one reading of every attached database and record it.
///
/// **Never fails as a whole.** One database that will not answer is one line in the journal,
/// not a round that recorded nothing: a monitoring tool that stops monitoring everything
/// because one server is down has failed at the moment it was for.
pub fn round(
    store: &Store,
    registries: &Registries,
    tunnels: &Tunnels,
    global: &Path,
    attached: &[String],
) -> Taken {
    let mut taken = Taken::default();

    for label in attached {
        match one(store, registries, tunnels, global, label) {
            Ok(note) => {
                taken.read.push(label.clone());
                if let Some(note) = note {
                    taken.notes.push(format!("{label}: {note}"));
                }
            }
            Err(why) => taken.missed.push(format!("{label}: {}", why.message())),
        }
    }

    taken
}

/// One database: reach it, read it, record it.
fn one(
    store: &Store,
    registries: &Registries,
    tunnels: &Tunnels,
    global: &Path,
    label: &str,
) -> Outcome<Option<String>> {
    let database = registries
        .in_scope(Scope::Global)
        .and_then(|registry| registry.get(label))
        .ok_or_else(|| {
            Failure::usage(format!(
                "{label} is attached to the service but is not in the global registry"
            ))
            .hint(format!(
                "register it again, or stop watching it: `sloop service detach {label}`"
            ))
        })?
        .clone();

    // **`commands::open` rather than a second way in.** It resolves the password through
    // whichever of `R3`'s four routes the record names and opens the SSH forward when the
    // record describes one — and a daemon that reached databases differently from the CLI
    // would be a second implementation of the one thing most worth getting right.
    let (at, password) = commands::open(&database, Scope::Global, registries, tunnels, None)?;

    let adapter = commands::adapter_for(database.engine, global);
    let activity = adapter.activity(&Target {
        engine: database.engine,
        host: &at.host,
        port: at.port,
        database: &database.database,
        user: &database.user,
        password: &password,
    })?;

    store.run(&record(label, &activity)?)?;
    Ok(activity.note)
}

/// The statement that turns one reading into an hour's worth of difference.
///
/// **Built apart from being run**, which is this project's habit wherever the mistakes live in
/// the text: `service::unit` renders three platforms' definitions the same way, and a statement
/// that gets the reset case wrong is not visible in a type.
///
/// The numbers are `i64` and go in as themselves; the label is the only thing a user chose, and
/// it goes through [`literal`].
fn record(label: &str, activity: &Activity) -> Outcome<String> {
    let label = literal(label)?;

    // `NULL` where the engine would not say. Every arm below is written so that a NULL reading
    // adds nothing and disturbs no baseline, rather than being read as zero.
    let sql_number =
        |value: Option<i64>| value.map_or_else(|| "NULL".to_owned(), |n| n.to_string());
    let (now_in, now_out, size) = (
        sql_number(activity.rows_in),
        sql_number(activity.rows_out),
        sql_number(activity.size_bytes),
    );

    Ok(format!(
        "WITH watched AS (
             SELECT d.id AS database_id, m.id AS attachment_id,
                    m.counted_rows_in AS was_in, m.counted_rows_out AS was_out
               FROM monitored_database m, service s, registered_database d
              WHERE m.service_id = s.id AND s.name = 'sloop' AND m.enabled
                AND m.registered_database_id = d.id
                AND d.label = {label} AND d.project_id IS NULL
         ),
         moved AS (
             SELECT database_id, attachment_id,
                    -- No baseline yet: remember where the counter is and count nothing. The
                    -- alternative charges the whole history of a database to one minute.
                    -- (No apostrophes in here: a balance check over the quotes in this
                    -- statement is what proves a label cannot close its literal.)
                    CASE WHEN {now_in} IS NULL OR was_in IS NULL THEN 0
                         -- The counter went backwards, so the server reset it: everything it
                         -- reads now has moved since that reset, and all of it since our last
                         -- reading, which was before it.
                         WHEN {now_in} < was_in THEN {now_in}
                         ELSE {now_in} - was_in END AS rows_in,
                    CASE WHEN {now_out} IS NULL OR was_out IS NULL THEN 0
                         WHEN {now_out} < was_out THEN {now_out}
                         ELSE {now_out} - was_out END AS rows_out
               FROM watched
         ),
         put AS (
             INSERT INTO activity_hour
                    (registered_database_id, hour, rows_in, rows_out, size_bytes, readings)
             SELECT database_id,
                    date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC',
                    rows_in, rows_out, {size}, 1
               FROM moved
             ON CONFLICT ON CONSTRAINT one_row_per_database_per_hour DO UPDATE
                SET rows_in    = activity_hour.rows_in  + EXCLUDED.rows_in,
                    rows_out   = activity_hour.rows_out + EXCLUDED.rows_out,
                    -- A level, not a total: the newest reading wins, and a reading that said
                    -- nothing leaves the last one that did.
                    size_bytes = coalesce(EXCLUDED.size_bytes, activity_hour.size_bytes),
                    readings   = activity_hour.readings + 1,
                    updated_at = now()
          RETURNING registered_database_id
         )
         UPDATE monitored_database m
            SET counted_rows_in  = coalesce({now_in},  m.counted_rows_in),
                counted_rows_out = coalesce({now_out}, m.counted_rows_out),
                counted_at       = now()
           FROM moved
          WHERE m.id = moved.attachment_id
            AND EXISTS (SELECT 1 FROM put);"
    ))
}
