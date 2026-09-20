//! What reset decides, without destroying anything to find out.
//!
//! The survey is the whole of the interesting part: it is what decides whether a PostgreSQL
//! is sloop's to remove, what goes out of the global store, and — the line the owner drew —
//! what stays. Driving the destruction itself needs a throwaway cluster and lives in
//! `cli/tests/reset.rs`.

use std::path::{Path, PathBuf};

use super::{What, surveying};

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

    /// The same, with the `[database]` block a machine that has finished Setup carries.
    ///
    /// **Separate because the two states are different.** A record without it is a Setup that
    /// got as far as the cluster; one with it is a machine that has sloop's own database, and
    /// therefore a second password to forget.
    fn set_up_with_a_database(&self, origin: &str) {
        self.set_up_as(origin);
        let mut record = std::fs::read_to_string(self.0.join("server.toml")).expect("readable");
        record.push_str(
            "
[database]
name = \"sloop_database\"
role = \"sloop_db_admin\"
             password = \"keyring\"
",
        );
        std::fs::write(self.0.join("server.toml"), record).expect("writing the record");
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

    let sloops = surveying(store.path(), None).expect("a survey reads a record");
    assert!(sloops.owns_the_server);
    assert!(
        sloops.going.iter().any(|path| path.ends_with("postgres")),
        "a PostgreSQL sloop installed should go: {:?}",
        sloops.going
    );

    // The same store, the same directory on disk, and the opposite answer.
    store.set_up_as("machine");
    let guest = surveying(store.path(), None).expect("a survey reads a record");
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

    let what = surveying(store.path(), None).expect("a survey reads a record");

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

    let what = surveying(store.path(), None).expect("a survey reads a record");
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

    let what = surveying(store.path(), None).expect("a survey of an empty store is fine");
    assert!(what.server.is_none());
    assert!(!what.anything());
    assert!(what.going.is_empty());
}

/// A store holding only backups still has nothing to reset — and still keeps them.
#[test]
fn a_machine_that_only_has_backups_keeps_them_and_resets_nothing() {
    let store = Store::new("only-backups");
    store.put("backups/postgres/orders/20260916T031500Z/dump", "…");

    let what = surveying(store.path(), None).expect("a survey is fine");
    assert!(!what.anything(), "there is no state to remove");
    assert_eq!(what.kept, vec![store.path().join("backups")]);
}

/// **A reset that left a password behind was not a reset.**
///
/// The owner's report, and the reason this test exists: a machine that had been reset still
/// held `sloop_db_admin`'s password in the keyring, so the next `sloop setup` built a new
/// cluster and every command afterwards authenticated against a server that no longer
/// existed — *"password authentication failed"* for the role sloop owns, on a machine whose
/// whole point was that it had been wiped.
///
/// The two named here are the two that strand a machine, and both are derived from
/// `server.toml` rather than from the registry — so they are found even when nothing else on
/// the machine can be.
#[test]
fn a_reset_forgets_the_passwords_that_would_outlive_the_server() {
    let store = Store::new("passwords");
    store.set_up_with_a_database("sloop");

    let what = surveying(store.path(), None).expect("a survey reads a record");
    let named: Vec<&str> = what
        .passwords
        .iter()
        .map(|kept| kept.key.as_str())
        .collect();

    assert!(
        named.iter().any(|key| key.contains("postgres")),
        "the superuser's password should go: {named:?}"
    );
    assert!(
        named.iter().any(|key| key.contains("sloop_db_admin")),
        "sloop_db_admin's password should go: {named:?}"
    );
    assert!(
        what.anything(),
        "a machine with passwords to forget has something to reset"
    );
}

/// **And the one it must not forget.** The private half of the backup keypair is filed under
/// `backup-key:<public>`, and it is the only thing standing between a kept backup and an
/// unreadable one. Deleting it while keeping the backups would be deleting the backups the
/// slow way — so nothing named here may ever carry that prefix.
#[test]
fn a_reset_never_forgets_the_backup_key() {
    let store = Store::new("backup-key");
    store.set_up_as("sloop");
    store.put(
        "backups/postgres/orders/20260916T031500Z/manifest.json",
        "{}",
    );

    let what = surveying(store.path(), None).expect("a survey reads a record");

    for kept in &what.passwords {
        assert!(
            !kept.key.starts_with("backup-key:"),
            "reset would have forgotten a backup key: {}",
            kept.key
        );
    }
}

/// A key named twice is a key deleted twice, which is a second `NoEntry` nobody needs to read.
#[test]
fn no_password_is_named_more_than_once() {
    let store = Store::new("dedup");
    store.set_up_as("sloop");

    let what = surveying(store.path(), None).expect("a survey reads a record");
    let mut keys: Vec<&str> = what
        .passwords
        .iter()
        .map(|kept| kept.key.as_str())
        .collect();
    let before = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(before, keys.len(), "a key was named twice: {keys:?}");
}

/// **A machine whose only sloop left is the background service still has something to reset.**
///
/// The owner's rule that reset clears everything sloop put here includes the registration
/// that runs it at boot — and a machine with no store, no cluster and no passwords can still
/// have one, which is exactly the machine that would otherwise be told there is nothing to do
/// while sloop kept starting every morning against a store that is gone.
#[test]
fn a_registered_service_is_something_to_reset_on_its_own() {
    let bare = What {
        global: std::path::PathBuf::from("/nowhere"),
        server: None,
        owns_the_server: false,
        going: Vec::new(),
        kept: Vec::new(),
        passwords: Vec::new(),
        some_unknown: false,
        service: None,
        installed: Vec::new(),
    };
    assert!(
        !bare.anything(),
        "a machine with nothing on it has nothing to reset"
    );

    let with_a_service = What {
        service: crate::service::mechanism::Mechanism::of_this_machine(),
        ..bare
    };
    // Every platform this runs on has one of the three mechanisms, so this is not a
    // conditional assertion — it is the whole point of the field.
    assert!(
        with_a_service.service.is_some(),
        "this machine has no service mechanism at all"
    );
    assert!(
        with_a_service.anything(),
        "a registered service is something to reset"
    );
}
