//! Where a dump goes, and what is written beside it.
//!
//! The layout is the one in `CLAUDE.md`:
//!
//! ```text
//! backups/<engine>/<label>/<utc-timestamp>/
//!     dump
//!     manifest.json
//! ```
//!
//! **UTC in the path.** `20260916T031500Z` sorts correctly, means the same thing in every
//! timezone, and survives a clock going back an hour — none of which is true of a local
//! timestamp. What a person is *shown* is the other half of the same rule, and it is
//! always local: see [`stamp::Local`].
//!
//! **This module was started a task early**, by `R8`'s `db drop`, which took a safety copy
//! before it destroyed a database and needed somewhere to put it. It left the manifest to
//! `R9` because the manifest needs the local offset and the local offset needs a dependency;
//! [`manifest`] and the second half of [`stamp`] are that decision, made and paid for.
//!
//! **`db drop` writes nothing now**, by the owner's decision on 2026-09-16, so everything
//! under `backups/` was written by `backup` and has a manifest beside it. A directory here
//! without one is a run that was killed — see [`store`], which is the only thing that has to
//! tell those apart.

pub mod manifest;
pub mod stamp;
pub mod store;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::engine::Engine;

/// The directory every backup lives under, inside a project or the global store.
pub const DIR: &str = "backups";

/// The one directory `--replace` keeps, at a path a script can name.
///
/// **A word, not a timestamp, and that is the whole feature.** `backups/postgres/app/latest`
/// can be written into a cron line, a restore script or a monitoring check once and stay
/// correct for ever, which is what somebody asking for `--replace` is asking for.
pub const LATEST: &str = "latest";

/// Where a replace is written before it is swapped in.
pub const WRITING: &str = "latest.writing";

/// Where the copy being displaced waits until the swap is finished.
pub const DISPLACED: &str = "latest.previous";

/// What a directory under a label is.
///
/// Four names, because a replace is a swap and a swap has a middle. Everything except
/// [`Kind::Sequential`] is `--replace`'s, and the three of those are the live copy and the
/// two halves of an interrupted swap — which exist on disk for microseconds normally, and
/// until the next `--replace` run when a machine is turned off at the wrong moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `<utc-timestamp>/` — a copy of its own, and what retention counts.
    Sequential,
    /// `latest/` — the one copy `--replace` keeps.
    Replaced,
    /// `latest.writing/` — a replace being written, or one that was killed.
    Writing,
    /// `latest.previous/` — the copy a swap displaced and did not finish removing.
    Displaced,
}

impl Kind {
    /// What a directory's name says it is, or `None` when sloop did not write it.
    #[must_use]
    pub fn of(name: &str) -> Option<Self> {
        if stamp::Stamp::from_utc_path(name).is_some() {
            return Some(Self::Sequential);
        }
        match name {
            LATEST => Some(Self::Replaced),
            WRITING => Some(Self::Writing),
            DISPLACED => Some(Self::Displaced),
            _ => None,
        }
    }

    /// Is this one of `--replace`'s, rather than a copy of its own?
    ///
    /// Retention skips every one of them: a label that keeps exactly one copy at a fixed
    /// path has nothing for "keep the newest seven" to mean, and the two transient names
    /// are the next `--replace` run's to tidy, not a prune's to delete.
    #[must_use]
    pub const fn is_replace(self) -> bool {
        !matches!(self, Self::Sequential)
    }

    /// What a listing says about it, when there is anything to say.
    #[must_use]
    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Sequential => None,
            Self::Replaced => Some("replace"),
            Self::Writing => Some("a replace that did not finish — the next one tidies it"),
            Self::Displaced => {
                Some("displaced by an interrupted replace — the next one puts it back")
            }
        }
    }
}

/// Every backup of one label, whatever shape they are in.
///
/// Both modes write under here: `--sequential` adds a timestamped directory each run, and
/// `--replace` keeps [`LATEST`] in the same place.
#[must_use]
pub fn label_dir(root: &Path, engine: Engine, label: &str) -> PathBuf {
    root.join(DIR).join(engine.scheme()).join(sanitise(label))
}

/// Where a backup taken now would go.
///
/// `label` is the name the database is registered under rather than its name on the
/// server: two projects may both have an `app`, and what a person looks for is the name
/// they chose.
#[must_use]
pub fn directory_for(root: &Path, engine: Engine, label: &str, taken: stamp::Stamp) -> PathBuf {
    label_dir(root, engine, label).join(taken.utc_path())
}

/// The dump file inside a backup directory.
///
/// No extension. `R11` adds `.age` for an encrypted one, and a bare `dump` today leaves
/// that decision where it belongs rather than promising a format in a filename.
#[must_use]
pub fn dump_file(directory: &Path) -> PathBuf {
    directory.join("dump")
}

/// A label, made safe to be one path component.
///
/// A registered name may contain almost anything a person can type — `db add` only refuses
/// a colon and stray whitespace — and a `/` in one would otherwise put the backup in a
/// directory nobody goes looking in. Replaced rather than refused, because a name that
/// works everywhere else in the tool should not stop working here.
pub fn sanitise(label: &str) -> String {
    let swapped: String = label
        .chars()
        .map(|letter| {
            if letter.is_control()
                || matches!(letter, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                letter
            }
        })
        .collect();

    // A trailing dot or space is legal in a name and illegal in a Windows directory, and
    // a name made entirely of them would leave nothing at all.
    let trimmed = swapped.trim_end_matches([' ', '.']).to_owned();
    if trimmed.is_empty() {
        "unnamed".to_owned()
    } else {
        trimmed
    }
}
