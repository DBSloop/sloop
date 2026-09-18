//! Which databases the service watches — `R25`.
//!
//! **The list is a table, not a file and not a flag.** `sloop service attach orders` writes a
//! row; the daemon reads every row on every round; so attaching reaches a service that is
//! already running without anything being restarted, reloaded or signalled. That is the whole
//! of `R25`'s *Done when*, and it falls out of the shape rather than being arranged: there is
//! nothing to reload because there is nothing cached.
//!
//! **The global registry, and only the global registry.** A service has no working directory
//! — systemd gives it `/`, the Service Control Manager gives it `C:\Windows\System32` — so
//! the walk up the tree that finds a project's `.sloop` finds nothing, every time. A project
//! database could be attached and never sampled, which is worse than being told plainly that
//! `attach` reads the global store.
//!
//! **Detaching deletes the attachment and nothing else.** History is `bandwidth_day`, keyed by
//! the *database* rather than by the attachment, so a detach cannot reach it — which is the
//! other half of the `Done when` and is a property of the schema rather than a rule this code
//! has to remember. `sloop backups prune` is what deletes history, when somebody asks for it
//! to be deleted.
//!
//! **What this does not write.** `service.installed` and `service.mechanism` stay untouched.
//! `R24` settled that the service manager is the only honest answer to *is it installed* —
//! a row saying installed while systemd has never heard of the unit is worse than no row —
//! and nothing here is in a better position to know. The one column this writes on that table
//! is `last_seen_at`, which is not a claim about the machine at all: it is the daemon saying
//! *I was here*, and only a daemon can say that.

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;

use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;

use crate::backup::stamp::Stamp;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::store::{Store, literal};
use crate::registry::{Registries, Resolution};
use crate::ssh::tunnel::Tunnels;

use super::unit::SERVICE_NAME;

/// One attached database, as `status` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The label it is registered under, in the global store.
    pub label: String,
    /// False once something pauses one. Nothing sets it yet — see [`attached`] for why the
    /// reader honours it anyway.
    pub enabled: bool,
    /// When somebody attached it.
    pub attached_at: Stamp,
    /// When a running service last read this attachment, or `None` if none ever has.
    pub seen_at: Option<Stamp>,
    /// How many days of activity are already recorded against this database.
    pub days_of_history: i64,
}

/// What `attach` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attached {
    /// It was not attached, and now is.
    Now,
    /// It was already attached, and the row was left exactly as it was.
    Already,
    /// `--dry-run`: it would have been attached, every check having been made first, and
    /// nothing was written. [`crate::report::would`] has already said so.
    Rehearsed,
}

/// What `detach` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detached {
    /// False when it was not attached in the first place.
    pub was_attached: bool,
    /// Days of activity left behind, which detaching does not touch.
    pub days_of_history: i64,
    /// `--dry-run`: the row is still there, and the count above is what it would have kept.
    pub rehearsed: bool,
}

/// Where one database stands with the service, before anything is changed.
#[derive(Debug, Deserialize)]
struct Standing {
    /// Is that label in the global registry at all?
    known: bool,
    /// Is it attached to the service?
    attached: bool,
    /// Rows in `bandwidth_day` for it.
    days: i64,
}

/// One attachment, as the query hands it back.
#[derive(Debug, Deserialize)]
struct RawAttachment {
    label: String,
    enabled: bool,
    /// Seconds since the epoch. The column is `TIMESTAMPTZ`; what crosses the wire is an
    /// integer, so no timezone rendering happens anywhere but in [`Stamp::local`] — which is
    /// the rule the whole project follows about what a human is shown.
    attached_at: i64,
    seen_at: Option<i64>,
    days: i64,
}

/// The predicate that picks the attachments of this machine's one service.
///
/// **One place, because it is written three times and getting it wrong is silent.** A round
/// that read a wider set than `status` shows would sample something nobody attached; a
/// narrower one would quietly stop sampling something somebody did.
///
/// A function rather than a constant so that the name comes from [`SERVICE_NAME`]: a rename
/// there has to reach every one of the three, and a literal here would be the one it missed.
///
/// `m.enabled` is in it even though nothing sets it false. The column is `0004`'s and it is
/// what a later *pause* would use — a reader that ignored it would have to be found and
/// changed on the day that arrives, and this is the reader.
fn attached() -> String {
    format!(
        "m.service_id = s.id AND s.name = {} AND m.enabled",
        service_name()
    )
}

