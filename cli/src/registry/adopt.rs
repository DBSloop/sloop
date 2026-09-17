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
    let target = locations.global_dir()?;

    let Some(old) = locations.legacy_dir().filter(|old| *old != target) else {
        return Ok(target);
    };
    if !holds_anything(&old) {
        return Ok(target);
    }

    if target.exists() {
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

    move_directory(&old, &target)?;
    crate::report::notice(&style::dim(&format!(
        "Moved the global store from {} to {}.",
        old.display(),
        target.display()
    )));

    Ok(target)
}

/// Does this directory exist and have anything in it?
///
/// An empty leftover directory is not a store, and moving one would announce a migration
/// that migrated nothing.
fn holds_anything(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Move `from` onto `to`, across volumes if it has to.
///
/// `rename` is one call and keeps everything — including a file that is open. It also fails
/// across volumes, and `%APPDATA%` on a roaming Windows profile really is on another one, so
/// the copy is not a theoretical fallback.
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
