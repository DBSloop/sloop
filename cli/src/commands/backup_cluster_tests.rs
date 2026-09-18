//! `sloop backup` against a real PostgreSQL: one created for the test, on a port nothing
//! else uses, destroyed again when the test finishes.
//!
//! The cluster comes from `engine::cluster_tests`, which already knows how to build one
//! and how to take it away again whether the test passed or not. What is new here is
//! everything around it — a registry on disk, a password route, the layout, the manifest
//! and the exit code — because that is what `R9` is.
//!
//! **The password travels the `command:` route**, which is a real one of the four and the
//! only one a test may use. The keyring is shared, real and outside any sandbox, so a test
//! that wrote to it would be writing to the developer's own credential store; the
//! environment route would mean setting a variable in this process, which Rust 2024 makes
//! an `unsafe` call and this workspace forbids outright. So the roles are given plain
//! passwords for the duration of the cluster and `echo` hands them over.

use std::path::{Path, PathBuf};

use super::{Context, Mode, Taken, refuse_to_overwrite, run};
use crate::backup::manifest::{self, Manifest};
use crate::crypt::PrivateKey;
use crate::engine::Engine;
use crate::engine::cluster_tests::{Cluster, skip};
use crate::exit::Exit;
use crate::registry::file::{Database, Encryption, KeyKept, Registry};
use crate::registry::{Registries, Resolution, World, resolve};
use crate::secret::Route;

/// Plain, because it goes through `echo`. What a password may contain is R3's question and
/// is answered by R3's tests against all four routes.
const ALPHA: &str = "alpha-backs-up";
const BETA: &str = "beta-backs-up";

/// The world with no project in it. `--global` never asks it anything; it exists because
/// [`resolve`] takes one.
struct Nowhere;

impl World for Nowhere {
    fn is_directory(&self, _path: &Path) -> bool {
        false
    }
    fn is_project(&self, _dir: &Path) -> bool {
        false
    }
    fn project_named(&self, _name: &str) -> Option<PathBuf> {
        None
    }
    fn boundary(&self) -> Option<&Path> {
        None
    }
}

/// `--global`, so the store is the one directory below and nothing walks up out of it.
fn global_only() -> Resolution {
    resolve(Path::new("."), &Nowhere, true, None, None).expect("--global resolves to itself")
}

/// A keypair the test holds, written into the registry as `key export` would have left it.
///
/// **Nothing is stored anywhere.** Encrypting needs only the public half, so the registry
/// gets that and the test keeps the private one in a variable — which is what lets these
/// tests run without writing a key into the developer's own Credential Manager. Whether a
/// key can be *kept* is a question for `tests/key.rs`, where a sandbox contains the answer.
fn seeded_key(registry: &mut Registry) -> PrivateKey {
    let private = PrivateKey::generate();
    registry.set_encryption(Encryption {
        public_key: private.public(),
        private_key: Route::EncryptedFile,
        // Already copied, so the first backup does not stop to say what losing it costs.
        key_kept: Some(KeyKept::Exported),
    });
    private
}

