//! The screen a job runs on, without the terminal ever being handed back.
//!
//! **This is the owner's first rule, and it is the reason this file exists.** Until now a
//! job left the alternate screen, ran on the terminal underneath, and took the screen again
//! afterwards — so a password prompt, a download's progress and a backup's report all landed
//! in the scrollback. That was a deliberate design and it is now reversed: *"sloop will not
//! come to the traditional console again until we quit"*. See
//! `docs/OWNER-DECISIONS.md`, "The job runs inside the screen".
//!
//! So a job now runs *here*: [`Live`] takes over [`crate::console`] for as long as it lasts,
//! and every line it prints, every step it starts and every question it asks is drawn on this
//! screen instead of on the terminal.
//!
//! **What it draws is the owner's Progress B.** A list of steps, each one marked as it
//! settles, with the bar indented under whichever one is running:
//!
//! ```text
//!   ✓    Looked for PostgreSQL 18          not on this machine
//!   ✓    Resolved the build                18.1 · windows-x64
//!   ●∙∙  Downloading                       241 MB of 416 MB
//!
//!        ▕████████████▊        ▏  58%   12.4 MB/s · 18s
//! ```
//!
//! **Nothing here survives the screen**, which is the standing rule of the alternate buffer.
//! So everything is *also* written down: [`Live::transcript`] hands the whole run back to the
//! menu, which puts it on the outcome screen — the one the user actually reads.

use std::io::{IsTerminal as _, Write as _};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::console::{self, Console, Kind};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::mark::{self, Mark};
use crate::style::Hue;

use super::ask::{self, Key};
use super::paint::{self, INSET};

/// One thing that happened, kept so the outcome screen can show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    /// What it was: worked, did not, worth knowing, or none of those.
    pub mark: Mark,
    /// The sentence.
    pub text: String,
    /// The detail beside it, in its own column. Empty for most lines.
    pub note: String,
}

impl Told {
    /// A line a job printed, which already carries whatever colour it chose.
    fn said(text: &str) -> Self {
        Self {
            mark: Mark::Plain,
            text: text.to_owned(),
            note: String::new(),
        }
    }

    /// Is this one of the marked steps, rather than ordinary output?
    #[must_use]
    pub const fn is_a_step(&self) -> bool {
        matches!(self.mark, Mark::Ok | Mark::Warn | Mark::Bad)
    }
}

/// The step that is running, and how far along it is.
#[derive(Debug)]
struct Run {
    /// Which step, so a measure from an older one is ignored.
    id: u64,
    /// What it is doing, in the present tense.
    label: String,
    /// How much of it is done, when it is the kind of work with a size.
    measure: Option<Measure>,
    /// Which frame of the spinner is showing.
    frame: usize,
}

/// How far along a step is.
#[derive(Debug)]
struct Measure {
    done: u64,
    total: Option<u64>,
    note: String,
}

/// A question waiting on the bottom row.
#[derive(Debug)]
struct Asking {
    /// What is being asked.
    question: String,
    /// What has been typed so far.
    typed: String,
    /// Whether to draw dots instead of it.
    hidden: bool,
}

/// Everything on the screen, behind one lock.
///
/// **One lock rather than several**, because the ticker thread and the job's own thread both
/// write here and a screen assembled from two half-updated halves is a screen that flickers
/// between two truths.
#[derive(Debug)]
struct Inner {
    /// Where this is in the tree, for the line above everything.
    crumbs: Vec<String>,
    /// What the job is, in one sentence.
    title: String,
    /// Everything that has happened, oldest first.
    told: Vec<Told>,
    /// The step that is running, if one is.
    running: Option<Run>,
    /// The question on the bottom row, if there is one.
    asking: Option<Asking>,
    /// When the job started, for the clock in the corner.
    since: Instant,
}

/// The console a job gets while the menu is driving it.
#[derive(Debug)]
pub struct Live {
    inner: Mutex<Inner>,
    /// Whether there is a screen to draw on.
    ///
    /// **False under `cargo test`, and that is the point.** The shell will not open without a
    /// terminal, so in a real session this is always true; in a test it is not, and a screen
    /// that painted anyway wrote cursor-home and clear-to-end into the middle of the test
    /// harness's own output. That is not cosmetic — it scrambled the run's summary, which is
    /// how a failing test comes to look like a test that never reported.
    ///
    /// What it does not affect is [`Live::transcript`]: everything is still written down, so
    /// what a job said is as testable as where it went.
    draws: bool,
}

impl Live {
    /// Open a live screen for one job.
    #[must_use]
    pub fn opened(crumbs: &[&str], title: &str) -> Arc<Self> {
        let live = Arc::new(Self {
            draws: std::io::stderr().is_terminal(),
            inner: Mutex::new(Inner {
                crumbs: crumbs.iter().map(|crumb| (*crumb).to_owned()).collect(),
                title: title.to_owned(),
                told: Vec::new(),
                running: None,
                asking: None,
                since: Instant::now(),
            }),
        });
        live.repaint();
        live
    }

