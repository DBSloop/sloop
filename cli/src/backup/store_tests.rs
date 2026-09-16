//! Reading a store, and the retention arithmetic.
//!
//! **Built on a real directory tree, not a mock.** What this module is for is the gap
//! between what `backup` wrote and what is actually on the disk — a manifest that never
//! arrived, a dump that was truncated, a directory somebody dropped in by hand — and none
//! of those are things a fake filesystem would have been made to do.
//!
//! The clock is passed in everywhere, so every retention case below is exact rather than
//! "roughly thirty days ago, depending on when the suite ran".

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Check, Found, GRACE, Held, Policy, State, Stored, label_for, parse_age, plan, scan};
use crate::backup::stamp::Stamp;
use crate::backup::{directory_for, dump_file, manifest};
use crate::engine::Engine;

/// A temporary store of this test's own.
fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sloop-store-{}-{label}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&path).expect("a temporary directory");
    path
}

/// 2026-09-16T03:15:00Z, the moment this project's own examples use.
const NOW: i64 = 1_789_528_500;

/// A day, in seconds.
const DAY: i64 = 24 * 60 * 60;

/// Write a complete backup — dump, then a manifest that agrees with it.
fn complete(root: &Path, label: &str, at: i64, contents: &[u8]) -> PathBuf {
    let directory = written(root, label, at, contents);
    manifest_for(&directory, label, contents, "dump");
    directory
}

/// Write the directory and the dump, and no manifest: a run that died.
fn written(root: &Path, label: &str, at: i64, contents: &[u8]) -> PathBuf {
    let directory = directory_for(root, Engine::Postgres, label, Stamp::from_unix_seconds(at));
    std::fs::create_dir_all(&directory).expect("a backup directory");
    std::fs::write(dump_file(&directory), contents).expect("a dump");
    directory
}

/// A manifest describing a dump, written the way `backup` writes one.
fn manifest_for(directory: &Path, label: &str, contents: &[u8], file: &str) {
    let dump = directory.join(file);
    let sha256 = manifest::checksum(&dump).expect("a checksum");
    let taken = Stamp::from_unix_seconds(NOW);

    let manifest = manifest::Manifest {
        version: 1,
        sloop: "test".to_owned(),
        label: label.to_owned(),
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
        took_seconds: 0.5,
        dump_seconds: 0.4,
        dump: manifest::Dump {
            file: file.to_owned(),
            bytes: u64::try_from(contents.len()).expect("a test dump fits"),
            sha256,
            encryption: None,
        },
        rows: 3,
        tables: Vec::new(),
    };

    manifest.write(directory).expect("a manifest");
}

/// Read a store and nothing else.
fn read(root: &Path) -> Found {
    scan(root, Check::Size).expect("a readable store")
}

#[test]
fn a_finished_backup_reads_back_as_one() {
    let root = scratch("complete");
    complete(&root, "app", NOW, b"the dump");

    let found = read(&root);

    assert_eq!(found.backups.len(), 1);
    let stored = &found.backups[0];
    assert_eq!(stored.label, "app");
    assert_eq!(stored.engine, Engine::Postgres);
    assert_eq!(stored.state, State::Complete);
    assert!(stored.is_complete());
    assert_eq!(stored.taken.unix_seconds(), NOW);
    assert_eq!(stored.bytes(), 8);
    assert!(stored.problem().is_none());

    std::fs::remove_dir_all(&root).ok();
}

/// **The distinction the whole module exists for.** The manifest is written last, so a
/// directory without one is a run that was killed — and it is reported as that rather than
/// counted as a backup somebody could restore from.
#[test]
fn a_half_written_backup_is_reported_and_not_counted() {
    let root = scratch("unfinished");
    written(&root, "app", NOW, b"half a dump");

    let found = read(&root);

    assert_eq!(found.backups.len(), 1);
    let stored = &found.backups[0];
    assert_eq!(stored.state, State::Unfinished);
    assert!(!stored.is_complete());
    assert!(!stored.is_damaged());
    assert!(
        stored
            .problem()
            .is_some_and(|said| said.contains("no manifest")),
        "{:?} never said why",
        stored.problem()
    );
    // Still measurable: what is on the disk is what a prune would free.
    assert_eq!(stored.bytes(), 11);

    std::fs::remove_dir_all(&root).ok();
}

