//! The schedule the service keeps — `R27a`, and the reason a cron line is never needed.
//!
//! **Owner's words:** *"so in that case we don't need to setup cron manually per system, sloop
//! can handle, can handle backup retention policy, schedule etc"*. Per database: how often, how
//! many to keep, how old is too old — the same three things `backups prune` already takes,
//! stored against the database rather than typed into a crontab.
//!
//! **It runs the commands, it does not reimplement them.** `backup::run` and `backups::prune`,
//! exactly as the menu calls them. A second implementation of taking a backup is a second set
//! of bugs in the one thing this tool exists to get right.
//!
//! **A missed run is caught up, not skipped silently, and is labelled late.** A laptop asleep
//! at 03:00 backs up when it wakes. The next run is then measured from *now* rather than from
//! when it was due — a machine that was off for a week owes one backup, not a hundred and
//! sixty-eight.
//!
//! **`R17`'s lock is what keeps it honest.** A scheduled run that collides with a manual one
//! gets exit `7` from `backup::run` itself, and the schedule records that and stands aside
//! rather than queueing a second.
//!
//! **Rule 4 is at its sharpest here, because a service has no terminal at all.** Nothing below
//! can ask a question: `backup::run` is handed a consent that answers the ones a schedule
//! implies, and anything else it would have asked is a failure it reports rather than a prompt
//! nobody can see.

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;

use std::time::Duration;

use serde::Deserialize;

use crate::backup::stamp::Stamp;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::store::{Store, literal};

/// What a database's schedule says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// How often a backup is taken.
    pub every: Duration,
    /// Keep this many of the newest.
    pub keep_last: Option<usize>,
    /// Remove anything older than this many days.
    pub keep_for_days: Option<u32>,
}

impl Policy {
    /// Does it delete anything at all?
    ///
    /// **Both unset is "keep everything"**, which is exactly what `backups prune` refuses to
    /// run with — so a schedule with no policy takes backups and prunes nothing, rather than
    /// deleting on a rule nobody set.
    #[must_use]
    pub const fn prunes(&self) -> bool {
        self.keep_last.is_some() || self.keep_for_days.is_some()
    }
}

/// One database's schedule, as the table holds it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Scheduled {
    /// What it is registered as.
    pub label: String,
    /// Seconds between runs. `None` means nobody has asked for backups.
    pub every_seconds: Option<i64>,
    /// Keep this many of the newest.
    pub keep_last: Option<i64>,
    /// Remove anything older than this many days.
    pub keep_for_days: Option<i64>,
    /// When the next one is due.
    pub due_at: Option<i64>,
    /// When the last one ran.
    pub ran_at: Option<i64>,
    /// What it exited with.
    pub exit_code: Option<i64>,
    /// A word for what happened.
    pub outcome: Option<String>,
    /// Whether that run was later than it should have been.
    pub was_late: Option<bool>,
}

impl Scheduled {
    /// The policy, when there is one.
    #[must_use]
    pub fn policy(&self) -> Option<Policy> {
        let every = u64::try_from(self.every_seconds?).ok()?;
        Some(Policy {
            every: Duration::from_secs(every),
            keep_last: self.keep_last.and_then(|n| usize::try_from(n).ok()),
            keep_for_days: self.keep_for_days.and_then(|n| u32::try_from(n).ok()),
        })
    }

    /// Is a backup owed, at this moment?
    ///
    /// **A schedule with no run behind it is due immediately**, which is what somebody setting
    /// one up expects: the first backup happens, rather than happening in a day.
    #[must_use]
    pub fn due(&self, now: Stamp) -> bool {
        self.policy().is_some() && self.due_at.is_none_or(|at| at <= now.unix_seconds())
    }

    /// Was this run later than one whole interval after it was due?
    ///
    /// **One whole interval, because that is what a *missed* run means.** A round that fires a
    /// few seconds after the moment is not late; a laptop that was asleep through an entire
    /// scheduled backup is, and saying so is the difference between a schedule somebody can
    /// trust and one that flatters itself.
    #[must_use]
    pub fn late(&self, now: Stamp) -> bool {
        let (Some(policy), Some(due)) = (self.policy(), self.due_at) else {
            return false;
        };

        let behind = now.unix_seconds() - due;
        behind > i64::try_from(policy.every.as_secs()).unwrap_or(i64::MAX)
    }

    /// When it was due, as a stamp.
    #[must_use]
    pub fn due_stamp(&self) -> Option<Stamp> {
        self.due_at.map(Stamp::from_unix_seconds)
    }

    /// When it last ran, as a stamp.
    #[must_use]
    pub fn ran_stamp(&self) -> Option<Stamp> {
        self.ran_at.map(Stamp::from_unix_seconds)
    }
}

/// Every attached database and whatever schedule it has.
pub fn all(store: &Store) -> Outcome<Vec<Scheduled>> {
    store.json(SCHEDULES)
}

