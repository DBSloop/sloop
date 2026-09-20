//! What a job's own screen holds, and what is left of it afterwards.
//!
//! **No terminal anywhere near these.** [`Live`] paints through `crossterm`, which reports a
//! width of nothing when there is no terminal and is written to a captured standard error
//! either way; what is asserted here is the state it keeps, which is the thing the outcome
//! screen is built from.

use std::sync::Arc;

use super::{Live, Told, bar_width, clipped, dots};
use crate::console::{Console as _, Kind};
use crate::mark::Mark;

/// A live screen with nothing on it yet.
fn opened() -> Arc<Live> {
    Live::opened(
        &["Backups", "Back one up now"],
        "dumps it and checks every row arrived",
    )
}

#[test]
fn a_line_a_job_printed_is_kept_as_it_printed_it() {
    let live = opened();
    live.line(Kind::Err, "  22 tables");
    assert_eq!(
        live.transcript(),
        vec![Told {
            mark: Mark::Plain,
            text: "  22 tables".to_owned(),
            note: String::new(),
        }]
    );
}

#[test]
fn a_step_is_written_down_once_it_settles_and_not_before() {
    let live = opened();
    live.begin(1, "Connecting to shop");
    assert!(live.transcript().is_empty(), "a step that is still running");

    live.settle(1, Mark::Ok, "Connected", "postgres 17.9");
    assert_eq!(
        live.transcript(),
        vec![Told {
            mark: Mark::Ok,
            text: "Connected".to_owned(),
            note: "postgres 17.9".to_owned(),
        }]
    );
}

/// A step dropped without an answer says nothing on the way in and nothing on the way out.
#[test]
fn a_step_settled_as_nothing_leaves_no_trace() {
    let live = opened();
    live.begin(1, "Dumping");
    live.settle(1, Mark::Plain, "", "");
    assert!(live.transcript().is_empty());
}

/// The transcript is what the outcome screen is built from, so its order is the order things
/// happened in.
#[test]
fn the_transcript_is_in_the_order_it_happened() {
    let live = opened();
    live.begin(1, "Connecting");
    live.settle(1, Mark::Ok, "Connected", "");
    live.line(Kind::Err, "a note");
    live.begin(2, "Dumping");
    live.settle(2, Mark::Warn, "Dumped", "the source was live");

    let marks: Vec<Mark> = live.transcript().iter().map(|told| told.mark).collect();
    assert_eq!(marks, vec![Mark::Ok, Mark::Plain, Mark::Warn]);
}

#[test]
fn a_measure_belongs_to_the_step_that_is_running() {
    let live = opened();
    live.begin(1, "Downloading");
    live.measure(1, 120, Some(400), "12 MB/s");
    // A measure from a step that has already settled is ignored rather than drawn.
    live.settle(1, Mark::Ok, "Downloaded", "");
    live.measure(1, 400, Some(400), "");
    assert_eq!(live.transcript().len(), 1);
}

#[test]
fn only_a_marked_step_counts_as_a_step() {
    assert!(
        Told {
            mark: Mark::Ok,
            text: String::new(),
            note: String::new()
        }
        .is_a_step()
    );
    assert!(
        !Told {
            mark: Mark::Plain,
            text: String::new(),
            note: String::new()
        }
        .is_a_step()
    );
}

/// A screen whose bar ran off the right edge would wrap and push everything below it about.
#[test]
fn the_bar_fits_every_terminal_it_is_drawn_in() {
    for columns in [20_usize, 60, 80, 120, 300] {
        let width = bar_width(columns);
        assert!(width >= 12, "{columns} columns gave a {width}-wide bar");
        assert!(
            width + 46 <= columns.max(58),
            "{columns} columns gave a {width}-wide bar"
        );
    }
}

#[test]
fn a_label_too_long_for_the_screen_is_cut_rather_than_wrapped() {
    let long = "x".repeat(200);
    let cut = clipped(&long, 60);
    assert!(cut.chars().count() <= 60, "{}", cut.chars().count());
    assert!(cut.ends_with('\u{2026}'));
    // One that fits is left exactly as it is.
    assert_eq!(clipped("Dumping shop", 60), "Dumping shop");
}

#[test]
fn a_hidden_answer_is_drawn_as_one_mark_per_character() {
    assert_eq!(dots(0), "");
    assert_eq!(dots(4).chars().count(), 4);
}

/// **A screen with nowhere to draw draws nothing.** Under `cargo test` stderr is captured,
/// and a live screen that painted anyway wrote cursor-home and clear-to-end into the middle
/// of the harness's own output — which scrambled the run summary and made a failing test look
/// like one that never reported.
#[test]
fn a_live_screen_with_no_terminal_paints_nothing() {
    let live = opened();
    assert!(
        !live.draws,
        "a test has no terminal, so nothing should be painted"
    );
    assert!(!live.animates(), "and nothing should be animated either");

    // It still writes everything down, which is what the outcome screen is built from.
    live.settle(1, Mark::Ok, "Connected", "postgres 17.9");
    assert_eq!(live.transcript().len(), 1);
}
