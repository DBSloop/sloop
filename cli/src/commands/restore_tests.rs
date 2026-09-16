//! Which backup a restore picks, and what it refuses to pick.
//!
//! **The choosing is the part that can be tested without a server**, and it is also the part
//! where being wrong is worst: a restore that quietly picks a half-written directory replaces
//! a working database with nothing. What happens once a backup is chosen needs a real
//! cluster and lives in `engine::cluster_tests`.

use std::path::{Path, PathBuf};

use super::choose;
use crate::backup::manifest::{self, Manifest};
use crate::backup::stamp::Stamp;
use crate::backup::store::State;
use crate::backup::{LATEST, directory_for, dump_file, label_dir};
use crate::engine::Engine;
use crate::exit::Exit;

/// 2026-09-16T03:15:00Z.
const NOW: i64 = 1_789_528_500;

/// A day, in seconds.
const DAY: i64 = 24 * 60 * 60;

/// A store of this test's own.
fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sloop-restore-{}-{label}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&path).expect("a temporary directory");
    path
}

/// A finished backup, dump and manifest agreeing.
fn complete(root: &Path, at: i64, contents: &[u8]) -> PathBuf {
    let directory = directory_for(root, Engine::Postgres, "app", Stamp::from_unix_seconds(at));
    write_into(&directory, contents, true);
    directory
}

/// A directory with a dump in it, and a manifest only when asked for one.
fn write_into(directory: &Path, contents: &[u8], with_manifest: bool) {
    std::fs::create_dir_all(directory).expect("a directory");
    std::fs::write(dump_file(directory), contents).expect("a dump");
    if !with_manifest {
        return;
    }

    let taken = Stamp::from_unix_seconds(NOW);
    let manifest = Manifest {
        version: 1,
        sloop: "test".to_owned(),
        label: "app".to_owned(),
        engine: Engine::Postgres,
        source: "postgres://tester@127.0.0.1:55999/app".to_owned(),
        database: "app".to_owned(),
        server: manifest::Server {
            version: "17.9".to_owned(),
            tls: false,
        },
        taken: manifest::Taken {
            utc: taken.utc_iso(),
            local: taken.local().iso(),
            offset: taken.local().offset_label(),
            offset_seconds: taken.local().offset_seconds(),
            unix: taken.unix_seconds(),
        },
        took_seconds: 0.4,
        dump_seconds: 0.3,
        dump: manifest::Dump {
            file: "dump".to_owned(),
            bytes: u64::try_from(contents.len()).expect("a test dump fits"),
            sha256: manifest::checksum(&dump_file(directory)).expect("a checksum"),
            encryption: None,
        },
        rows: 1,
        tables: Vec::new(),
    };
    manifest.write(directory).expect("a manifest");
}

/// **The newest whole one**, which is what somebody in a hurry means by no flag at all.
#[test]
fn with_no_flag_it_takes_the_newest_whole_backup() {
    let root = scratch("newest");
    complete(&root, NOW - 2 * DAY, b"older");
    complete(&root, NOW, b"newest");
    complete(&root, NOW - DAY, b"middle");

    let chosen = choose(&root, "app", None, false).expect("a backup");

    assert_eq!(chosen.taken.unix_seconds(), NOW);
    assert_eq!(chosen.state, State::Complete);

    std::fs::remove_dir_all(&root).ok();
}

/// **A half-written directory is never the default.** It is newer than everything else and
/// it is still not what a restore reaches for.
#[test]
fn a_newer_unfinished_backup_is_skipped_for_the_newest_whole_one() {
    let root = scratch("skip-broken");
    complete(&root, NOW - DAY, b"the good one");
    write_into(
        &directory_for(
            &root,
            Engine::Postgres,
            "app",
            Stamp::from_unix_seconds(NOW),
        ),
        b"half a dump",
        false,
    );

    let chosen = choose(&root, "app", None, false).expect("a backup");

    assert_eq!(chosen.taken.unix_seconds(), NOW - DAY);
    assert_eq!(chosen.state, State::Complete);

    std::fs::remove_dir_all(&root).ok();
}

/// Asked for by name, including `latest` — the name `--replace` keeps its copy under.
#[test]
fn from_takes_the_directorys_own_name() {
    let root = scratch("from");
    complete(&root, NOW - DAY, b"older");
    complete(&root, NOW, b"newest");
    write_into(
        &label_dir(&root, Engine::Postgres, "app").join(LATEST),
        b"the replace copy",
        true,
    );

    let asked = Stamp::from_unix_seconds(NOW - DAY).utc_path();
    let chosen = choose(&root, "app", Some(&asked), false).expect("that one");
    assert_eq!(chosen.taken.unix_seconds(), NOW - DAY);

    let replaced = choose(&root, "app", Some(LATEST), false).expect("the replace copy");
    assert!(replaced.directory.ends_with(LATEST));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_name_that_is_not_there_is_refused_by_name() {
    let root = scratch("missing-from");
    complete(&root, NOW, b"the only one");

    let failure = choose(&root, "app", Some("20200101T000000Z"), false).expect_err("no such one");

    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure.message().contains("20200101T000000Z"),
        "{}",
        failure.message()
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_label_with_nothing_under_it_says_so_and_says_what_to_run() {
    let root = scratch("empty");

    let failure = choose(&root, "app", None, false).expect_err("nothing to restore");

    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("sloop backup")),
        "{:?}",
        failure.hint_text()
    );

    std::fs::remove_dir_all(&root).ok();
}

/// **A damaged backup is refused, and `--force` is the way past it.** Somebody in a disaster
/// with one corrupted backup should be allowed to try it; somebody with a typo should not do
/// it by accident.
#[test]
fn a_damaged_backup_is_refused_until_it_is_forced() {
    let root = scratch("damaged");
    let directory = complete(&root, NOW, b"the whole dump");
    std::fs::write(dump_file(&directory), b"cut").expect("a shorter dump");

    let failure = choose(&root, "app", None, false).expect_err("it does not check out");
    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("--force")),
        "{:?}",
        failure.hint_text()
    );

    let forced = choose(&root, "app", None, true).expect("--force restores it anyway");
    assert!(forced.is_damaged());

    std::fs::remove_dir_all(&root).ok();
}

/// A rewritten dump of the same length: only hashing finds it, and a restore hashes.
#[test]
fn a_restore_checks_the_hash_and_not_only_the_size() {
    let root = scratch("hash");
    let directory = complete(&root, NOW, b"the whole dump");
    std::fs::write(dump_file(&directory), b"the whole dumq").expect("a dump of the same length");

    let failure = choose(&root, "app", None, false).expect_err("the hash has to be checked");

    assert!(
        failure.message().contains("hashes to"),
        "{}",
        failure.message()
    );

    std::fs::remove_dir_all(&root).ok();
}

/// Another database's backups are not this one's.
#[test]
fn only_this_labels_backups_are_considered() {
    let root = scratch("labels");
    complete(&root, NOW, b"app's own");
    write_into(
        &directory_for(
            &root,
            Engine::Postgres,
            "billing",
            Stamp::from_unix_seconds(NOW + DAY),
        ),
        b"billing's",
        true,
    );

    let chosen = choose(&root, "app", None, false).expect("a backup");

    assert_eq!(chosen.label, "app");

    std::fs::remove_dir_all(&root).ok();
}
