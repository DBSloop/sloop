//! The layout and the stamp, both of which are pure functions and neither of which is
//! allowed to depend on the machine's clock or its timezone.

use std::path::{Path, PathBuf};

use super::stamp::{Local, Stamp};
use super::{directory_for, dump_file, sanitise};
use crate::engine::Engine;

/// Known moments, against a calendar.
///
/// The point of writing the arithmetic by hand is that it has to be checked against
/// something that did not come from the same head. Every row below was cross-checked
/// against an independent calendar, and two of them are constants anybody can recognise:
/// `2_147_483_647` is where a 32-bit clock stops, and `951_868_800` is 2000-03-01.
#[test]
fn a_moment_becomes_the_directory_name_it_should() {
    let cases = [
        (0_i64, "19700101T000000Z"),
        (1, "19700101T000001Z"),
        (86_399, "19700101T235959Z"),
        (86_400, "19700102T000000Z"),
        // A leap day, and the day after it.
        (951_782_400, "20000229T000000Z"),
        (951_868_800, "20000301T000000Z"),
        // 2000 was a leap year and 2100 is not: the century rule, both ways.
        (4_107_542_400, "21000301T000000Z"),
        // A round moment in this project's own week.
        (1_789_528_500, "20260916T031500Z"),
        // A 32-bit second count would have stopped here.
        (2_147_483_647, "20380119T031407Z"),
        (2_147_483_648, "20380119T031408Z"),
        // Before the epoch, which a wrong clock can produce. Floor division, so this is
        // the last second of 1969 rather than the first of 1970.
        (-1, "19691231T235959Z"),
        (-86_400, "19691231T000000Z"),
    ];

    for (seconds, expected) in cases {
        assert_eq!(
            Stamp::from_unix_seconds(seconds).utc_path(),
            expected,
            "{seconds}"
        );
    }
}

/// The path is text that sorts chronologically, which is the whole reason it is shaped
/// this way rather than `16-09-2026` or a local time.
#[test]
fn the_directory_names_sort_into_chronological_order() {
    let moments = [
        0_i64,
        86_400,
        951_782_400,
        1_789_528_500,
        1_789_528_501,
        2_147_483_648,
    ];

    let names: Vec<String> = moments
        .iter()
        .map(|&seconds| Stamp::from_unix_seconds(seconds).utc_path())
        .collect();

    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "the names do not sort the way the moments do"
    );
}

#[test]
fn the_readable_form_says_which_zone_it_is_in() {
    let stamp = Stamp::from_unix_seconds(1_789_528_500);
    assert_eq!(stamp.readable_utc(), "2026-09-16 03:15:00 UTC");
    assert_eq!(stamp.utc_iso(), "2026-09-16T03:15:00Z");
    // Labelled, so that nothing is ever presented as somebody's wall clock when it is not.
    assert!(stamp.readable_utc().ends_with("UTC"));
}

/// The local rendering, in timezones this machine is almost certainly not in.
///
/// The offset is the only part that has to be looked up; everything after it is the same
/// arithmetic the UTC path uses, moved by a number. So the half that can be checked
/// against a known calendar is checked against one, and half-hour and negative offsets are
/// in the list because they are where a naive implementation goes wrong.
#[test]
fn a_moment_reads_as_the_clock_of_wherever_it_was_taken() {
    let stamp = Stamp::from_unix_seconds(1_789_528_500); // 2026-09-16 03:15:00 UTC

    for (offset, readable, iso) in [
        (0, "2026-09-16 03:15:00 +00:00", "2026-09-16T03:15:00+00:00"),
        // India, the half-hour case.
        (
            19_800,
            "2026-09-16 08:45:00 +05:30",
            "2026-09-16T08:45:00+05:30",
        ),
        // New York in September, and the date is still the same one.
        (
            -14_400,
            "2026-09-15 23:15:00 -04:00",
            "2026-09-15T23:15:00-04:00",
        ),
        // Chatham Islands, the quarter-hour case nobody remembers exists.
        (
            45_900,
            "2026-09-16 16:00:00 +12:45",
            "2026-09-16T16:00:00+12:45",
        ),
        // Far enough west to be on the day before.
        (
            -39_600,
            "2026-09-15 16:15:00 -11:00",
            "2026-09-15T16:15:00-11:00",
        ),
    ] {
        let local = Local::at(stamp, offset);
        assert_eq!(local.readable(), readable, "offset {offset}");
        assert_eq!(local.iso(), iso, "offset {offset}");
        assert_eq!(local.offset_seconds(), offset);
    }
}