/// Attach a database to the service.
///
/// Idempotent, the way `R24` made `start` and `stop` idempotent: attaching something already
/// attached is what a second run of a provisioning script does, and the caller wants the state
/// afterwards rather than a complaint about the state before.
pub fn attach(store: &Store, label: &str) -> Outcome<Attached> {
    let standing = standing(store, label)?;
    if !standing.known {
        return Err(unknown(label));
    }
    if standing.attached {
        return Ok(Attached::Already);
    }

    // **Here, and not a line earlier.** `--dry-run` reads everything and checks everything
    // and then writes nothing, so the unknown name above is still exit `2` on a rehearsal and
    // an attachment that already exists still says so.
    if crate::report::would(&format!("attach {label} to the service")) {
        return Ok(Attached::Rehearsed);
    }

    store.run(&attach_sql(label)?)?;
    Ok(Attached::Now)
}

/// Stop sampling a database, and leave everything already sampled where it is.
pub fn detach(store: &Store, label: &str) -> Outcome<Detached> {
    let standing = standing(store, label)?;
    if !standing.known {
        return Err(unknown(label));
    }
    if !standing.attached {
        return Ok(Detached {
            was_attached: false,
            days_of_history: standing.days,
            rehearsed: false,
        });
    }

    if crate::report::would(&format!(
        "stop the service watching {label}, keeping {} days of activity",
        standing.days
    )) {
        return Ok(Detached {
            was_attached: true,
            days_of_history: standing.days,
            rehearsed: true,
        });
    }

    store.run(&detach_sql(label)?)?;

    Ok(Detached {
        was_attached: true,
        days_of_history: standing.days,
        rehearsed: false,
    })
}

/// Everything attached to this machine's service, in label order.
pub fn attachments(store: &Store) -> Outcome<Vec<Attachment>> {
    let raw: Vec<RawAttachment> = store.json(&format!(
        "SELECT coalesce(json_agg(json_build_object(
                  'label',       d.label,
                  'enabled',     m.enabled,
                  'attached_at', extract(epoch FROM m.attached_at)::bigint,
                  'seen_at',     extract(epoch FROM m.seen_at)::bigint,
                  'days',        (SELECT count(*) FROM bandwidth_day b
                                   WHERE b.registered_database_id = d.id)
                ) ORDER BY d.label), '[]')
           FROM monitored_database m, service s, registered_database d
          WHERE {attached}
            AND m.registered_database_id = d.id;",
        attached = attached(),
    ))?;

    Ok(raw
        .into_iter()
        .map(|one| Attachment {
            label: one.label,
            enabled: one.enabled,
            attached_at: Stamp::from_unix_seconds(one.attached_at),
            seen_at: one.seen_at.map(Stamp::from_unix_seconds),
            days_of_history: one.days,
        })
        .collect())
}

/// When the daemon last said it was here, or `None` if it never has.
pub fn last_seen(store: &Store) -> Outcome<Option<Stamp>> {
    let said = store.ask(&format!(
        "SELECT coalesce(extract(epoch FROM last_seen_at)::bigint::text, '')
           FROM service WHERE name = {service};",
        service = service_name(),
    ))?;

    Ok(said.trim().parse().ok().map(Stamp::from_unix_seconds))
}

/// One turn of the daemon's loop: say it is here, and read the list.
///
/// **Reading and marking are one statement, so `seen_at` cannot claim more than happened.**
/// The `UPDATE … RETURNING` marks exactly the rows it returns — there is no window in which
/// the daemon has stamped an attachment it then failed to read, and no second predicate to
/// drift away from the first.
///
/// **One statement rather than three, and that is about `psql` rather than about SQL.** A
/// script prints a command tag per statement — `UPDATE 2` — and [`Store::ask`] hands back
/// everything the child said, so an `INSERT` before the `SELECT` would arrive in front of the
/// JSON and nothing would parse. Data-modifying `WITH` clauses run whether or not the outer
/// query reads them, which is exactly the shape this wants.
pub fn round(store: &Store) -> Outcome<Vec<String>> {
    store.json(&format!(
        "WITH here AS (
             INSERT INTO service (name, last_seen_at) VALUES ({service}, now())
             ON CONFLICT (name) DO UPDATE SET last_seen_at = now()
          RETURNING id
         ),
         picked_up AS (
             UPDATE monitored_database m
                SET seen_at = now()
               FROM service s
              WHERE {attached}
          RETURNING m.registered_database_id
         )
         SELECT coalesce(json_agg(d.label ORDER BY d.label), '[]')
           FROM picked_up, registered_database d
          WHERE d.id = picked_up.registered_database_id;",
        service = service_name(),
        attached = attached(),
    ))
}

/// Where one database stands, in one question.
fn standing(store: &Store, label: &str) -> Outcome<Standing> {
    store.json(&standing_sql(label)?)
}

