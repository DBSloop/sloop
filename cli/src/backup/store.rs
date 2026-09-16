//! What is already on the disk: reading stored backups, and choosing which to remove.
//!
//! **A directory is not a backup until its manifest says so.** `backup` writes the dump,
//! checksums it, and writes `manifest.json` last — so the presence of that file is the one
//! signal that separates a finished backup from a run that was killed halfway, and this
//! module is where that distinction stops being a comment in [`super::manifest`] and starts
//! being something a person is shown. A half-written dump is *reported*, never silently
//! counted as a backup somebody could restore from.
//!
//! **Reading is cheap and hashing is not.** A listing checks that the dump is there and is
//! the size the manifest claims, which catches a truncated file for the price of a `stat`.
//! Hashing gigabytes to draw a table would make this the command nobody runs, so the full
//! checksum is [`Check::Checksum`] and somebody has to ask for it.
//!
//! **Nothing here deletes anything.** [`plan`] decides, the command shows the decision, and
//! only then is anything removed — which is what "pruning previews before it deletes" means
//! in `R12`.

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::manifest::{self, Manifest};
use super::stamp::Stamp;
use super::{DIR, dump_file};
use crate::crypt;
use crate::engine::Engine;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// How hard a scan looks at the dump beside a manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The dump is there and is the size the manifest recorded. One `stat` per backup.
    Size,
    /// The dump hashes to what the manifest recorded. Reads every byte of every dump.
    Checksum,
}

/// One directory under `backups/`, and what was found in it.
#[derive(Debug, Clone)]
pub struct Stored {
    /// Which engine's tree it sits in.
    pub engine: Engine,
    /// The label directory it sits in — the name the database is registered under, made
    /// safe to be one path component.
    pub label: String,
    /// The directory itself.
    pub directory: PathBuf,
    /// When it was taken, read from the directory name, which is the only source a backup
    /// with no manifest has left.
    pub taken: Stamp,
    /// The manifest, when there is one that parses.
    pub manifest: Option<Manifest>,
    /// What is actually in the directory.
    pub state: State,
}

/// What a stored directory turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Manifest present, dump present, and the two agree.
    Complete,
    /// No manifest. The run that made this died before it finished.
    Unfinished,
    /// A manifest that cannot be used — unparseable, or written by a newer sloop.
    Unreadable(String),
    /// A manifest that reads, and a dump that disagrees with it.
    Damaged(String),
}

impl Stored {
    /// Is this something a restore could use?
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.state == State::Complete
    }

    /// Has sloop *verified* a disagreement here?
    ///
    /// Only [`State::Damaged`], and the distinction earns exit `6`: a manifest and a dump
    /// that contradict each other is a mismatch that was measured, where an unfinished
    /// directory is simply a run that never got that far.
    #[must_use]
    pub fn is_damaged(&self) -> bool {
        matches!(self.state, State::Damaged(_))
    }

    /// What is wrong with it, in the words a listing prints.
    #[must_use]
    pub fn problem(&self) -> Option<&str> {
        match &self.state {
            State::Complete => None,
            State::Unfinished => Some("unfinished — no manifest, so the run did not finish"),
            State::Unreadable(why) | State::Damaged(why) => Some(why),
        }
    }

    /// Its size on the disk, whatever state it is in.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        if let Some(manifest) = &self.manifest {
            return manifest.dump.bytes;
        }

        // No manifest to ask: measure whichever dump is lying there, encrypted or not.
        [dump_file(&self.directory), sealed(&self.directory)]
            .iter()
            .find_map(|path| std::fs::metadata(path).ok())
            .map_or(0, |found| found.len())
    }

    /// Engine and label together — the group a retention policy is applied within.
    ///
    /// Retention is per label and not per store: keeping the newest three of *everything*
    /// would keep three copies of one database and none of the other five.
    #[must_use]
    pub fn group(&self) -> (Engine, &str) {
        (self.engine, self.label.as_str())
    }
}

