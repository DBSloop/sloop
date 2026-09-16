//! What `backup` decides without a server: the exit code `--all` leaves behind, and the
//! two pieces of formatting a person actually reads.
//!
//! The parts that need a database are in `backup_cluster_tests.rs`, and the parts that
//! need a whole process are in `tests/backup.rs`.

use super::{describe_bytes, plural, short, verdict};
use crate::exit::Exit;
use crate::failure::Failure;

fn failing(exit: Exit) -> (String, Failure) {
    ("somewhere".to_owned(), Failure::new(exit, "it went wrong"))
}

/// `--all` never stops, so the code it exits with is the only thing a scheduler has to go
/// on. Agreeing failures keep their meaning; disagreeing ones become `1`, which means
/// read the output rather than a wrong specific answer.
#[test]
fn the_code_all_leaves_behind_says_what_a_monitor_can_act_on() {
    assert_eq!(verdict(&[]), Exit::Success, "nothing failed");

    assert_eq!(
        verdict(&[failing(Exit::Connect)]),
        Exit::Connect,
        "one unreachable server is a connection failure"
    );
    assert_eq!(
        verdict(&[
            failing(Exit::Connect),
            failing(Exit::Connect),
            failing(Exit::Connect)
        ]),
        Exit::Connect,
        "three of the same thing is still that thing"
    );
    assert_eq!(
        verdict(&[failing(Exit::Dump), failing(Exit::Dump)]),
        Exit::Dump
    );

    // Two different causes. Reporting either one of them would tell a monitoring system
    // something untrue about the other.
    assert_eq!(
        verdict(&[failing(Exit::Connect), failing(Exit::Dump)]),
        Exit::Failure
    );
    assert_eq!(
        verdict(&[failing(Exit::Usage), failing(Exit::Connect)]),
        Exit::Failure
    );

    // Whatever happens, a run with failures in it never exits 0.
    for codes in [
        vec![Exit::Connect],
        vec![Exit::Dump, Exit::Connect],
        vec![Exit::Usage, Exit::Usage],
        vec![Exit::Failure, Exit::Dump, Exit::Connect],
    ] {
        let failures: Vec<_> = codes.into_iter().map(failing).collect();
        assert_ne!(verdict(&failures), Exit::Success, "{failures:?}");
    }
}

#[test]
fn a_size_reads_at_a_glance_and_a_small_one_stays_exact() {
    for (bytes, expected) in [
        (0_u64, "0 B"),
        (1, "1 B"),
        (1023, "1023 B"),
        (1024, "1.0 KiB"),
        (1536, "1.5 KiB"),
        (1_048_576, "1.0 MiB"),
        (5_872_025_600, "5.5 GiB"),
    ] {
        assert_eq!(describe_bytes(bytes), expected, "{bytes}");
    }
}

#[test]
fn one_of_something_is_not_pluralised() {
    assert_eq!(plural(0, "table"), "0 tables");
    assert_eq!(plural(1, "table"), "1 table");
    assert_eq!(plural(2, "row"), "2 rows");
}

/// Short enough for a line, long enough to tell two dumps apart — and never a different
/// hash from the one in the manifest.
#[test]
fn a_shortened_checksum_is_the_front_of_the_real_one() {
    let full = "3f9ac1c2b5d4e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f";
    assert_eq!(short(full), "3f9ac1c2b5d4");
    assert!(full.starts_with(&short(full)));
    assert_eq!(
        short("abc"),
        "abc",
        "a short one is not padded or panicked on"
    );
}
