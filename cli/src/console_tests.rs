//! What a job says, and who hears it.
//!
//! **These tests take over a process-wide seam, so they hold a lock while they do it.**
//! `cargo test` runs a module's tests on several threads at once, and two of them installing
//! different consoles would be two tests reading each other's lines.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use super::{Console, Kind, bytes, current, meter, settled, step, take_over};
use crate::failure::Outcome;
use crate::mark::{self, Mark};

/// Held for the length of any test that installs a console.
fn alone() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A console that writes everything down instead of drawing it.
#[derive(Debug, Default)]
struct Recorder {
    said: Mutex<Vec<String>>,
    answer: Mutex<String>,
}

impl Recorder {
    fn lines(&self) -> Vec<String> {
        self.said
            .lock()
            .map(|said| said.clone())
            .unwrap_or_default()
    }

    fn write(&self, line: String) {
        if let Ok(mut said) = self.said.lock() {
            said.push(line);
        }
    }
}

impl Console for Recorder {
    fn line(&self, kind: Kind, text: &str) {
        self.write(format!("{kind:?}: {text}"));
    }

    fn begin(&self, id: u64, label: &str) {
        self.write(format!("begin {id}: {label}"));
    }

    fn measure(&self, id: u64, done: u64, total: Option<u64>, note: &str) {
        self.write(format!("measure {id}: {done}/{total:?} {note}"));
    }

    fn settle(&self, id: u64, mark: Mark, label: &str, note: &str) {
        self.write(format!("settle {id}: {mark:?} {label} {note}"));
    }

    fn tick(&self) {
        self.write("tick".to_owned());
    }

    fn animates(&self) -> bool {
        false
    }

    fn ask(&self, prompt: &str, mask: bool) -> Outcome<String> {
        self.write(format!(
            "ask{}: {prompt}",
            if mask { " hidden" } else { "" }
        ));
        Ok(self
            .answer
            .lock()
            .map(|answer| answer.clone())
            .unwrap_or_default())
    }
}

/// Install a recorder, run `body`, and hand back what it heard.
fn heard(body: impl FnOnce()) -> Vec<String> {
    let _alone = alone();
    let recorder = Arc::new(Recorder::default());
    let driving = take_over(Arc::clone(&recorder) as Arc<dyn Console>);
    body();
    drop(driving);
    recorder.lines()
}

#[test]
fn a_line_reaches_whoever_is_driving() {
    let lines = heard(|| crate::report::note("a line"));
    assert_eq!(lines, vec!["Err: a line".to_owned()]);
}

#[test]
fn the_console_goes_back_when_the_guard_is_dropped() {
    let _alone = alone();
    let recorder = Arc::new(Recorder::default());
    {
        let _driving = take_over(Arc::clone(&recorder) as Arc<dyn Console>);
        current().line(Kind::Err, "inside");
    }
    current().animates();
    assert_eq!(recorder.lines(), vec!["Err: inside".to_owned()]);
}

#[test]
fn a_step_begins_and_settles_under_the_name_it_finished_with() {
    let lines = heard(|| {
        let working = step("Connecting to shop", "Connected");
        working.ok("postgres 17.9");
    });
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].ends_with("Connecting to shop"), "{:?}", lines[0]);
    assert!(
        lines[1].contains("Ok Connected postgres 17.9"),
        "{:?}",
        lines[1]
    );
}

/// A job that returned early must not leave a spinner running on a screen nobody is
/// redrawing.
#[test]
fn a_step_dropped_unanswered_settles_itself() {
    let lines = heard(|| {
        let working = step("Dumping", "Dumped");
        drop(working);
    });
    assert!(lines[1].contains("Plain"), "{:?}", lines[1]);
}

#[test]
fn a_settled_step_is_settled_once_however_many_times_it_is_asked() {
    let lines = heard(|| {
        let mut working = step("Dumping", "Dumped");
        working.finish(Mark::Ok, "one");
        working.finish(Mark::Bad, "two");
    });
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[1].contains("Ok Dumped one"));
}

#[test]
fn progress_reaches_the_step_it_belongs_to() {
    let lines = heard(|| {
        let working = step("Downloading", "Downloaded");
        working.at(120, Some(400), "120 of 400");
        working.ok("");
    });
    assert!(lines[1].contains("measure"), "{:?}", lines[1]);
    assert!(lines[1].contains("120/Some(400)"), "{:?}", lines[1]);
}

#[test]
fn a_question_goes_to_whoever_is_driving() {
    let lines = heard(|| {
        let _ = super::ask_hidden("Password for shop: ");
    });
    assert_eq!(lines, vec!["ask hidden: Password for shop: ".to_owned()]);
}

/// The same sentence on both surfaces: what `sloop backup shop` prints in a shell is what
/// the menu's live screen puts on its rail.
#[test]
fn a_settled_line_carries_the_mark_the_label_and_the_note() {
    let line = plain(&settled(Mark::Ok, "Dumped 22 tables", "412.8 MB"));
    // The mark sits in a spinner frame's worth of columns, so a settled step and a running
    // one start their labels in the same place.
    assert!(
        line.starts_with(&format!(
            "{}{}  Dumped 22 tables",
            Mark::Ok.glyph(),
            " ".repeat(crate::mark::FRAME_WIDTH - crate::mark::Mark::WIDTH)
        )),
        "{line}"
    );
    assert!(line.ends_with("412.8 MB"), "{line}");
}

#[test]
fn a_settled_line_with_nothing_beside_it_ends_at_its_label() {
    let line = plain(&settled(Mark::Warn, "Nothing to prune", ""));
    assert_eq!(
        line,
        format!(
            "{}{}  Nothing to prune",
            Mark::Warn.glyph(),
            " ".repeat(crate::mark::FRAME_WIDTH - crate::mark::Mark::WIDTH)
        )
    );
}

/// However long the label, the note never touches it.
#[test]
fn the_note_is_never_pushed_up_against_the_label() {
    for label in ["a", "a label of some length", &"x".repeat(60)] {
        let line = plain(&settled(Mark::Ok, label, "note"));
        assert!(line.ends_with("  note"), "{line}");
    }
}

#[test]
fn a_meter_is_a_bar_a_percentage_and_the_note() {
    let line = plain(&meter(58, Some(100), "12.4 MB/s", 20));
    assert!(line.contains("58%"), "{line}");
    assert!(line.contains("12.4 MB/s"), "{line}");
    assert!(
        line.contains(mark::bar_start()) && line.contains(mark::bar_end()),
        "{line}"
    );
}

/// Work whose size nobody knows still says how much of it has happened.
#[test]
fn a_meter_with_no_total_falls_back_to_what_it_has() {
    let line = plain(&meter(2_400_000, None, "", 20));
    assert_eq!(line, "2.4 MB");
    let line = plain(&meter(17, None, "17 of the tables", 20));
    assert_eq!(line, "17 of the tables");
}

/// A total of zero is division by zero, and a backup is not the place to find that out.
#[test]
fn a_total_of_zero_does_not_divide_by_it() {
    let line = plain(&meter(0, Some(0), "", 20));
    assert_eq!(line, "0 B");
}

#[test]
fn bytes_are_counted_the_way_the_vendor_counts_them() {
    assert_eq!(bytes(512), "512 B");
    assert_eq!(bytes(2_048), "2.0 kB");
    assert_eq!(bytes(415_700_000), "415.7 MB");
    assert_eq!(bytes(3_200_000_000), "3.2 GB");
}

/// Strip every escape, the way a terminal that was told to stay monochrome would.
fn plain(painted: &str) -> String {
    anstream::adapter::strip_str(painted).to_string()
}