/// Everything under one store's `backups/`.
#[derive(Debug, Default)]
pub struct Found {
    /// The backup directories, newest first.
    pub backups: Vec<Stored>,
    /// Paths inside a label directory that sloop did not write and will not touch: a file
    /// where a backup should be, or a directory whose name is not a timestamp. Reported
    /// rather than ignored, because something put them there.
    pub strays: Vec<PathBuf>,
}

/// Read every backup under a store's root.
///
/// `root` is a registry's directory — the `.sloop` beside the code, or the global store. A
/// store with no `backups/` in it yet is not an error; it is a store nothing has been backed
/// up into.
pub fn scan(root: &Path, check: Check) -> Outcome<Found> {
    let base = root.join(DIR);
    if !base.is_dir() {
        return Ok(Found::default());
    }

    let mut found = Found::default();

    for engine_entry in directories(&base)? {
        // A directory that is not one of the three engines was not written by this tool.
        // Skipped rather than reported: `backups/` lives inside a registry directory, and
        // the owner is allowed to keep their own things next to it.
        let Some(engine) = file_name(&engine_entry).and_then(|name| Engine::parse(name).ok())
        else {
            continue;
        };

        for label_entry in directories(&engine_entry)? {
            let Some(label) = file_name(&label_entry).map(str::to_owned) else {
                continue;
            };

            for entry in children(&label_entry)? {
                let Some(taken) = file_name(&entry)
                    .filter(|_| entry.is_dir())
                    .and_then(Stamp::from_utc_path)
                else {
                    found.strays.push(entry);
                    continue;
                };

                found
                    .backups
                    .push(read_one(engine, label.clone(), entry, taken, check));
            }
        }
    }

    // Newest first, then by label, so a listing reads as a history rather than as a
    // directory walk. `Ord` on `Stamp` is its second count, which is exactly this order.
    found.backups.sort_by(|left, right| {
        right
            .taken
            .cmp(&left.taken)
            .then_with(|| left.label.cmp(&right.label))
    });
    found.strays.sort();

    Ok(found)
}

/// Look at one backup directory.
///
/// Never fails. Everything that can be wrong with a stored backup is a *finding* about that
/// backup rather than a reason to abandon the listing — one unreadable manifest must not be
/// what stops somebody seeing the other nine.
fn read_one(
    engine: Engine,
    label: String,
    directory: PathBuf,
    taken: Stamp,
    check: Check,
) -> Stored {
    let mut stored = Stored {
        engine,
        label,
        directory,
        taken,
        manifest: None,
        state: State::Unfinished,
    };

    if !manifest::completed(&stored.directory) {
        return stored;
    }

    let manifest = match Manifest::read(&manifest::path_in(&stored.directory)) {
        Ok(manifest) => manifest,
        Err(failure) => {
            stored.state = State::Unreadable(failure.message().to_owned());
            return stored;
        }
    };

    stored.state = inspect_dump(&stored.directory, &manifest, check);
    stored.manifest = Some(manifest);
    stored
}

/// Does the dump beside a manifest match what the manifest says about it?
fn inspect_dump(directory: &Path, manifest: &Manifest, check: Check) -> State {
    let dump = directory.join(&manifest.dump.file);

    let Ok(found) = std::fs::metadata(&dump) else {
        return State::Damaged(format!(
            "the manifest is here and {} is not",
            manifest.dump.file
        ));
    };

    if found.len() != manifest.dump.bytes {
        return State::Damaged(format!(
            "{} is {} bytes and the manifest says {}",
            manifest.dump.file,
            found.len(),
            manifest.dump.bytes
        ));
    }

    if check == Check::Size {
        return State::Complete;
    }

    match manifest::checksum(&dump) {
        Ok(hash) if hash == manifest.dump.sha256 => State::Complete,
        Ok(hash) => State::Damaged(format!(
            "{} hashes to {} and the manifest says {}",
            manifest.dump.file,
            front(&hash),
            front(&manifest.dump.sha256)
        )),
        Err(failure) => State::Damaged(failure.message().to_owned()),
    }
}