    /// Everything that happened, for the screen that is shown afterwards.
    #[must_use]
    pub fn transcript(&self) -> Vec<Told> {
        self.inner
            .lock()
            .map(|inner| inner.told.clone())
            .unwrap_or_default()
    }

    /// Draw the whole screen where the last one was.
    fn repaint(&self) {
        if !self.draws {
            return;
        }
        let Ok(inner) = self.inner.lock() else {
            return;
        };
        let drawn = inner.drawn();
        drop(inner);

        let mut out = std::io::stderr();
        let _ = write!(out, "{}", ask::redrawn_in_place(&drawn, "\r\n", true));
        let _ = out.flush();
    }

    /// Write a line down and redraw.
    fn record(&self, told: Told) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.told.push(told);
        }
        self.repaint();
    }
}

impl Inner {
    /// The whole screen as one string, `\r\n` between the rows.
    ///
    /// **Built whole and written once**, the same rule the menu's own list follows: a frame
    /// arriving in twenty writes is a frame that tears while the eye is on it.
    fn drawn(&self) -> String {
        let columns = ask::columns().unwrap_or(80);
        let rows = ask::rows().unwrap_or(24);
        let mut out: Vec<String> = Vec::new();

        out.push(String::new());
        out.push(format!(
            "{}{}",
            pad(INSET),
            paint::trail_of(&self.crumbs.iter().map(String::as_str).collect::<Vec<_>>())
        ));
        out.push(String::new());
        out.push(format!(
            "{}{}{}",
            pad(INSET),
            paint::hue(Hue::Text, &self.title),
            elapsed(self.since, self.title.chars().count(), columns)
        ));
        out.push(String::new());

        // The bar and the footer are spoken for before the feed gets what is left, so a long
        // run scrolls rather than pushing "Esc to cancel" off the bottom.
        let spoken_for = out.len() + 5;
        let room = rows.saturating_sub(spoken_for).max(3);
        let feed = self.feed(columns);
        for line in feed.iter().skip(feed.len().saturating_sub(room)) {
            out.push(line.clone());
        }

        if let Some(running) = &self.running
            && let Some(measure) = &running.measure
        {
            out.push(String::new());
            out.push(format!(
                "{}{}",
                pad(INSET + mark::FRAME_WIDTH + 2),
                console::meter(
                    measure.done,
                    measure.total,
                    &measure.note,
                    bar_width(columns)
                )
            ));
        }

        out.push(String::new());
        match &self.asking {
            Some(asking) => {
                out.push(format!(
                    "{}{}  {}",
                    pad(INSET),
                    paint::accent("?"),
                    paint::hue(Hue::Text, &asking.question)
                ));
                let shown = if asking.hidden {
                    dots(asking.typed.chars().count())
                } else {
                    asking.typed.clone()
                };
                out.push(format!(
                    "{}{}{}",
                    pad(INSET + 3),
                    paint::accent(mark::bar_end()),
                    paint::hue(Hue::Text, &shown)
                ));
                out.push(String::new());
                out.push(format!(
                    "{}{}",
                    pad(INSET),
                    paint::dim("Enter to answer    Esc to stop")
                ));
            }
            None => out.push(format!("{}{}", pad(INSET), paint::dim("Esc to stop"))),
        }

        out.join("\r\n")
    }

    /// The step list, as the rows it occupies.
    fn feed(&self, columns: usize) -> Vec<String> {
        let mut rows: Vec<String> = Vec::new();
        for told in &self.told {
            rows.push(match told.mark {
                // A line the job printed carries its own colour and its own indent already.
                Mark::Plain => format!("{}{}", pad(INSET + mark::FRAME_WIDTH + 2), told.text),
                _ => format!(
                    "{}{}",
                    pad(INSET),
                    console::settled(told.mark, &told.text, &told.note)
                ),
            });
        }

        if let Some(running) = &self.running {
            let frame = mark::frames()[running.frame % mark::frames().len()];
            rows.push(format!(
                "{}{}  {}",
                pad(INSET),
                paint::hue(Mark::Doing.hue(), frame),
                paint::hue(Hue::Text, &clipped(&running.label, columns))
            ));
        }
        rows
    }
}

impl Console for Live {
    fn line(&self, _kind: Kind, text: &str) {
        self.record(Told::said(text));
    }