/// **The three statements are built apart from being run**, which is the habit `service::unit`
/// already follows for the three platforms' definition files and for the same reason: the
/// mistakes live in the text, and the text can be checked on a machine with no PostgreSQL on
/// it at all. What they have in common is [`literal`] — a label is whatever somebody typed,
/// and a quote in it that closed the literal early would be an injection with a registry entry
/// as the payload.
fn standing_sql(label: &str) -> Outcome<String> {
    let label = literal(label)?;

    Ok(format!(
        "SELECT json_build_object(
                  'known',    EXISTS (SELECT 1 FROM registered_database d
                                       WHERE d.label = {label} AND d.project_id IS NULL),
                  'attached', EXISTS (SELECT 1 FROM monitored_database m, service s,
                                                    registered_database d
                                       WHERE {attached}
                                         AND m.registered_database_id = d.id
                                         AND d.label = {label} AND d.project_id IS NULL),
                  'days',     (SELECT count(*) FROM bandwidth_day b
                                 JOIN registered_database d ON d.id = b.registered_database_id
                                WHERE d.label = {label} AND d.project_id IS NULL)
                );",
        attached = attached(),
    ))
}

/// The row, made against this machine's one service.
fn attach_sql(label: &str) -> Outcome<String> {
    Ok(format!(
        "{ensure}
         INSERT INTO monitored_database (service_id, registered_database_id)
         SELECT s.id, d.id
           FROM service s, registered_database d
          WHERE s.name = {service}
            AND d.label = {label} AND d.project_id IS NULL
         ON CONFLICT ON CONSTRAINT one_attachment_per_database DO NOTHING;",
        ensure = ensure_service(),
        service = service_name(),
        label = literal(label)?,
    ))
}

/// The row, taken away again.
///
/// **`USING`, so the delete is scoped by the same join [`attached`] describes.** A `DELETE` keyed
/// on the label alone would reach a second service's attachment on the day there is one, and
/// the table is keyed by name precisely so that there could be.
///
/// **It names `monitored_database` and nothing else**, which is what leaves the history alone.
/// `bandwidth_day` hangs off the *database*, not off the attachment, so there is no cascade
/// from here to it and no rule this function has to remember.
fn detach_sql(label: &str) -> Outcome<String> {
    Ok(format!(
        "DELETE FROM monitored_database m
           USING service s, registered_database d
          WHERE m.service_id = s.id AND s.name = {service}
            AND m.registered_database_id = d.id
            AND d.label = {label} AND d.project_id IS NULL;",
        service = service_name(),
        label = literal(label)?,
    ))
}

/// The row every attachment points at, made if this machine has not made it yet.
///
/// **Not a claim that anything is installed.** It carries `0004`'s defaults — `installed`
/// false, `mechanism` NULL — which the table's own CHECK allows and which nothing reads. What
/// it is for is a foreign key: an attachment has to hang off something, and that something is
/// this machine's one service.
fn ensure_service() -> String {
    let mut sql = String::new();
    let _ = write!(
        sql,
        "INSERT INTO service (name) VALUES ({service})
         ON CONFLICT (name) DO NOTHING;",
        service = service_name(),
    );
    sql
}

/// The service's name as a literal. Infallible — [`SERVICE_NAME`] is a constant with no quote
/// and no NUL in it — and spelled through [`literal`] anyway, so that the one place a name is
/// written into SQL is the same place every other value goes through.
fn service_name() -> String {
    literal(SERVICE_NAME).unwrap_or_else(|_| format!("'{SERVICE_NAME}'"))
}

/// A label that is not in the global store.
///
/// **It says which registry it looked in**, because the likeliest reason is a database that
/// was registered into a project. Rule 4's shape applied to a name rather than to a prompt:
/// name the thing that would have fixed it.
fn unknown(label: &str) -> Failure {
    Failure::new(
        Exit::Usage,
        format!("there is no database called {label} in the global registry"),
    )
    .hint(
        "the service has no working directory, so it can only watch the global store. \
         `sloop db list --global` shows what is in it, and `sloop db add --global` puts \
         something there.",
    )
}