// ---------------------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------------------

/// A backup being written right now has no manifest either.
///
/// **The one thing that stops `prune` racing `backup`.** An unfinished directory is
/// indistinguishable from an in-progress one until `R17` brings the lockfile, so anything
/// younger than this is left alone and said to be possibly still running. An hour is longer
/// than it sounds: it only has to cover a dump that is still being written, and the cost of
/// being wrong in the other direction is deleting a backup while it is being taken.
pub const GRACE: Duration = Duration::from_secs(60 * 60);

/// What a prune is allowed to remove.
///
/// **`keep` is a floor and `older_than` is the axe.** With both, anything past the newest
/// `keep` copies *and* older than the cutoff goes — which is the policy somebody actually
/// wants from a crontab: throw away last month, but never leave me with fewer than seven.
/// A policy with neither is refused by the command before it reaches here, because a prune
/// that was given no rule would be a prune that deletes everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct Policy {
    /// Keep at least this many of the newest complete backups per label, whatever age.
    pub keep: Option<usize>,
    /// Remove backups older than this.
    pub older_than: Option<Duration>,
    /// Remove unfinished, unreadable and damaged directories as well.
    pub include_broken: bool,
}

/// Why a backup is being left where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Inside the `--keep` floor.
    Newest,
    /// Younger than `--older-than`.
    Young,
    /// Broken, and `--include-broken` was not given.
    Broken,
    /// Broken, but recent enough that it may be a backup being written right now.
    MaybeRunning,
}

impl Held {
    /// The phrase a preview prints beside it.
    #[must_use]
    pub const fn why(self) -> &'static str {
        match self {
            Self::Newest => "inside the keep count",
            Self::Young => "not old enough",
            Self::Broken => "not a finished backup — --include-broken removes these",
            Self::MaybeRunning => "may still be being written — left alone",
        }
    }
}

/// What a prune would do.
#[derive(Debug, Default)]
pub struct Plan {
    /// What would be deleted, newest first.
    pub remove: Vec<Stored>,
    /// What would be left, and why.
    pub held: Vec<(Stored, Held)>,
}

impl Plan {
    /// The bytes a prune would free.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.remove.iter().map(Stored::bytes).sum()
    }
}

/// Decide what a policy removes, and leave a reason against everything it does not.
///
/// `now` is passed in rather than read from the clock so that the decision is a pure
/// function of what is on the disk and what the policy says. A retention rule that can only
/// be tested against the machine's own clock is a retention rule nobody can test.
#[must_use]
pub fn plan(found: Vec<Stored>, now: Stamp, policy: &Policy) -> Plan {
    let cutoff = policy
        .older_than
        .map(|age| now.unix_seconds().saturating_sub(seconds_of(age)));
    let grace = now.unix_seconds().saturating_sub(seconds_of(GRACE));

    let mut plan = Plan::default();
    // How many complete backups have already been seen in this label, newest first — which
    // is what makes `keep` a count of copies rather than a count of directories.
    let mut kept_in_group: Vec<((Engine, String), usize)> = Vec::new();

    for stored in found {
        if !stored.is_complete() {
            // Broken, and possibly not broken at all: a dump still being written looks
            // exactly like one that was abandoned.
            let held = if stored.taken.unix_seconds() > grace {
                Held::MaybeRunning
            } else if policy.include_broken {
                plan.remove.push(stored);
                continue;
            } else {
                Held::Broken
            };
            plan.held.push((stored, held));
            continue;
        }

        let key = (stored.engine, stored.label.clone());
        let index = if let Some(index) = kept_in_group.iter().position(|(group, _)| *group == key) {
            index
        } else {
            kept_in_group.push((key, 0));
            kept_in_group.len() - 1
        };
        let seen = &mut kept_in_group[index].1;

        let floor = policy.keep.unwrap_or(0);
        if *seen < floor {
            *seen += 1;
            plan.held.push((stored, Held::Newest));
            continue;
        }

        match cutoff {
            // Old enough, and past the floor.
            Some(cutoff) if stored.taken.unix_seconds() < cutoff => plan.remove.push(stored),
            Some(_) => {
                *seen += 1;
                plan.held.push((stored, Held::Young));
            }
            // A count and nothing else: everything past the floor goes.
            None => plan.remove.push(stored),
        }
    }

    plan
}

