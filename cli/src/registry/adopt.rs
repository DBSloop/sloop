//! Taking over a store an older sloop left somewhere else.
//!
//! The global store used to follow each platform's config convention — `%APPDATA%\sloop`,
//! `~/.config/sloop`, `~/Library/Application Support/sloop`. It is `~/.sloop` now, on all
//! three. Somebody who had already registered databases has them in the old place, and the
//! two ways of handling that which are *not* acceptable are reading neither (their registry
//! appears to have been wiped) and reading both (two registries drifting apart, with no way
//! to tell which one a command just wrote to).
//!
//! So: the old one is **moved**, once, on the first run that looks for the store, and the
//! move is said out loud. After it there is one store and the old path does not exist. A
//! move that cannot be done is a failure with both paths in it, never a silent half-state —
//! data that is hard to reach is recoverable, and data that has quietly become two copies
//! is not.

use std::path::{Path, PathBuf};

use crate::failure::{Failure, Outcome};
use crate::style;

use super::locations::Locations;

/// The global store, having taken over an older one if there was one to take over.
///
/// **Every route to the global store goes through here**, rather than through
/// [`Locations::global_dir`] directly, so that no command can be the one that reads an empty
/// store while the real one sits at the old path. It costs one `exists` call per run once
/// the move has happened, because the old directory is gone by then.
pub fn global(locations: &Locations) -> Outcome<PathBuf> {
    let personal = locations.global_dir()?;

    if let Some(machine) = locations.machine_dir()
        && pick(
            Found::of(personal.exists(), machine.exists()),
            crate::account::is_root(),
        ) == Which::Machine
    {
        // **A machine-wide store is never adopted into, and that is the point of returning
        // here.** `~/.config/sloop` belonged to one account; moving it into a store every
        // account on the box reads would hand that account's databases to all of them.
        return Ok(machine);
    }

    let target = personal;

    let Some(old) = locations.legacy_dir().filter(|old| *old != target) else {
        return Ok(target);
    };
    if !holds_anything(&old) {
        return Ok(target);
    }

    // **`R19c` made the target always exist, so "is it there" stopped being the question.**
    // Every set-up machine has `~/.sloop/server.toml` — the record saying which PostgreSQL
    // holds the registry — so a test of mere existence would refuse to migrate any machine
    // that had ever run Setup, which is all of them. What decides is whether the target holds
    // a *store*: a registry, an index, sealed passwords. A directory holding nothing but the
    // server record has nothing to collide with, and the old store moves into it.
    if holds_a_store(&target) {
        // Rare: the move already happened and something recreated the old directory. Said
        // rather than ignored, because the alternative is somebody adding databases to a
        // store nothing reads and having no idea why they never appear.
        crate::report::notice(&style::dim(&format!(
            "Note: {} also holds a sloop store. It is not read — the store is {}.",
            old.display(),
            target.display()
        )));
        return Ok(target);
    }

    move_into(&old, &target)?;
    crate::report::notice(&style::dim(&format!(
        "Moved the global store from {} to {}.",
        old.display(),
        target.display()
    )));

    Ok(target)
}

/// Which of the two stores a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// `~/.sloop`, this account's own.
    Personal,
    /// `/var/lib/sloop`, the one every account on the machine reads.
    Machine,
}

/// Which store this run works on — `R31`, and the whole of it is these four lines.
///
/// **An account that already has its own store keeps it.** That is first for a reason: a
/// machine-wide store appearing later must never make somebody's registered databases
/// disappear, and "the one you have been using" is the only rule that cannot do that.
///
/// **Then the machine-wide one, if the machine has one.** This is what the owner asked for —
/// a store root created is the store `ubuntu@` finds afterwards, with nothing to configure.
///
/// **Then root makes one and everybody else keeps their own.** A root run has nowhere else
/// to put a cluster: its home is `0700`, so the postmaster it starts could not read a data
/// directory inside it even as the account that owns it.
///
/// Pure, and takes the two `exists` answers rather than asking, so every branch is checkable
/// from a machine that is none of these platforms.
const fn pick(found: Found, root: bool) -> Which {
    match found {
        // **An account that already has its own store keeps it.** First for a reason: a
        // machine-wide store appearing later must never make somebody's registered databases
        // look as though they had been wiped.
        Found::Personal | Found::Both => Which::Personal,
        // **What the owner asked for.** A store root made is the store the next account
        // finds, with nothing to configure and nothing to be told.
        Found::Machine => Which::Machine,
        // **Root has nowhere else to put a cluster.** Its home is `0700`, so the postmaster
        // could not read a data directory inside it even as the account that owns one.
        // Anybody else makes their own, exactly as they always did.
        Found::Neither => {
            if root {
                Which::Machine
            } else {
                Which::Personal
            }
        }
    }
}