/// An offset is always signed and always padded, because a column of them is scanned
/// rather than parsed.
#[test]
fn an_offset_is_written_the_way_every_timestamp_writes_one() {
    let stamp = Stamp::from_unix_seconds(0);
    for (offset, expected) in [
        (0, "+00:00"),
        (3_600, "+01:00"),
        (-3_600, "-01:00"),
        (19_800, "+05:30"),
        (-34_200, "-09:30"),
        (50_400, "+14:00"),
        (-43_200, "-12:00"),
    ] {
        assert_eq!(
            Local::at(stamp, offset).offset_label(),
            expected,
            "{offset}"
        );
    }
}

/// The lookup itself. What it answers depends on the machine, so what is asserted is what
/// has to be true on every machine: it is a real offset, and moving the instant by it
/// produces the same moment rather than a different one.
#[test]
fn this_machines_offset_is_a_real_one() {
    let now = Stamp::now();
    let local = now.local();
    let offset = local.offset_seconds();

    assert!(
        (-50_400..=50_400).contains(&offset),
        "{offset} is not an offset any timezone has"
    );
    assert_eq!(
        offset % 900,
        0,
        "{offset} is not a whole quarter of an hour"
    );

    // The local rendering is the UTC one moved by exactly the offset it reports, which is
    // the property that keeps the two from drifting apart.
    let shifted = Stamp::from_unix_seconds(now.unix_seconds() + i64::from(offset));
    assert!(
        local
            .readable()
            .starts_with(shifted.readable_utc().trim_end_matches(" UTC")),
        "{} does not match {}",
        local.readable(),
        shifted.readable_utc()
    );
}

#[test]
fn now_is_somewhere_in_this_century() {
    let now = Stamp::now();
    assert!(
        now.unix_seconds() > 1_700_000_000,
        "the clock reads before 2023"
    );
    let path = now.utc_path();
    assert_eq!(path.len(), 16, "{path}");
    assert!(path.ends_with('Z'), "{path}");
}

/// The layout from `CLAUDE.md`, and nothing else.
#[test]
fn a_backup_lands_where_the_layout_says() {
    let taken = Stamp::from_unix_seconds(1_789_528_500);
    let root = Path::new("/store");
    let directory = directory_for(root, Engine::Postgres, "orders", taken);

    // Assembled with `join` rather than written as a string: on Windows `join` produces
    // backslashes, and a literal with forward slashes in it compares against a path this
    // platform never produces.
    let expected: PathBuf = ["backups", "postgres", "orders", "20260916T031500Z"]
        .iter()
        .fold(root.to_path_buf(), |path, part| path.join(part));

    assert_eq!(directory, expected);
    assert_eq!(dump_file(&directory), expected.join("dump"));

    // The engine's own word for itself, so the tree reads the way the registry does.
    for (engine, expected) in [
        (Engine::Postgres, "postgres"),
        (Engine::Mysql, "mysql"),
        (Engine::Mariadb, "mariadb"),
    ] {
        let path = directory_for(Path::new("/store"), engine, "x", taken);
        assert!(
            path.to_string_lossy().contains(expected),
            "{engine}: {}",
            path.display()
        );
    }
}

/// A label becomes one path component, whatever a person called their database.
///
/// `db add` allows almost anything, so this is the line between "a name that works" and a
/// backup written into a directory nobody thinks to look in.
#[test]
fn a_label_that_could_escape_its_directory_cannot() {
    for (label, expected) in [
        ("orders", "orders"),
        ("my-app", "my-app"),
        ("app.v2", "app.v2"),
        ("Ünïcödé", "Ünïcödé"),
        // The separators, which are the ones that would actually move the directory.
        ("a/b", "a_b"),
        ("a\\b", "a_b"),
        ("../../etc", ".._.._etc"),
        // Illegal in a Windows filename, so refused everywhere rather than on one
        // platform — a backup that works on Linux and not on Windows is worse than one
        // with an underscore in its name.
        ("a:b", "a_b"),
        ("a*b", "a_b"),
        ("a?b", "a_b"),
        ("a\"b", "a_b"),
        ("a<b", "a_b"),
        ("a>b", "a_b"),
        ("a|b", "a_b"),
        // Legal in a name, illegal at the end of a Windows directory.
        ("trailing.", "trailing"),
        ("trailing ", "trailing"),
        // And a name that is nothing but those still has to go somewhere.
        ("...", "unnamed"),
        ("", "unnamed"),
    ] {
        assert_eq!(sanitise(label), expected, "{label:?}");
    }

    // The property that matters: whatever comes out is exactly one component.
    for label in ["a/b", "../../etc", "a\\b\\c", ""] {
        let cleaned = sanitise(label);
        assert_eq!(
            Path::new(&cleaned).components().count(),
            1,
            "{label:?} became {cleaned:?}"
        );
    }
}