/// Read `30d`, `12h`, `6w` — the age a `--older-than` takes.
///
/// **No `m`.** It is minutes to half the world and months to the other half, and a
/// retention policy is not a place to be ambiguous about a factor of forty-three thousand.
/// Hours are the smallest unit worth having and weeks the largest that is exactly defined;
/// a month is `30d` if somebody wants one, and it says so in the error.
pub fn parse_age(input: &str) -> Outcome<Duration> {
    let text = input.trim();
    let refuse = || {
        Failure::usage(format!("{input} is not an age sloop understands")).hint(
            "a number and a unit: 12h, 30d, 6w — no `m`, because it means minutes to some \
             people and months to others",
        )
    };

    let (count, unit) = text.split_at(text.len().saturating_sub(1));
    let multiplier = match unit {
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        "w" => 7 * 24 * 60 * 60,
        _ => return Err(refuse()),
    };

    if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(refuse());
    }

    count
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .map(Duration::from_secs)
        .ok_or_else(|| {
            Failure::usage(format!("{input} is longer than sloop can measure"))
                .hint("an age in hours, days or weeks, inside a human lifetime")
        })
}

/// A duration as whole seconds, clamped into the signed arithmetic a stamp uses.
fn seconds_of(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

/// The front of a checksum, which is all a line of output has room for.
fn front(sha256: &str) -> String {
    sha256.chars().take(12).collect()
}

/// The encrypted name of the dump in a directory, for a backup with no manifest to name it.
fn sealed(directory: &Path) -> PathBuf {
    crypt::sealed_name(&dump_file(directory))
}

/// The name of a path's last component, as text.
fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(std::ffi::OsStr::to_str)
}

/// Every child directory of a directory.
fn directories(of: &Path) -> Outcome<Vec<PathBuf>> {
    Ok(children(of)?
        .into_iter()
        .filter(|entry| entry.is_dir())
        .collect())
}

/// Every child of a directory, sorted, with the path in the error when there is one.
fn children(of: &Path) -> Outcome<Vec<PathBuf>> {
    let reading = std::fs::read_dir(of).map_err(|error| {
        Failure::new(
            Exit::Usage,
            format!("could not read {}: {error}", of.display()),
        )
        .hint("that is where sloop keeps this store's backups")
    })?;

    let mut found: Vec<PathBuf> = Vec::new();
    for entry in reading {
        let entry = entry.map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("could not read an entry in {}: {error}", of.display()),
            )
        })?;
        found.push(entry.path());
    }
    found.sort();

    Ok(found)
}

/// Delete one stored backup, and say which one when it will not go.
pub fn remove(stored: &Stored) -> Outcome<()> {
    std::fs::remove_dir_all(&stored.directory).map_err(|error| {
        Failure::new(
            Exit::Failure,
            format!("could not remove {}: {error}", stored.directory.display()),
        )
        .hint("the rest were still pruned; this one is still there")
    })
}

/// The label directory a registered name's backups live in.
///
/// The same transformation `backup` applied on the way in, so a filter typed as the name
/// somebody registered finds the directory that name produced — `my/app` and `my_app` are
/// one label on the disk, and a filter that did not know that would find nothing and say
/// there were no backups.
#[must_use]
pub fn label_for(name: &str) -> String {
    super::sanitise(name)
}