    fn begin(&self, id: u64, label: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            let carried = inner
                .running
                .take()
                .filter(|running| running.id == id)
                .and_then(|running| running.measure);
            inner.running = Some(Run {
                id,
                label: label.to_owned(),
                measure: carried,
                frame: 0,
            });
        }
        self.repaint();
    }

    fn measure(&self, id: u64, done: u64, total: Option<u64>, note: &str) {
        let mut changed = false;
        if let Ok(mut inner) = self.inner.lock()
            && let Some(running) = inner.running.as_mut().filter(|running| running.id == id)
        {
            running.measure = Some(Measure {
                done,
                total,
                note: note.to_owned(),
            });
            changed = true;
        }
        if changed {
            self.repaint();
        }
    }

    fn settle(&self, id: u64, mark: Mark, label: &str, note: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner
                .running
                .as_ref()
                .is_some_and(|running| running.id == id)
            {
                inner.running = None;
            }
            // A step dropped unanswered said nothing on the way in and says nothing here.
            if !(mark == Mark::Plain && label.is_empty()) {
                inner.told.push(Told {
                    mark,
                    text: label.to_owned(),
                    note: note.to_owned(),
                });
            }
        }
        self.repaint();
    }

    fn tick(&self) {
        let mut moving = false;
        if let Ok(mut inner) = self.inner.lock()
            && let Some(running) = inner.running.as_mut()
        {
            running.frame = running.frame.wrapping_add(1);
            moving = true;
        }
        if moving {
            self.repaint();
        }
    }

    fn animates(&self) -> bool {
        self.draws
    }

    /// Ask, on the bottom row of this screen, and never on the terminal underneath.
    ///
    /// **Rule 3 is unchanged by the route.** What is typed is held for as long as it takes to
    /// hand back and is drawn as dots, exactly as `rpassword` would have drawn nothing — the
    /// difference is only which screen it happens on.
    fn ask(&self, prompt: &str, mask: bool) -> Outcome<String> {
        if let Ok(mut inner) = self.inner.lock() {
            inner.asking = Some(Asking {
                // The prompts were written for a shell, where a trailing colon and a space
                // are how a line says it is waiting. On a screen of its own the question is
                // a question, and the box underneath says the rest.
                question: prompt.trim_end().trim_end_matches(':').to_owned(),
                typed: String::new(),
                hidden: mask,
            });
        }

        let answered = self.read_it();

        if let Ok(mut inner) = self.inner.lock() {
            inner.asking = None;
        }
        self.repaint();
        answered
    }
}

impl Live {
    /// The key loop behind [`Console::ask`].
    fn read_it(&self) -> Outcome<String> {
        let _raw = ask::Raw::on()?;
        loop {
            self.repaint();
            let Some(key) = ask::pressed()? else { continue };
            let Ok(mut inner) = self.inner.lock() else {
                return Ok(String::new());
            };
            let Some(asking) = inner.asking.as_mut() else {
                return Ok(String::new());
            };
            match key {
                Key::Enter => {
                    let typed = std::mem::take(&mut asking.typed);
                    return Ok(typed);
                }
                Key::Typed(letter) => asking.typed.push(letter),
                Key::Rubbed => {
                    asking.typed.pop();
                }
                // **Esc stops the job rather than answering it with nothing.** An empty
                // password read as an answer would be a connection attempt somebody did not
                // make, and an empty confirmation read as an answer is a destructive
                // operation one keystroke away from happening by accident.
                Key::Back | Key::Quit => {
                    return Err(Failure::new(Exit::Usage, "you stopped it at the question")
                        .hint("nothing was changed"));
                }
                _ => {}
            }
        }
    }
}

/// Run `body` with `live` taking every line and every question it produces.
pub fn under(live: &Arc<Live>, body: impl FnOnce() -> Outcome<Exit>) -> Outcome<Exit> {
    let driving = console::take_over(Arc::clone(live) as Arc<dyn Console>);
    let outcome = body();
    drop(driving);
    outcome
}

/// The clock in the top right, once a job has been running long enough to want one.
///
/// **Three seconds before it appears.** A clock that starts at `0:00` on every job makes
/// every job look slow; one that turns up when a job is actually taking a while is
/// information.
fn elapsed(since: Instant, used: usize, columns: usize) -> String {
    let seconds = since.elapsed().as_secs();
    if seconds < 3 {
        return String::new();
    }
    let clock = format!("{}:{:02}", seconds / 60, seconds % 60);
    let room = columns
        .saturating_sub(INSET * 2)
        .saturating_sub(used + clock.chars().count());
    format!("{}{}", pad(room), paint::dim(&clock))
}

/// How wide the bar is drawn, given the terminal.
fn bar_width(columns: usize) -> usize {
    // Everything beside it — the indent, the caps, the percentage and the note — is about
    // thirty columns, and a bar under twelve is a bar that cannot show a percent.
    columns.saturating_sub(46).clamp(12, 48)
}

/// A label cut to fit, rather than one that wraps and pushes the screen about.
fn clipped(text: &str, columns: usize) -> String {
    let room = columns.saturating_sub(INSET + mark::FRAME_WIDTH + 4).max(8);
    if text.chars().count() <= room {
        return text.to_owned();
    }
    text.chars()
        .take(room.saturating_sub(1))
        .collect::<String>()
        + "…"
}

/// What a hidden answer looks like while it is being typed.
fn dots(count: usize) -> String {
    let dot = if mark::unicode() { "\u{2022}" } else { "*" };
    dot.repeat(count)
}

fn pad(width: usize) -> String {
    " ".repeat(width)
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