/// Which of the two stores this machine already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// A fresh machine: whoever runs first decides.
    Neither,
    /// This account has `~/.sloop`.
    Personal,
    /// The machine has a store and this account has never made one.
    Machine,
    /// Both, which is a machine somebody used before it was shared.
    Both,
}

impl Found {
    /// From the two questions the disk answers.
    const fn of(personal: bool, machine: bool) -> Self {
        match (personal, machine) {
            (true, true) => Self::Both,
            (true, false) => Self::Personal,
            (false, true) => Self::Machine,
            (false, false) => Self::Neither,
        }
    }
}

/// Does this directory exist and have anything in it?
///
/// An empty leftover directory is not a store, and moving one would announce a migration
/// that migrated nothing.
fn holds_anything(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Does this directory hold a *store*, rather than just the record of where the database is?
///
/// `server.toml` is not a store. It is one file saying which PostgreSQL the store lives in,
/// written by Setup on every machine, and it is the one thing in `~/.sloop` that has to stay
/// exactly where it is — everything else can be moved onto it.
fn holds_a_store(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };

    entries
        .flatten()
        .any(|entry| entry.file_name() != std::ffi::OsStr::new(crate::server::record::FILE))
}

/// Move `from` onto `to`, across volumes if it has to.
///
/// `rename` is one call and keeps everything — including a file that is open. It also fails
/// across volumes, and `%APPDATA%` on a roaming Windows profile really is on another one, so
/// the copy is not a theoretical fallback.
fn move_into(from: &Path, to: &Path) -> Outcome<()> {
    std::fs::create_dir_all(to).map_err(|error| cannot(from, to, &error.to_string()))?;

    // **Entry by entry, not a rename of the whole directory.** `to` may already hold
    // `server.toml`, and a rename onto a directory that is not empty fails — so the old
    // store's contents move *into* the new one and the record stays where Setup put it.
    // Nothing is overwritten: the only file that could collide is the record, and the old
    // store predates it.
    let entries = std::fs::read_dir(from).map_err(|error| cannot(from, to, &error.to_string()))?;
    for entry in entries.flatten() {
        let landing = to.join(entry.file_name());
        if landing.exists() {
            continue;
        }
        if std::fs::rename(entry.path(), &landing).is_err() {
            // Across volumes — `%APPDATA%` on a roaming Windows profile really is on another
            // one — so a copy is not a theoretical fallback.
            copy_entry(&entry.path(), &landing)
                .map_err(|error| cannot(from, to, &error.to_string()))?;
        }
    }

    std::fs::remove_dir_all(from).map_err(|error| {
        Failure::usage(format!(
            "moved the global store to {} but could not remove the old one at {}: {error}",
            to.display(),
            from.display()
        ))
        .hint("delete the old directory by hand — sloop reads the new one and would otherwise say this every run")
    })
}

/// One file or one directory, copied.
fn copy_entry(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        copy_tree(from, to)
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

#[allow(dead_code)]
fn move_directory(from: &Path, to: &Path) -> Outcome<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|error| cannot(from, to, &error.to_string()))?;
    }

    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }

    copy_tree(from, to).map_err(|error| {
        // Leave the half-copy behind rather than deleting it: it is a copy of the user's
        // registry, and the sentence below tells them where both halves are.
        cannot(from, to, &error.to_string())
    })?;

    std::fs::remove_dir_all(from).map_err(|error| {
        Failure::usage(format!(
            "copied the global store to {} but could not remove the old one at {}: {error}",
            to.display(),
            from.display()
        ))
        .hint("delete the old directory by hand — sloop reads the new one and would otherwise say this every run")
    })
}

fn cannot(from: &Path, to: &Path, why: &str) -> Failure {
    Failure::usage(format!(
        "cannot move the global store from {} to {}: {why}",
        from.display(),
        to.display()
    ))
    .hint(format!(
        "move it by hand — sloop reads {} and will not read the old path",
        to.display()
    ))
}