/// The daemon's side of the loop, with somewhere to remember what it last complained about.
///
/// **A struct rather than a function, because of what a failing round does every sixty
/// seconds.** A PostgreSQL that is restarting, a store that has been moved, a password that
/// has been rotated — each is a real reason the round cannot run, each is worth saying once,
/// and none is worth saying 1,440 times a day into somebody's journal. So the reason is
/// remembered and a repeat is silent until it changes or until it works again.
pub struct Round {
    store: std::path::PathBuf,
    /// The last thing that went wrong, while it is still going wrong.
    complaint: Option<String>,
    /// What the last round read, so a round that read the same thing says nothing.
    watching: Option<Vec<String>>,
    /// Every SSH forward this daemon holds, opened once and reused.
    ///
    /// **Outside the loop, because a forward per minute is an `ssh` process per minute.**
    /// `R19e`'s *"ten commands, one login"* applied to a process that runs for months. `None`
    /// on a machine where one could not be made at all, which leaves every direct database
    /// working and is said once.
    tunnels: Option<Tunnels>,
    /// What the last round could not read, so a server that is down says so once.
    missed: Vec<String>,
}

impl Round {
    /// One of these per daemon, made before the loop starts.
    #[must_use]
    pub fn at(store: &Path) -> Self {
        Self {
            store: store.to_path_buf(),
            complaint: None,
            watching: None,
            tunnels: Tunnels::new().ok(),
            missed: Vec::new(),
        }
    }

    /// Do a round. Never fails: a daemon that exits because its database blinked is a daemon
    /// that has to be restarted by hand, which is the one thing a service is for avoiding.
    pub fn turn(&mut self) {
        match self.attempt() {
            Ok(watching) => self.settled(watching),
            Err(failure) => self.grumble(failure.message()),
        }
    }

    /// Open the store, read the list, and sample what is on it.
    ///
    /// **One store per round, opened through [`Registries`].** Both halves need it — the list
    /// comes out of it and the readings go into it — and opening two would mean resolving
    /// sloop's own password twice a minute for as long as the machine is up.
    fn attempt(&mut self) -> Outcome<Vec<String>> {
        if Store::open(&self.store)?.is_none() {
            return Err(Failure::new(
                Exit::Usage,
                format!(
                    "there is no sloop store at {} to read the attachment list from",
                    self.store.display()
                ),
            ));
        }

        let registries = Registries::open(Resolution::global_only(), &self.store)?;
        let store = registries
            .store()
            .cloned()
            .ok_or_else(|| Failure::usage("the registry was opened without a database"))?;

        let watching = round(&store)?;

        // **`R26`.** The list is read first and sampled second, so a database attached a
        // moment ago is read on the same round it is picked up on.
        if let Some(tunnels) = self.tunnels.as_ref() {
            let taken = super::sample::round(&store, &registries, tunnels, &self.store, &watching);
            // **Said with the failures, because they are the same kind of thing**: something
            // about a database that a person could act on, and that is worth exactly one line
            // however many months this process runs for.
            let mut worth_saying = taken.missed.clone();
            worth_saying.extend(taken.notes);
            self.grumble_about_the_ones_that_would_not_answer(&worth_saying);
        } else if !watching.is_empty() {
            self.grumble_about_the_ones_that_would_not_answer(&[String::from(
                "no samples: sloop could not prepare to open an SSH forward on this machine",
            )]);
        }

        Ok(watching)
    }

    /// Say which databases would not answer, and say it only when that changes.
    ///
    /// A database that is down stays down, and a line a minute about it for a week is a
    /// journal nobody reads — which is the same reason the round's own complaint is deduped.
    fn grumble_about_the_ones_that_would_not_answer(&mut self, missed: &[String]) {
        if self.missed == missed {
            return;
        }

        for line in missed {
            crate::note!("sloop service: {line}");
        }
        if missed.is_empty() && !self.missed.is_empty() {
            crate::note!("sloop service: every attached database is answering again");
        }
        self.missed = missed.to_vec();
    }

    /// A round that worked, said out loud only when it read something different.
    ///
    /// **This is `R25`'s promise, in the journal.** Attaching a database to a service that is
    /// already running makes a line appear at the next round — nothing was restarted, nothing
    /// was signalled, and the list simply grew. A round that read the same list as the one
    /// before is silent, because a heartbeat every minute forever is noise.
    fn settled(&mut self, watching: Vec<String>) {
        self.complaint = None;
        if self.watching.as_ref() == Some(&watching) {
            return;
        }

        if watching.is_empty() {
            crate::note!(
                "sloop service: nothing is attached — `sloop service attach <name>` attaches \
                 a database"
            );
        } else {
            crate::note!("sloop service: watching {}", watching.join(", "));
        }
        self.watching = Some(watching);
    }

    /// Say it once, and not again until it changes.
    fn grumble(&mut self, what: &str) {
        // A round that failed read nothing, so the next one that works has something to say
        // even if the list is what it was.
        self.watching = None;

        if self.complaint.as_deref() == Some(what) {
            return;
        }
        crate::note!("sloop service: {what}");
        self.complaint = Some(what.to_owned());
    }
}