/// A dump that was truncated after its manifest was written — the corruption a listing can
/// find for the price of a `stat`.
#[test]
fn a_truncated_dump_is_damaged_and_says_both_sizes() {
    let root = scratch("truncated");
    let directory = complete(&root, "app", NOW, b"the whole dump");
    std::fs::write(dump_file(&directory), b"cut").expect("a shorter dump");

    let found = read(&root);
    let stored = &found.backups[0];

    assert!(stored.is_damaged(), "{:?}", stored.state);
    let said = stored.problem().expect("a reason");
    assert!(said.contains('3') && said.contains("14"), "{said}");

    std::fs::remove_dir_all(&root).ok();
}

/// The same bytes, a different file: only hashing finds this one, which is why `--check`
/// exists and why a plain listing does not pretend to have done it.
#[test]
fn a_rewritten_dump_of_the_same_length_is_only_found_by_hashing() {
    let root = scratch("rewritten");
    let directory = complete(&root, "app", NOW, b"the whole dump");
    std::fs::write(dump_file(&directory), b"the whole dumq").expect("a dump");

    assert!(
        !scan(&root, Check::Size).expect("a store").backups[0].is_damaged(),
        "a size check cannot see a rewrite of the same length"
    );
    assert!(
        scan(&root, Check::Checksum).expect("a store").backups[0].is_damaged(),
        "hashing has to see it"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// A manifest nobody can parse is a finding about that backup, not the end of the listing.
#[test]
fn an_unreadable_manifest_does_not_stop_the_others_being_listed() {
    let root = scratch("unreadable");
    let broken = complete(&root, "app", NOW - DAY, b"one");
    complete(&root, "app", NOW, b"two");
    std::fs::write(manifest::path_in(&broken), b"{ not json").expect("a broken manifest");

    let found = read(&root);

    assert_eq!(found.backups.len(), 2);
    assert!(matches!(found.backups[0].state, State::Complete));
    assert!(matches!(found.backups[1].state, State::Unreadable(_)));

    std::fs::remove_dir_all(&root).ok();
}

/// Newest first, which is the order a listing reads in.
#[test]
fn backups_come_back_newest_first() {
    let root = scratch("order");
    complete(&root, "app", NOW - 2 * DAY, b"oldest");
    complete(&root, "app", NOW, b"newest");
    complete(&root, "app", NOW - DAY, b"middle");

    let found = read(&root);
    let order: Vec<i64> = found
        .backups
        .iter()
        .map(|stored| stored.taken.unix_seconds())
        .collect();

    assert_eq!(order, vec![NOW, NOW - DAY, NOW - 2 * DAY]);

    std::fs::remove_dir_all(&root).ok();
}

/// Something that is not a backup at all: reported, never deleted, never counted.
#[test]
fn a_directory_sloop_did_not_write_is_reported_as_a_stray() {
    let root = scratch("strays");
    complete(&root, "app", NOW, b"a dump");
    let label = root.join("backups").join("postgres").join("app");
    std::fs::create_dir_all(label.join("yesterday")).expect("a hand-made directory");
    std::fs::write(label.join("notes.txt"), b"mine").expect("a file");

    let found = read(&root);

    assert_eq!(found.backups.len(), 1);
    assert_eq!(found.strays.len(), 2);
    assert!(found.strays.iter().all(|path| path.starts_with(&label)));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_store_with_nothing_in_it_is_not_an_error() {
    let root = scratch("empty");

    let found = read(&root);

    assert!(found.backups.is_empty());
    assert!(found.strays.is_empty());

    std::fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------------------

/// A complete backup, described without touching a disk.
fn stored_at(label: &str, at: i64, state: State) -> Stored {
    Stored {
        engine: Engine::Postgres,
        label: label.to_owned(),
        directory: PathBuf::from(format!("/nowhere/{label}/{at}")),
        taken: Stamp::from_unix_seconds(at),
        manifest: None,
        state,
    }
}

/// Newest first, the order [`scan`] hands to [`plan`].
fn newest_first(mut backups: Vec<Stored>) -> Vec<Stored> {
    backups.sort_by_key(|stored| std::cmp::Reverse(stored.taken));
    backups
}

/// **`--keep N` keeps exactly N.** Not N plus the one being written, not N per store.
#[test]
fn a_keep_count_keeps_exactly_that_many() {
    let backups = newest_first(
        (0..10)
            .map(|day| stored_at("app", NOW - i64::from(day) * DAY, State::Complete))
            .collect(),
    );

    let decided = plan(
        backups,
        Stamp::from_unix_seconds(NOW),
        &Policy {
            keep: Some(3),
            ..Policy::default()
        },
    );

    assert_eq!(decided.held.len(), 3);
    assert_eq!(decided.remove.len(), 7);
    // And it is the newest three that stayed.
    let kept: Vec<i64> = decided
        .held
        .iter()
        .map(|(stored, _)| stored.taken.unix_seconds())
        .collect();
    assert_eq!(kept, vec![NOW, NOW - DAY, NOW - 2 * DAY]);
    assert!(decided.held.iter().all(|(_, why)| *why == Held::Newest));
}

/// Retention is per database. Three of everything, not three in total.
#[test]
fn a_keep_count_applies_to_each_database_separately() {
    let mut backups = Vec::new();
    for day in 0..5 {
        backups.push(stored_at(
            "app",
            NOW - i64::from(day) * DAY,
            State::Complete,
        ));
        backups.push(stored_at(
            "billing",
            NOW - i64::from(day) * DAY,
            State::Complete,
        ));
    }

    let decided = plan(
        newest_first(backups),
        Stamp::from_unix_seconds(NOW),
        &Policy {
            keep: Some(2),
            ..Policy::default()
        },
    );

    for label in ["app", "billing"] {
        let kept = decided
            .held
            .iter()
            .filter(|(stored, _)| stored.label == label)
            .count();
        assert_eq!(kept, 2, "{label}");
    }
    assert_eq!(decided.remove.len(), 6);
}

#[test]
fn an_age_removes_what_is_older_and_nothing_else() {
    let backups = newest_first(vec![
        stored_at("app", NOW, State::Complete),
        stored_at("app", NOW - 29 * DAY, State::Complete),
        stored_at("app", NOW - 31 * DAY, State::Complete),
    ]);

    let decided = plan(
        backups,
        Stamp::from_unix_seconds(NOW),
        &Policy {
            older_than: Some(Duration::from_secs(30 * 24 * 60 * 60)),
            ..Policy::default()
        },
    );

    assert_eq!(decided.remove.len(), 1);
    assert_eq!(decided.remove[0].taken.unix_seconds(), NOW - 31 * DAY);
    assert!(decided.held.iter().all(|(_, why)| *why == Held::Young));
}

/// **The policy a crontab actually wants.** Throw away last month, but never leave me with
/// fewer than three — so the floor wins over the age.
#[test]
fn a_keep_count_is_a_floor_underneath_an_age() {
    let backups = newest_first(
        (0..6)
            .map(|month| stored_at("app", NOW - i64::from(month) * 40 * DAY, State::Complete))
            .collect(),
    );

    let decided = plan(
        backups,
        Stamp::from_unix_seconds(NOW),
        &Policy {
            keep: Some(3),
            older_than: Some(Duration::from_secs(30 * 24 * 60 * 60)),
            ..Policy::default()
        },
    );

    // Every one of them is older than thirty days except the first, and three survive
    // anyway.
    assert_eq!(decided.held.len(), 3);
    assert_eq!(decided.remove.len(), 3);
    assert_eq!(decided.held[0].1, Held::Newest);
}

/// A broken directory does not occupy one of the kept slots. "Keep three" means three
/// backups a restore could use, not three directories.
#[test]
fn a_broken_directory_does_not_use_up_a_kept_slot() {
    let backups = newest_first(vec![
        stored_at("app", NOW, State::Unfinished),
        stored_at("app", NOW - DAY, State::Complete),
        stored_at("app", NOW - 2 * DAY, State::Complete),
        stored_at("app", NOW - 3 * DAY, State::Complete),
    ]);

    let decided = plan(
        backups,
        Stamp::from_unix_seconds(NOW),
        &Policy {
            keep: Some(2),
            ..Policy::default()
        },
    );

    let complete_kept = decided
        .held
        .iter()
        .filter(|(stored, _)| stored.is_complete())
        .count();
    assert_eq!(complete_kept, 2);
    assert_eq!(decided.remove.len(), 1);
}

/// **Left alone by default**, and named rather than silently skipped.
#[test]
fn broken_directories_are_kept_unless_asked_for() {
    let old = NOW - 10 * DAY;
    let backups = vec![stored_at("app", old, State::Unfinished)];

    let kept = plan(
        backups.clone(),
        Stamp::from_unix_seconds(NOW),
        &Policy {
            keep: Some(1),
            ..Policy::default()
        },
    );
    assert!(kept.remove.is_empty());
    assert_eq!(kept.held[0].1, Held::Broken);
    assert!(kept.held[0].1.why().contains("--include-broken"));

    let swept = plan(
        backups,
        Stamp::from_unix_seconds(NOW),
        &Policy {
            keep: Some(1),
            include_broken: true,
            ..Policy::default()
        },
    );
    assert_eq!(swept.remove.len(), 1);
}

/// **The race `prune` must not lose.** A backup being written right now has no manifest
/// either, and deleting one mid-dump would be this command destroying the thing it is
/// supposed to be tidying up after.
#[test]
fn a_backup_that_may_still_be_running_is_never_removed() {
    let just_now = NOW - 60;
    let decided = plan(
        vec![stored_at("app", just_now, State::Unfinished)],
        Stamp::from_unix_seconds(NOW),
        &Policy {
            keep: Some(0),
            include_broken: true,
            ..Policy::default()
        },
    );

    assert!(decided.remove.is_empty(), "a running backup was removed");
    assert_eq!(decided.held[0].1, Held::MaybeRunning);

    // And once it is past the grace period, the same directory does go.
    let past = plan(
        vec![stored_at(
            "app",
            NOW - i64::try_from(GRACE.as_secs()).expect("an hour fits") - 1,
            State::Unfinished,
        )],
        Stamp::from_unix_seconds(NOW),
        &Policy {
            keep: Some(0),
            include_broken: true,
            ..Policy::default()
        },
    );
    assert_eq!(past.remove.len(), 1);
}

#[test]
fn a_policy_with_no_rule_is_refused_by_the_command_not_here() {
    // The command refuses this before it gets here; the arithmetic still has to be safe,
    // because "no rule" must never read as "everything".
    let decided = plan(
        vec![stored_at("app", NOW - 500 * DAY, State::Complete)],
        Stamp::from_unix_seconds(NOW),
        &Policy::default(),
    );

    assert_eq!(
        decided.remove.len(),
        1,
        "a bare policy is a keep-nothing one"
    );
}

// ---------------------------------------------------------------------------------------
// The age a flag takes
// ---------------------------------------------------------------------------------------

#[test]
fn an_age_reads_the_units_it_documents() {
    for (typed, seconds) in [
        ("12h", 12 * 60 * 60),
        ("1d", 24 * 60 * 60),
        ("30d", 30 * 24 * 60 * 60),
        ("6w", 6 * 7 * 24 * 60 * 60),
        (" 7d ", 7 * 24 * 60 * 60),
    ] {
        assert_eq!(parse_age(typed).expect(typed).as_secs(), seconds, "{typed}");
    }
}

/// **`m` is refused on purpose** — minutes to half the world and months to the other half.
#[test]
fn an_age_refuses_what_it_cannot_mean_exactly() {
    for typed in ["30", "30m", "1y", "d", "-1d", "1.5d", "thirty days", ""] {
        let failure = parse_age(typed).expect_err(typed);
        assert_eq!(failure.exit(), crate::exit::Exit::Usage, "{typed}");
        assert!(
            failure.hint_text().is_some_and(|hint| hint.contains("30d")),
            "{typed} was refused without showing the shape"
        );
    }
}

/// A label that had to be made safe for a path is found by the name somebody registered.
#[test]
fn a_filter_finds_the_directory_a_name_produced() {
    assert_eq!(label_for("my/app"), "my_app");
    assert_eq!(label_for("app"), "app");
}
