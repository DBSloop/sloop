//! What reset decides, without destroying anything to find out.
//!
//! The survey is the whole of the interesting part: it is what decides whether a PostgreSQL
//! is sloop's to remove, what goes out of the global store, and — the line the owner drew —
//! what stays. Driving the destruction itself needs a throwaway cluster and lives in
//! `cli/tests/reset.rs`.

use std::path::{Path, PathBuf};

use super::survey;

/// A global store on disk, thrown away when it goes out of scope.
struct Store(PathBuf);

impl Store {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "sloop-reset-{}-{label}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a temporary directory should be creatable");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Write a `server.toml` naming an origin, and whatever else a store would hold.
    fn set_up_as(&self, origin: &str) {
        std::fs::write(
            self.0.join("server.toml"),
            format!(
                "version = 2\n\
                 bin = '/usr/lib/postgresql/18/bin'\n\
                 data = '{}/postgres/data'\n\
                 port = 5433\n\
                 superuser = \"postgres\"\n\
                 password = \"keyring\"\n\
                 origin = \"{origin}\"\n",
                self.0.display()
            ),
        )
        .expect("writing the record");
    }

    fn put(&self, relative: &str, what: &str) {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("creatable");
        }
        std::fs::write(path, what).expect("writable");
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// **The hinge of the whole command.** `origin` decides whether the PostgreSQL is sloop's to
/// remove, and getting it backwards would either strand a server sloop installed or delete
/// one it was a guest on.
#[test]
fn a_postgresql_sloop_installed_is_sloops_to_remove_and_one_it_found_is_not() {
    let store = Store::new("origin");
    store.set_up_as("sloop");
    std::fs::create_dir_all(store.path().join("postgres").join("data")).expect("creatable");

    let sloops = survey(store.path()).expect("a survey reads a record");
    assert!(sloops.owns_the_server);
    assert!(
        sloops.going.iter().any(|path| path.ends_with("postgres")),
        "a PostgreSQL sloop installed should go: {:?}",
        sloops.going
    );

    // The same store, the same directory on disk, and the opposite answer.
    store.set_up_as("machine");
    let guest = survey(store.path()).expect("a survey reads a record");
    assert!(!guest.owns_the_server);
    assert!(
        !guest.going.iter().any(|path| path.ends_with("postgres")),
        "sloop is a guest on that server and must not remove it: {:?}",
        guest.going
    );
}

/// **Backups are never deleted.** The owner's line, and the one assertion in this file that
/// is about a promise rather than a mechanism.
#[test]
fn backups_are_kept_and_said_out_loud() {
    let store = Store::new("backups");
    store.set_up_as("sloop");
    store.put(
        "backups/postgres/orders/20260916T031500Z/manifest.json",
        "{}",
    );
    store.put("secrets.sealed", "SLOOPSEC");
    store.put("projects/demo", "/work/demo");

    let what = survey(store.path()).expect("a survey reads a record");

    assert_eq!(
        what.kept,
        vec![store.path().join("backups")],
        "backups should be kept and named"
    );
    for path in &what.going {
        assert!(
            !path.ends_with("backups"),
            "reset would have deleted a backup: {}",
            path.display()
        );
    }
}

/// Everything that *is* sloop's own state goes, named one at a time rather than by sweeping
/// the directory — which is what keeps `backups/` out of it as the store grows.
#[test]
fn the_stores_own_state_goes_and_nothing_else_does() {
    let store = Store::new("state");
    store.set_up_as("machine");
    store.put("secrets.sealed", "SLOOPSEC");
    store.put("projects/demo", "/work/demo");
    store.put("registry.toml", "version = 1\n");
    store.put("backups/keep-me", "a dump");
    // Something sloop did not write. A reset that removed this would be reaching past its
    // own state into a directory somebody else shares with it.
    store.put("notes.txt", "mine");

    let what = survey(store.path()).expect("a survey reads a record");
    let going: Vec<String> = what
        .going
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();

    for wanted in ["server.toml", "secrets.sealed", "registry.toml", "projects"] {
        assert!(
            going.contains(&wanted.to_owned()),
            "{wanted} should go: {going:?}"
        );
    }
    assert!(!going.contains(&"notes.txt".to_owned()), "{going:?}");
    assert!(!going.contains(&"backups".to_owned()), "{going:?}");
}

/// A machine with nothing on it is not an error. `reset` on a machine that was never set up
/// is a sentence, not a failure — the same shape `an_empty_registry_says_what_to_do_about_it`
/// settled for `db list`.
#[test]
fn a_machine_with_nothing_on_it_has_nothing_to_reset() {
    let store = Store::new("nothing");

    let what = survey(store.path()).expect("a survey of an empty store is fine");
    assert!(what.server.is_none());
    assert!(!what.anything());
    assert!(what.going.is_empty());
}

/// A store holding only backups still has nothing to reset — and still keeps them.
#[test]
fn a_machine_that_only_has_backups_keeps_them_and_resets_nothing() {
    let store = Store::new("only-backups");
    store.put("backups/postgres/orders/20260916T031500Z/dump", "…");

    let what = survey(store.path()).expect("a survey is fine");
    assert!(!what.anything(), "there is no state to remove");
    assert_eq!(what.kept, vec![store.path().join("backups")]);
}
