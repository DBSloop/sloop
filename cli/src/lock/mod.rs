//! One lock per database, so two runs never write the same thing at once — `R17`.
//!
//! **The failure this prevents is a scheduled one.** A backup that takes longer than the gap
//! between two cron lines has a second copy of itself starting while the first is still
//! dumping; a `mirror` into a database that a `restore` is already replacing leaves neither
//! of them holding what they think. Neither is a mistake anybody makes at a prompt — both are
//! things that happen at three in the morning to somebody who did everything right.
//!
//! **The kernel holds it, not a file that says it is held.** [`std::fs::File::try_lock`] takes
//! an advisory lock the operating system owns, and an operating system releases every lock a
//! process held the moment that process stops existing — killed, crashed, powered off. A
//! lockfile containing a process id would need this program to ask whether that process is
//! still alive, which cannot be done portably without `unsafe`, and would leave a database
//! locked forever the first time somebody pulled a plug.
//!
//! **The note sits beside the lock rather than inside it**, and that is not tidiness. On
//! Windows the lock covers the file's bytes as well as the file, so a run that is refused
//! cannot read what the holder wrote there — which is exactly the moment the message was for.
//! A sibling `.who` file is readable by anyone.
//!
//! **And it is emptied when the lock goes, which it did not used to be.** A note that
//! outlived its holder made [`refused`] describe a run that had already finished as though
//! it were still working — a red CI build once reported a process id that was the reading
//! process's own, and the hours that went into looking for the holder went into looking for
//! something that was not there. The note is truncated *while the lock is still held*, so
//! nobody can take the lock in between and find it empty; what is left is an empty file,
//! because deleting it would race the next run opening it.
//!
//! **A note naming this very process is not a busy machine**, and it says so in those words.
//! One run being refused its own lock is either a [`Held`] somebody kept alive past its work
//! or a release the operating system has not made visible yet — see
//! [`tests::the_lock_goes_when_the_holder_does`], which is where that second one was first
//! written down. Neither is "another sloop run", and reporting it as one sent somebody after
//! a process that was themselves.
//!
//! **Exit `7`, frozen.** A scheduler that reads it knows this run did not fail — it did not
//! start, and the next one along will be fine.

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// The directory locks live in, under whichever store the database belongs to.
const DIRECTORY: &str = "locks";

/// A lock that is held for as long as this value is alive.
///
/// Dropping it releases the lock. So does the process ending, however it ends, which is the
/// property a lockfile of this program's own devising could not have had.
#[derive(Debug)]
pub struct Held {
    /// Kept only so [`Held::path`] can answer.
    #[cfg_attr(not(test), allow(dead_code))]
    path: PathBuf,
    /// **The lock itself.** Held by this handle and released when it closes.
    _file: File,
}

impl Drop for Held {
    /// Empty the note, then release the lock.
    ///
    /// **That order is the whole point.** Truncating while the lock is still held means no
    /// other run can be between the two: whoever takes it next writes its own note first.
    /// The other order would leave a window in which the note belongs to nobody.
    fn drop(&mut self) {
        let _ = std::fs::write(beside(&self.path), b"");
    }
}

