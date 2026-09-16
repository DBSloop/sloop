//! Where a dump goes, and what is written beside it.
//!
//! **This module exists a task early, and only as far as it had to.** `R9` owns the
//! `backup` command; `R8`'s `db drop` takes a safety copy before it destroys a database,
//! and that copy has to land somewhere a person can find and `restore` can read. So the
//! *layout* is settled here and `R9` writes into the same one.
//!
//! **What is deliberately not here is the manifest.** `CLAUDE.md` wants it to record the
//! local time and the offset as well as UTC, and neither can be computed without asking
//! the operating system which timezone it is in — which means a new dependency in a
//! project whose headline claim is how short its dependency graph is. That is a decision
//! for `R9`, in `R9`'s report, rather than something this task slips in on the way past.
//! A `db drop` safety copy is therefore a dump at a UTC-stamped path and nothing else,
//! which is enough to find it and enough to restore it.
//!
//! The layout is not invented either. It is the one in `CLAUDE.md`:
//!
//! ```text
//! backups/<engine>/<label>/<utc-timestamp>/
//!     dump
//! ```
//!
//! **UTC in the path.** `20260916T031500Z` sorts correctly, means the same thing in every
//! timezone, and survives a clock going back an hour — none of which is true of a local
//! timestamp. What a person is shown is a separate question, and `R9`'s one.

pub mod stamp;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::engine::Engine;

/// The directory every backup lives under, inside a project or the global store.
pub const DIR: &str = "backups";

/// Where a backup taken now would go.
///
/// `label` is the name the database is registered under rather than its name on the
/// server: two projects may both have an `app`, and what a person looks for is the name
/// they chose.
#[must_use]
pub fn directory_for(root: &Path, engine: Engine, label: &str, taken: stamp::Stamp) -> PathBuf {
    root.join(DIR)
        .join(engine.scheme())
        .join(sanitise(label))
        .join(taken.utc_path())
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
fn sanitise(label: &str) -> String {
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