/// Copy a directory and everything under it.
///
/// Files and directories, nothing clever. The store holds a registry, an encrypted password
/// file, a keypair, pointer files and backups; none of them is a symlink or a device, and a
/// copy that tried to be general would be a copy with more ways to go wrong.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;

    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{copy_tree, holds_anything, move_directory};

    /// A temporary directory of this test's own, removed when it goes out of scope.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "sloop-adopt-{}-{label}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("a temporary directory should be creatable");
            Self(root)
        }

        fn at(&self, relative: &str) -> PathBuf {
            let mut path = self.0.clone();
            for part in relative.split('/') {
                path.push(part);
            }
            path
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.at(relative);
            std::fs::create_dir_all(path.parent().expect("a file has a parent"))
                .expect("a temporary directory should be creatable");
            std::fs::write(&path, contents).expect("a temporary file should be writable");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("the file should be there")
    }

    /// `R31`'s rule, one case each. Pure, so every branch is reachable from a host that is
    /// none of the platforms it decides for.
    mod which_store {
        use super::super::{Found, Which, pick};

        const ROOT: bool = true;
        const ANYBODY: bool = false;

        #[test]
        fn a_store_you_already_have_is_the_one_you_keep() {
            // First, and first for a reason: a machine-wide store appearing later must never
            // make somebody's registered databases look as though they were wiped.
            assert_eq!(pick(Found::Both, ROOT), Which::Personal);
            assert_eq!(pick(Found::Both, ANYBODY), Which::Personal);
            assert_eq!(pick(Found::Personal, ROOT), Which::Personal);
            assert_eq!(pick(Found::Personal, ANYBODY), Which::Personal);
        }

        #[test]
        fn the_machine_store_is_found_by_an_account_that_never_made_one() {
            // The whole of what the owner asked for: root sets the machine up, and `ubuntu@`
            // afterwards sees the same databases with nothing to configure.
            assert_eq!(pick(Found::Machine, ANYBODY), Which::Machine);
        }

        #[test]
        fn root_makes_the_machine_store_rather_than_one_under_slash_root() {
            // Root's home is 0700, so a cluster inside it cannot be read by the account the
            // postmaster runs as. This line is why `initdb` stopped refusing.
            assert_eq!(pick(Found::Neither, ROOT), Which::Machine);
        }

        #[test]
        fn everybody_else_keeps_their_own() {
            assert_eq!(pick(Found::Neither, ANYBODY), Which::Personal);
        }

        #[test]
        fn what_is_on_the_disk_reads_the_way_it_is_asked() {
            assert_eq!(Found::of(false, false), Found::Neither);
            assert_eq!(Found::of(true, false), Found::Personal);
            assert_eq!(Found::of(false, true), Found::Machine);
            assert_eq!(Found::of(true, true), Found::Both);
        }
    }

    #[test]
    fn a_move_takes_everything_and_leaves_nothing() {
        let scratch = Scratch::new("move");
        scratch.write("old/sloop.toml", "registry");
        scratch.write("old/projects/demo", "/somewhere/demo");
        scratch.write("old/backups/postgres/demo/stamp/dump", "bytes");

        move_directory(&scratch.at("old"), &scratch.at("new")).expect("the move should work");

        assert!(!scratch.at("old").exists(), "the old store is still there");
        assert_eq!(read(&scratch.at("new/sloop.toml")), "registry");
        assert_eq!(read(&scratch.at("new/projects/demo")), "/somewhere/demo");
        assert_eq!(
            read(&scratch.at("new/backups/postgres/demo/stamp/dump")),
            "bytes"
        );
    }

    /// The fallback path, exercised directly: `rename` succeeds on one volume, so the only
    /// way to know the copy works is to call it.
    #[test]
    fn the_cross_volume_copy_carries_the_whole_tree() {
        let scratch = Scratch::new("copy");
        scratch.write("old/sloop.toml", "registry");
        scratch.write("old/a/b/c/deep", "buried");

        copy_tree(&scratch.at("old"), &scratch.at("new")).expect("the copy should work");

        assert_eq!(read(&scratch.at("new/sloop.toml")), "registry");
        assert_eq!(read(&scratch.at("new/a/b/c/deep")), "buried");
        assert!(scratch.at("old").exists(), "a copy does not remove");
    }

    #[test]
    fn an_empty_directory_is_not_a_store() {
        let scratch = Scratch::new("empty");
        std::fs::create_dir_all(scratch.at("old")).expect("creatable");

        assert!(!holds_anything(&scratch.at("old")));
        assert!(!holds_anything(&scratch.at("never-existed")));

        scratch.write("old/sloop.toml", "registry");
        assert!(holds_anything(&scratch.at("old")));
    }
}