impl Held {
    /// Where the lock is, for a run that wants to say so.
    ///
    /// Only the tests ask today — the commands take a lock and forget about it, which is the
    /// point. It stays because "where is this lock" is the first question anybody debugging
    /// an exit 7 has, and answering it from outside the module needs this.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Take the lock for one database, or fail with exit `7`.
///
/// `store` is the registry's own directory, so a database registered in a project and one
/// registered globally under the same name are two different locks — which is right, because
/// they are two different databases.
///
/// `doing` is the command's name, written into the file so the run that is refused can say
/// what is holding it.
pub fn take(store: &Path, label: &str, doing: &str) -> Outcome<Held> {
    let path = path_for(store, label);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Failure::new(
                Exit::Failure,
                format!("could not create {}: {error}", parent.display()),
            )
        })?;
    }

    // Opened for writing rather than created anew: the file outlives the lock on purpose, so
    // the ordinary case is opening one that is already there.
    let file = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| {
            Failure::new(
                Exit::Failure,
                format!("could not open {}: {error}", path.display()),
            )
        })?;

    match file.try_lock() {
        Ok(()) => {}

        // **Held by somebody else, which is not a failure of this run.** Exit 7 says so, and
        // says it the same way every time, because a scheduler reads the number.
        Err(std::fs::TryLockError::WouldBlock) => return Err(refused(&path, label)),

        // **And this is not that.** It used to be — every error was reported as "another
        // sloop run is working on this", including a filesystem that cannot lock and a file
        // that cannot be reached. Exit 7 tells a scheduler the run did not start and the
        // next one will be fine, so saying it about something that will still be true in an
        // hour sends somebody looking for a process that was never there.
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(Failure::new(
                Exit::Failure,
                format!("could not lock {}: {error}", path.display()),
            )
            .hint(
                "this is not another run holding it — the lock itself could not be taken. A \
                 filesystem that does not support locking is the usual reason",
            ));
        }
    }

    // Written only once the lock is ours, so whoever reads it is reading the holder's.
    let _ = write_who(&path, doing);

    Ok(Held { path, _file: file })
}

/// The lockfile for one label in one store.
///
/// The label is sanitised the same way a backup directory's is — it has to be one path
/// component, and the same name has to produce the same file every time or the lock locks
/// nothing.
#[must_use]
pub fn path_for(store: &Path, label: &str) -> PathBuf {
    store
        .join(DIRECTORY)
        .join(format!("{}.lock", crate::backup::sanitise(label)))
}

/// The refusal, with whatever the holder said about itself.
fn refused(path: &Path, label: &str) -> Failure {
    let held_by = std::fs::read_to_string(beside(path))
        .ok()
        .map(|said| said.trim().to_owned())
        .filter(|said| !said.is_empty());

    // **A lock this process already holds is a defect, and saying "another sloop run" about
    // it sends somebody looking for a process that does not exist.** It happened: a red
    // build reported a process id that was the reading process's own, and nobody could tell
    // from the message that the two were the same.
    if held_by
        .as_deref()
        .is_some_and(|who| who.contains(&format!("process {}", std::process::id())))
    {
        // **Still exit 7**, because the lock is held and rule 7 froze what that means. Only
        // the words change, and they have to: a scheduler reads the number and a person
        // reads the sentence, and the sentence was sending people after a process that was
        // themselves.
        return Failure::new(
            Exit::Locked,
            format!("this sloop run took the lock on {label} and has not released it"),
        )
        .hint(format!(
            "no other run is involved, so one of two things is true: something is keeping \
             the lock at {} alive past the work it was taken for, or it was released a \
             moment ago and the operating system has not made that visible yet — see \
             `lock::tests::the_lock_goes_when_the_holder_does`",
            path.display()
        ));
    }

    let message = match held_by {
        // A courtesy rather than a guarantee: a note that cannot be read still leaves a
        // sentence that says the useful half.
        Some(who) => format!("another sloop run is working on {label}: {who}"),
        None => format!("another sloop run is working on {label}"),
    };

    Failure::new(Exit::Locked, message).hint(format!(
        "exit 7 means this run did not start rather than that it failed. The lock is at {}, \
         and it goes when that run does",
        path.display()
    ))
}

/// Say who holds it, for the benefit of whoever is refused.
fn write_who(path: &Path, doing: &str) -> std::io::Result<()> {
    std::fs::write(
        beside(path),
        format!(
            "`sloop {doing}`, process {}, since {}
",
            std::process::id(),
            crate::backup::stamp::Stamp::now().utc_path()
        ),
    )
}

/// The note that sits beside a lock.
#[must_use]
pub fn beside(lock: &Path) -> PathBuf {
    lock.with_extension("who")
}