/// Set, change or clear one database's schedule.
///
/// `every` of `None` clears it: attached and sampled, and nobody has asked for backups.
pub fn set(store: &Store, label: &str, policy: Option<Policy>) -> Outcome<()> {
    let label = literal(label)?;

    let (every, keep_last, keep_days) = match policy {
        None => (
            String::from("NULL"),
            String::from("NULL"),
            String::from("NULL"),
        ),
        Some(policy) => (
            policy.every.as_secs().to_string(),
            policy
                .keep_last
                .map_or_else(|| String::from("NULL"), |n| n.to_string()),
            policy
                .keep_for_days
                .map_or_else(|| String::from("NULL"), |n| n.to_string()),
        ),
    };

    store.run(&format!(
        "UPDATE monitored_database m
            SET backup_every_seconds = {every},
                keep_last            = {keep_last},
                keep_for_days        = {keep_days},
                -- **Due now, on a schedule that has just been set.** Somebody who asks for a
                -- daily backup wants the first one today, not tomorrow. Clearing it clears
                -- this too, so nothing is owed by a database nobody is backing up.
                backup_due_at        = CASE WHEN {every} IS NULL THEN NULL
                                            ELSE coalesce(m.backup_due_at, now()) END
           FROM service s, registered_database d
          WHERE m.service_id = s.id AND s.name = 'sloop'
            AND m.registered_database_id = d.id
            AND d.label = {label} AND d.project_id IS NULL;"
    ))
}

/// Write down what a scheduled run did, and when the next one is.
///
/// **Measured from now, not from when it was due.** A machine that was off for a week owes one
/// backup and not a hundred and sixty-eight, so catching up is one run and then back on
/// schedule.
pub fn record(store: &Store, label: &str, ran: &Ran) -> Outcome<()> {
    let label = literal(label)?;
    let outcome = literal(ran.outcome)?;

    store.run(&format!(
        "UPDATE monitored_database m
            SET backup_ran_at    = now(),
                backup_exit_code = {code},
                backup_outcome   = {outcome},
                backup_was_late  = {late},
                backup_due_at    = now() + make_interval(secs => {every})
           FROM service s, registered_database d
          WHERE m.service_id = s.id AND s.name = 'sloop'
            AND m.registered_database_id = d.id
            AND d.label = {label} AND d.project_id IS NULL;",
        code = ran.exit.code(),
        late = if ran.late { "TRUE" } else { "FALSE" },
        every = ran.every.as_secs(),
    ))
}

/// What one scheduled run came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ran {
    /// How it ended.
    pub exit: Exit,
    /// A word for it, for the screen.
    pub outcome: &'static str,
    /// Whether it was later than it should have been.
    pub late: bool,
    /// The interval, for working out when the next one is.
    pub every: Duration,
}

impl Ran {
    /// What an exit code means for a schedule.
    ///
    /// **`7` is not a failure and must not read as one.** A scheduled run that collided with a
    /// manual one stood aside; the backup somebody was already taking is the backup. `6` is
    /// its own thing too — finished, and the counts disagreed — which is neither success nor
    /// a crash and has been frozen as its own code since `R13`.
    #[must_use]
    pub const fn describe(exit: Exit) -> &'static str {
        match exit {
            Exit::Success => "succeeded",
            Exit::Locked => "stood aside — a run was already going",
            Exit::Mismatch => "finished, but the counts disagreed",
            _ => "failed",
        }
    }
}

/// How a schedule is spelled on the command line, and what it means in seconds.
///
/// **The same vocabulary `backups prune --older-than` already uses**, because somebody who has
/// typed `30d` once should not have to learn a second spelling for the same idea.
pub fn every_from(text: &str) -> Outcome<Duration> {
    let trimmed = text.trim();
    let (count, unit) = trimmed.split_at(
        trimmed
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );

    let count: u64 = count.parse().map_err(|_| unreadable(trimmed))?;
    let seconds = match unit.trim() {
        "h" | "hour" | "hours" => 3_600,
        "d" | "day" | "days" => 86_400,
        "w" | "week" | "weeks" => 604_800,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        _ => return Err(unreadable(trimmed)),
    };

    let every = count
        .checked_mul(seconds)
        .ok_or_else(|| unreadable(trimmed))?;
    if every == 0 {
        return Err(Failure::usage("a backup every nothing is not a schedule")
            .hint("`--every 1d` is once a day; `--every 6h` is four times a day"));
    }

    Ok(Duration::from_secs(every))
}

/// How an interval reads back to a person.
#[must_use]
pub fn every_reads_as(every: Duration) -> String {
    let seconds = every.as_secs();
    let (count, unit) = if seconds.is_multiple_of(604_800) {
        (seconds / 604_800, "week")
    } else if seconds.is_multiple_of(86_400) {
        (seconds / 86_400, "day")
    } else if seconds.is_multiple_of(3_600) {
        (seconds / 3_600, "hour")
    } else {
        (seconds / 60, "minute")
    };

    if count == 1 {
        format!("every {unit}")
    } else {
        format!("every {count} {unit}s")
    }
}

fn unreadable(text: &str) -> Failure {
    Failure::usage(format!("{text} is not an interval sloop understands")).hint(
        "a number and a unit: `30m`, `6h`, `1d`, `2w` — the same spelling `backups prune \
         --older-than` takes",
    )
}

/// Every attached database, with its schedule and what the last run came to.
const SCHEDULES: &str = "SELECT coalesce(json_agg(json_build_object(
          'label', d.label,
          'every_seconds', m.backup_every_seconds,
          'keep_last', m.keep_last,
          'keep_for_days', m.keep_for_days,
          'due_at', floor(extract(epoch FROM m.backup_due_at))::bigint,
          'ran_at', floor(extract(epoch FROM m.backup_ran_at))::bigint,
          'exit_code', m.backup_exit_code,
          'outcome', m.backup_outcome,
          'was_late', m.backup_was_late
        ) ORDER BY d.label), '[]')
   FROM monitored_database m, service s, registered_database d
  WHERE m.service_id = s.id AND s.name = 'sloop' AND m.enabled
    AND m.registered_database_id = d.id;";