/// Wait, briefly and with a bound, until `label`'s lock can be taken again.
///
/// **The bound is the point.** Two seconds is far longer than any real gap this matters for,
/// so a lock that is genuinely still held — the failure worth catching — still fails the
/// test; what it absorbs is only the moment between a handle being dropped and the operating
/// system saying so. See `crate::lock::tests::the_lock_goes_when_the_holder_does`, where the
/// same wait is written for the same reason.
fn wait_for_the_lock_to_go(store: &Path, label: &str) {
    let waited = std::time::Instant::now();
    loop {
        match crate::lock::take(store, label, "test") {
            Ok(held) => {
                drop(held);
                if waited.elapsed() > std::time::Duration::from_millis(1) {
                    eprintln!("the {label} lock took {:?} to come free", waited.elapsed());
                }
                return;
            }
            Err(why) => assert!(
                waited.elapsed() < std::time::Duration::from_secs(2),
                "the {label} lock did not go when the run that took it finished: {}",
                why.message()
            ),
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// One entry, as `db add` would have written it.
fn entry(port: u16, database: &str, user: &str, password: &str) -> Database {
    Database {
        engine: Engine::Postgres,
        host: "127.0.0.1".to_owned(),
        port,
        database: database.to_owned(),
        user: user.to_owned(),
        password: Route::Command(format!("echo {password}")),
        reach: crate::ssh::Reach::Direct,
    }
}

/// Where a backup of `label` should have landed, whatever second it was taken in.
fn stored(store: &Path, label: &str) -> Vec<PathBuf> {
    let directory = store.join("backups").join("postgres").join(label);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    found.sort();
    found
}

/// Everything a manifest says that can be checked against the directory it is sitting in.
///
/// Separate from the assertions about *this* fixture's rows, because these are true of
/// every backup sloop takes and the ones in the test are true of this one cluster.
fn describes_what_is_beside_it(directory: &Path, label: &str) -> Manifest {
    let dump = directory.join("dump.age");
    assert!(dump.is_file(), "no dump at {}", dump.display());
    assert!(
        manifest::completed(directory),
        "no manifest beside the dump"
    );

    // The directory is the UTC stamp, which is what makes a listing sort chronologically.
    let stamp = directory
        .file_name()
        .expect("a directory name")
        .to_string_lossy()
        .into_owned();
    assert_eq!(stamp.len(), 16, "{stamp} is not a UTC stamp");
    assert!(stamp.ends_with('Z'), "{stamp} is not in UTC");

    let manifest = Manifest::read(&manifest::path_in(directory)).expect("reading the manifest");
    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.label, label);
    assert_eq!(manifest.engine, Engine::Postgres);
    assert!(
        manifest.server.version.contains('.'),
        "the server's version is not recorded: {}",
        manifest.server.version
    );

    // The size and the checksum describe the file actually sitting beside it — which is
    // the encrypted one, so `backups list` can check it without holding the key.
    let on_disk = std::fs::metadata(&dump).expect("the dump is there").len();
    assert_eq!(manifest.dump.file, "dump.age");
    assert_eq!(manifest.dump.bytes, on_disk);
    assert!(on_disk > 0, "the dump is empty");
    assert_eq!(
        manifest.dump.sha256,
        manifest::checksum(&dump).expect("hashing the dump"),
        "the checksum is not this file's"
    );
    assert_eq!(manifest.dump.sha256.len(), 64);

    // Both timestamps, and the offset that reconciles them.
    assert_eq!(
        manifest.taken.utc.replace(['-', ':'], ""),
        stamp,
        "the directory name and the manifest disagree about when this was taken"
    );
    assert_eq!(
        manifest.taken.offset,
        manifest.taken_locally().offset_label()
    );
    assert!(
        manifest.taken.local.ends_with(&manifest.taken.offset),
        "the local time does not carry its offset: {}",
        manifest.taken.local
    );
    assert!(manifest.took_seconds >= manifest.dump_seconds);

    manifest
}

/// R11's "Done when", against a backup that was really taken.
///
/// The private key was never stored on this machine at all — the test is holding it — and
/// what comes out of the file is the archive `pg_restore` would have read if nothing had
/// been encrypted. A key that is not this one is refused with a sentence.
fn only_the_key_opens_it(directory: &Path, manifest: &Manifest, private: &PrivateKey) {
    // archive `pg_restore` would have read if nothing had been encrypted.
    let sealed = directory.join("dump.age");
    let head = std::fs::read(&sealed).expect("reading the encrypted dump");
    assert!(
        head.starts_with(
            b"age-encryption.org/v1
"
        ),
        "the dump is not an age file"
    );
    assert!(
        !head.starts_with(b"PGDMP"),
        "the dump is sitting there in the clear"
    );
    assert_eq!(
        manifest
            .dump
            .encryption
            .as_ref()
            .map(|sealed| (sealed.format.clone(), sealed.recipient.clone())),
        Some(("age".to_owned(), private.public().to_string())),
        "the manifest does not say which key opens this"
    );

    let mut plain = Vec::new();
    std::io::Read::read_to_end(
        &mut crate::crypt::opened(&sealed, private).expect("opening it with the key"),
        &mut plain,
    )
    .expect("reading the plaintext");
    assert!(
        plain.starts_with(b"PGDMP"),
        "what came out is not a PostgreSQL archive"
    );
    assert!(plain.len() > 1000, "only {} bytes came out", plain.len());

    // And a key that is not this one is refused with a sentence rather than a panic — the
    // other half of R11's "Done when".
    let stranger = PrivateKey::generate();
    let refused = crate::crypt::opened(&sealed, &stranger);
    let refused = refused.err().expect("a stranger's key opened the backup");
    assert_eq!(refused.exit().code(), 5, "{refused:?}");
    assert!(
        refused
            .message()
            .contains("no key on this machine can open it"),
        "{refused:?}"
    );
}
/// The whole of R9's "Done when", in order, against one cluster.
///
/// Three databases, one of them deliberately unreachable and first in the alphabet so that
/// the two after it only get backed up if `--all` really does carry on past a failure.
#[test]
fn every_database_is_backed_up_and_a_broken_one_does_not_stop_the_rest() {
    let Some(cluster) = Cluster::start("backup") else {
        skip("initdb is not on this machine");
        return;
    };

    // The fixture's roles have deliberately awful passwords, which cannot survive `echo`.
    // Changed here rather than in the shared fixture, so R4's tests keep testing what they
    // were written to test.
    let simplified = cluster.psql(
        "postgres",
        &format!("ALTER ROLE alpha PASSWORD '{ALPHA}'; ALTER ROLE beta PASSWORD '{BETA}';"),
    );
    if let Err(why) = simplified {
        skip(&format!("this server would not take the fixture: {why}"));
        return;
    }

    let store = cluster.root.join("store");
    let mut registry = Registry::default();
    let private = seeded_key(&mut registry);
    // Alphabetical, because that is the order `--all` walks them in: the unreachable one
    // is met first and the other two prove the run did not stop there.
    registry.insert("broken".to_owned(), entry(1, "source_db", "alpha", ALPHA));
    registry.insert(
        "empty".to_owned(),
        entry(cluster.port, "target_db", "beta", BETA),
    );
    registry.insert(
        "orders".to_owned(),
        entry(cluster.port, "source_db", "alpha", ALPHA),
    );
    // Built in hand rather than written to a file and read back. `R19c4` put the registry
    // in PostgreSQL, and what these two tests are about is `backup --all` — giving each of
    // them a `sloop_database` of its own would be a second cluster to prove something
    // `registry::cluster_tests` proves once.
    let registries = Registries::of(global_only(), &store, registry, None);
    // Nothing here is reached over SSH, so no forward is ever opened — but a `Context`
    // needs one, because every command shares the set the run holds.
    let tunnels = crate::ssh::tunnel::Tunnels::new().expect("this binary knows where it is");
    let mut context = Context {
        registries,
        global: &store,
        password_command: None,
        tunnels: &tunnels,
    };

    // --- --all, with one of the three unreachable ------------------------------------
    let exit =
        run(&mut context, None, true, Mode::Sequential).expect("--all reports rather than stops");
    assert_eq!(
        exit,
        Exit::Connect,
        "one unreachable server, so the run exits with the code that says so"
    );

    // --- the two that could be reached are there, and the one that could not is not ---
    assert!(
        stored(&store, "broken").is_empty(),
        "a failed backup left a directory behind: {:?}",
        stored(&store, "broken")
    );

    let orders = stored(&store, "orders");
    assert_eq!(orders.len(), 1, "orders: {orders:?}");
    let empty = stored(&store, "empty");
    assert_eq!(empty.len(), 1, "empty: {empty:?}");

    // --- the layout, and the manifest inside it ---------------------------------------
    let directory = &orders[0];
    let manifest = describes_what_is_beside_it(directory, "orders");
    describes_the_source(&manifest, cluster.port);

    // --- the encrypted dump is a real dump, and the key is what opens it --------------
    only_the_key_opens_it(directory, &manifest, &private);

    // --- an empty database is a backup, not a failure ---------------------------------
    let nothing = describes_what_is_beside_it(&empty[0], "empty");
    assert_eq!(nothing.rows, 0);
    assert!(nothing.tables.is_empty());
    assert!(nothing.dump.bytes > 0, "even an empty dump has a header");

    // --- a second run in the same second does not write over the first ------------------
    // Whether it *is* the same second depends on how fast this machine is, so both answers
    // are accepted; what is not accepted is the first backup being replaced. One database
    // by name is exercised end to end by the test below.
    //
    // **The lock the run above took has to be observably gone first**, and that wait is not
    // politeness. `lock::tests::the_lock_goes_when_the_holder_does` already wrote this down
    // for macOS: sloop guarantees the lock goes when the handle does, but that the *next*
    // attempt in the same process sees it gone within zero nanoseconds is a timing property
    // no operating system promises, and a loaded parallel test run is exactly where it is
    // not provided. Without the wait this test asked its question of the kernel's scheduling
    // instead of of `backup`, and answered `7` — which is how `test (ubuntu-latest)` went red
    // on a commit that had changed nothing about locking.
    wait_for_the_lock_to_go(&store, "orders");

    let again = run(&mut context, Some("orders"), false, Mode::Sequential);
    let after = stored(&store, "orders");
    match again {
        Ok(exit) => {
            assert_eq!(exit, Exit::Success);
            assert_eq!(
                after.len(),
                2,
                "a second backup replaced the first: {after:?}"
            );
        }
        Err(failure) => {
            assert_eq!(failure.exit(), Exit::Failure, "{failure:?}");
            assert_eq!(after, orders, "it refused and changed something anyway");
        }
    }
    assert_eq!(
        Manifest::read(&manifest::path_in(directory)).expect("the first manifest is still there"),
        manifest,
        "the backup that was already there was not left alone"
    );

    // --- an unknown name is usage, and nothing is written ------------------------------
    let unknown = run(&mut context, Some("nope"), false, Mode::Sequential)
        .expect_err("nothing is registered as nope");
    assert_eq!(unknown.exit(), Exit::Usage);
    assert!(stored(&store, "nope").is_empty());
}

/// What the manifest says about where the backup came from.
fn describes_the_source(manifest: &Manifest, port: u16) {
    assert_eq!(manifest.database, "source_db");
    assert_eq!(
        manifest.source,
        format!("postgres://alpha@127.0.0.1:{port}/source_db"),
        "the source is the connection a server's own log would show"
    );
    assert!(
        !manifest.source.contains(ALPHA),
        "the password reached the manifest"
    );

    // Exact counts, and the fixture's own numbers — not an estimate and not a total that
    // happens to add up.
    assert_eq!(
        manifest
            .tables
            .iter()
            .map(|table| (format!("{}.{}", table.schema, table.name), table.rows))
            .collect::<Vec<_>>(),
        vec![
            ("app.widgets".to_owned(), 500),
            ("public.notes".to_owned(), 7),
        ],
    );
    assert_eq!(manifest.rows, 507);
}

/// What a person is shown after a backup, which is local time and never UTC dressed up as
/// it. Checked on the lines themselves rather than on a captured stream, because what
/// matters is the sentence and not which file descriptor it went to.
#[test]
fn what_it_prints_is_in_local_time_and_names_where_the_backup_went() {
    let Some(cluster) = Cluster::start("backup-display") else {
        skip("initdb is not on this machine");
        return;
    };

    if let Err(why) = cluster.psql("postgres", &format!("ALTER ROLE alpha PASSWORD '{ALPHA}';")) {
        skip(&format!("this server would not take the fixture: {why}"));
        return;
    }

    let store = cluster.root.join("store");
    let mut registry = Registry::default();
    let _private = seeded_key(&mut registry);
    registry.insert(
        "orders".to_owned(),
        entry(cluster.port, "source_db", "alpha", ALPHA),
    );
    // Built in hand rather than written to a file and read back. `R19c4` put the registry
    // in PostgreSQL, and what these two tests are about is `backup --all` — giving each of
    // them a `sloop_database` of its own would be a second cluster to prove something
    // `registry::cluster_tests` proves once.
    let registries = Registries::of(global_only(), &store, registry, None);
    // Nothing here is reached over SSH, so no forward is ever opened — but a `Context`
    // needs one, because every command shares the set the run holds.
    let tunnels = crate::ssh::tunnel::Tunnels::new().expect("this binary knows where it is");
    let mut context = Context {
        registries,
        global: &store,
        password_command: None,
        tunnels: &tunnels,
    };

    assert_eq!(
        run(&mut context, Some("orders"), false, Mode::Sequential).expect("backing it up"),
        Exit::Success
    );

    let directory = stored(&store, "orders").pop().expect("one backup");
    let manifest = Manifest::read(&manifest::path_in(&directory)).expect("reading the manifest");
    let taken = Taken {
        directory: directory.clone(),
        manifest,
    };
    let lines = taken.describe();
    let said = lines.join("\n");

    assert!(
        said.contains(&directory.display().to_string()),
        "it never says where the backup went:\n{said}"
    );
    assert!(said.contains("sha256 "), "no checksum in it:\n{said}");

    // The local time, with its offset, and the UTC one beside it — which is what lets
    // somebody match what they are reading against the directory name.
    let local = taken.manifest.taken_locally();
    assert!(
        said.contains(&local.readable()),
        "the local time is not in it:\n{said}"
    );
    assert!(
        said.contains(&local.offset_label()),
        "the offset is not in it:\n{said}"
    );
    assert!(
        said.contains(&taken.manifest.taken.utc),
        "the UTC stamp is not in it:\n{said}"
    );
    assert!(
        !said.contains(ALPHA),
        "the password is in what gets printed:\n{said}"
    );
}

/// A finished backup is never written over, however fast the next run arrives.
///
/// Driven directly rather than by taking two backups in a row, because whether two runs
/// land in the same second depends on how fast the machine is — and a test that only
/// sometimes exercises the thing it was written for is not a test.
#[test]
fn a_backup_that_is_already_there_is_refused_rather_than_replaced() {
    let directory = std::env::temp_dir().join(format!(
        "sloop-backup-guard-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&directory).expect("a temporary directory");

    // A half-written one — a run that was killed after the dump and before the manifest —
    // is written over, because there is nothing there worth keeping.
    std::fs::write(directory.join("dump"), b"half a dump").expect("writing a dump");
    refuse_to_overwrite(&directory, "orders")
        .expect("an unfinished backup is not something to protect");

    // A finished one is not.
    std::fs::write(manifest::path_in(&directory), b"{}").expect("writing a manifest");
    let refused =
        refuse_to_overwrite(&directory, "orders").expect_err("it overwrote a finished backup");
    assert_eq!(refused.exit(), Exit::Failure);
    assert!(refused.message().contains("orders"), "{refused:?}");
    assert!(
        refused.message().contains(&directory.display().to_string()),
        "{refused:?}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}
