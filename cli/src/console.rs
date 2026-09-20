//! Who a running job is talking to, and the one seam the interactive shell takes over.
//!
//! **Every job in this program prints and asks the same way, and this is where the two
//! surfaces diverge.** Run `sloop backup shop` from a shell and the lines go to standard
//! error, the password prompt is `rpassword`'s, and a spinner rewrites one line in place.
//! Run the same job from the menu and every one of those has to happen *inside the alternate
//! screen* — because the owner's rule is that sloop does not hand the terminal back until the
//! session is over.
//!
//! The alternative was a second implementation of every command's output for the menu, which
//! is the thing `commands::menu` exists to avoid. So instead there is one [`Console`] per
//! run: [`Plain`] by default, and the shell's own while a job runs under it. A command
//! cannot tell the difference and does not have to.
//!
//! **Three things go through here.**
//!
//! *Lines.* [`crate::report`] still owns whether a line is printed at all — `--quiet`,
//! `--json`, the log file — and hands the ones that survive to whichever console is current.
//!
//! *Steps.* [`step`] is a piece of work that takes long enough to be worth watching: it spins
//! while it runs and settles into a tick, a cross or a triangle. Outside the menu it is one
//! line rewritten in place; inside, it is a row on the live screen.
//!
//! *Questions.* [`ask`] is every password, every `y/n` and every typed confirmation. Rule 3
//! and rule 5 are unchanged by the route: the answer is read once, by whoever is driving.

use std::fmt::Write as _;
use std::io::{IsTerminal as _, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};

use crate::failure::{Failure, Outcome};
use crate::mark::{self, Mark};
use crate::style::{self, Hue};

/// Which stream a line belongs on, and how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Standard output: what the command was asked to produce.
    Out,
    /// Standard error: what it wants to say about producing it.
    Err,
    /// Standard error, never silenced.
    Loud,
}

/// Somebody a running job is talking to.
///
/// **`Send + Sync` because the ticker animates from another thread.** The job holds the main
/// thread for as long as the dump takes; something else has to be moving the spinner.
pub trait Console: Send + Sync {
    /// A finished line.
    fn line(&self, kind: Kind, text: &str);

    /// A piece of work has started. `label` is what it is doing, in the present tense.
    fn begin(&self, id: u64, label: &str);

    /// How far along it is. `total` of `None` is work whose size is not known.
    fn measure(&self, id: u64, done: u64, total: Option<u64>, note: &str);

    /// It finished, one way or another. `label` is what it did, in the past tense.
    fn settle(&self, id: u64, mark: Mark, label: &str, note: &str);

    /// Redraw whatever is moving. Called by the ticker, never by a job.
    fn tick(&self);

    /// Is there anybody watching closely enough for animation to be worth it?
    fn animates(&self) -> bool;

    /// Put a question and wait for the answer. `mask` hides what is typed.
    fn ask(&self, prompt: &str, mask: bool) -> Outcome<String>;
}

// ---------------------------------------------------------------- who is driving

/// The console the shell has taken over with, if it has.
static DRIVING: RwLock<Option<Arc<dyn Console>>> = RwLock::new(None);

/// Whoever a job should be talking to right now.
#[must_use]
pub fn current() -> Arc<dyn Console> {
    if let Ok(held) = DRIVING.read()
        && let Some(console) = held.as_ref()
    {
        return Arc::clone(console);
    }
    plain()
}

/// Is the interactive shell driving, rather than a plain run from a command line?
///
/// **Asked by anything that would otherwise take the terminal for itself.** The result grid
/// enters the alternate screen and leaves it again when it closes — which, under the menu,
/// would drop the *menu* back onto the terminal underneath. Under the shell the screen is
/// already taken and already the shell's to give back.
#[must_use]
pub fn shell_is_driving() -> bool {
    DRIVING.read().is_ok_and(|held| held.is_some())
}

/// The default one, made once.
fn plain() -> Arc<dyn Console> {
    static PLAIN: OnceLock<Arc<Plain>> = OnceLock::new();
    Arc::clone(PLAIN.get_or_init(|| Arc::new(Plain::default()))) as Arc<dyn Console>
}

/// Hand every line and every question to `console` until the returned guard is dropped.
///
/// **A guard rather than a pair of calls**, for the same reason the alternate screen is a
/// type: an early return or a panic in the middle of a job would otherwise leave every later
/// line being drawn into a screen that is no longer there.
#[must_use]
pub fn take_over(console: Arc<dyn Console>) -> Driving {
    if let Ok(mut held) = DRIVING.write() {
        *held = Some(console);
    }
    Driving
}

/// Hands the console back on the way out. See [`take_over`].
#[derive(Debug)]
pub struct Driving;

impl Drop for Driving {
    fn drop(&mut self) {
        if let Ok(mut held) = DRIVING.write() {
            *held = None;
        }
    }
}

// ---------------------------------------------------------------- steps

/// The next step's number.
static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Begin a piece of work worth watching.
///
/// `doing` is the present tense, shown while it runs — *"Connecting to shop"*. `done` is the
/// past tense, shown once it has settled — *"Connected"*. Two strings rather than one,
/// because a tick beside *"Connecting"* is a sentence that never finishes.
///
/// A step that is dropped without being settled settles itself as plain, so nothing is left
/// spinning after the work that owned it has returned.
#[must_use]
pub fn step(doing: &str, done: &str) -> Step {
    // `--quiet` and `--json` silence the commentary, and a step is commentary.
    if crate::report::is_quiet() || crate::report::is_json() {
        return Step {
            id: 0,
            doing: doing.to_owned(),
            done: done.to_owned(),
            settled: true,
        };
    }

    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    current().begin(id, doing);
    ticker().wake();
    Step {
        id,
        doing: doing.to_owned(),
        done: done.to_owned(),
        settled: false,
    }
}

/// One piece of work, while it is happening.
#[derive(Debug)]
pub struct Step {
    /// Which step, so a console drawing several can tell them apart.
    id: u64,
    /// What to call it while it is happening, and after it has failed.
    doing: String,
    /// What to call it once it has finished.
    done: String,
    /// Whether it has already been settled, so `Drop` does not settle it twice.
    settled: bool,
}

impl Step {
    /// It worked. `note` is the detail beside the tick, or empty.
    pub fn ok(mut self, note: &str) {
        self.finish(Mark::Ok, note);
    }

    /// It worked, and there is something to know. `note` is that something.
    pub fn warn(mut self, note: &str) {
        self.finish(Mark::Warn, note);
    }

    /// It did not work.
    pub fn bad(mut self, note: &str) {
        self.finish(Mark::Bad, note);
    }

    /// How far along it is, in bytes, rows, tables or whatever it is counting.
    pub fn at(&self, done: u64, total: Option<u64>, note: &str) {
        if !self.settled {
            current().measure(self.id, done, total, note);
        }
    }

    /// Change what it says it is doing, without settling it.
    pub fn saying(&self, label: &str) {
        if !self.settled {
            current().begin(self.id, label);
        }
    }

    fn finish(&mut self, mark: Mark, note: &str) {
        if self.settled {
            return;
        }
        self.settled = true;
        // **A step that did not work keeps the present tense.** `\u{2717} Dumped` is a tick's
        // sentence with a cross in front of it: the thing it names did not happen, and the
        // line has to say which of the two it is without the reader decoding the glyph.
        let label = if mark == Mark::Bad {
            &self.doing
        } else {
            &self.done
        };
        current().settle(self.id, mark, label, note);
        ticker().wake();
    }
}

impl Drop for Step {
    fn drop(&mut self) {
        // A job that returned early left this running. Settle it quietly rather than leaving
        // a spinner on a screen nobody is updating any more.
        self.finish(Mark::Plain, "");
    }
}

/// Run `body`, reporting how big `file` has got while it does.
///
/// **For work whose size nobody knows in advance.** A dump has no content length: the only
/// honest number is how much of it has landed, and the only way to have that without the
/// dump program cooperating is to watch the file. A backup of a hundred gigabytes then says
/// something every tenth of a second instead of nothing for twenty minutes.
pub fn watching<T>(step: &Step, file: &std::path::Path, body: impl FnOnce() -> T) -> T {
    let id = step.id;
    if id == 0 {
        // A hushed step: `--quiet` or `--json`, and a thread to draw nothing is a thread
        // nobody asked for.
        return body();
    }

    let finished = Arc::new(AtomicBool::new(false));
    let watcher = {
        let finished = Arc::clone(&finished);
        let file = file.to_path_buf();
        std::thread::Builder::new()
            .name("sloop-watch".to_owned())
            .spawn(move || {
                while !finished.load(Ordering::Relaxed) {
                    let done = std::fs::metadata(&file).map_or(0, |meta| meta.len());
                    if done > 0 {
                        current().measure(id, done, None, "");
                    }
                    std::thread::sleep(WATCH);
                }
            })
            .ok()
    };

    let out = body();
    finished.store(true, Ordering::Relaxed);
    if let Some(watcher) = watcher {
        let _ = watcher.join();
    }
    out
}

/// How often a growing file is measured.
const WATCH: std::time::Duration = std::time::Duration::from_millis(150);

// ---------------------------------------------------------------- questions

/// Put a question and read the answer, wherever the answer is coming from.
pub fn ask(prompt: &str) -> Outcome<String> {
    current().ask(prompt, false)
}

/// The same, with what is typed hidden.
pub fn ask_hidden(prompt: &str) -> Outcome<String> {
    current().ask(prompt, true)
}

// ---------------------------------------------------------------- the ticker

/// The thread that moves whatever is spinning.
///
/// **One thread for the whole process, parked when there is nothing to animate.** A thread
/// per step would be a thread per table on a backup, and a loop that never sleeps would be a
/// core spent on a spinner.
struct Ticker {
    /// Set while something is worth redrawing.
    awake: Mutex<bool>,
    /// Woken when that changes.
    bell: Condvar,
    /// Whether the thread has been started yet.
    started: AtomicBool,
}

impl Ticker {
    /// Something changed: start the thread if it has not started, and wake it if it is
    /// parked.
    fn wake(&'static self) {
        if !self.started.swap(true, Ordering::SeqCst) {
            let _ = std::thread::Builder::new()
                .name("sloop-spinner".to_owned())
                .spawn(move || self.run());
        }
        if let Ok(mut awake) = self.awake.lock() {
            *awake = true;
            self.bell.notify_all();
        }
    }

    /// Redraw every frame for as long as anybody is animating.
    fn run(&'static self) {
        loop {
            // Park until there is something to do, rather than spinning on a sleep.
            let Ok(mut awake) = self.awake.lock() else {
                return;
            };
            while !*awake {
                let Ok(next) = self.bell.wait(awake) else {
                    return;
                };
                awake = next;
            }
            drop(awake);

            let console = current();
            if console.animates() {
                console.tick();
                std::thread::sleep(mark::FRAME_TIME);
            } else {
                // Nothing to draw on: park again until the next step begins.
                if let Ok(mut awake) = self.awake.lock() {
                    *awake = false;
                }
                std::thread::sleep(mark::FRAME_TIME);
            }
        }
    }
}

fn ticker() -> &'static Ticker {
    static TICKER: OnceLock<Ticker> = OnceLock::new();
    TICKER.get_or_init(|| Ticker {
        awake: Mutex::new(false),
        bell: Condvar::new(),
        started: AtomicBool::new(false),
    })
}

// ---------------------------------------------------------------- the plain one

/// The console a run from a shell gets: lines on the streams, one line rewritten in place
/// for whatever is running.
#[derive(Debug, Default)]
pub struct Plain {
    /// What is on the one line being animated, if anything.
    running: Mutex<Option<Running>>,
}

/// The step `Plain` is currently drawing, and how far along it is.
#[derive(Debug)]
struct Running {
    /// Which step owns the line.
    id: u64,
    /// What it says it is doing.
    label: String,
    /// The bar, when the work has a size.
    measure: Option<(u64, Option<u64>, String)>,
    /// Which frame of the spinner is showing.
    frame: usize,
}

impl Plain {
    /// Rub out the line being animated, so something else can be printed where it was.
    ///
    /// Returns what was there, so the caller can put it back.
    fn erase(&self) {
        if self.running.lock().is_ok_and(|held| held.is_none()) || !self.animates() {
            return;
        }
        let mut out = std::io::stderr();
        let _ = write!(out, "\r\u{1b}[2K");
        let _ = out.flush();
    }

    /// Draw the running line where the cursor is, without a newline after it.
    fn draw(&self, running: &Running) {
        if !self.animates() {
            return;
        }
        let frame = mark::frames()[running.frame % mark::frames().len()];
        let mut line = format!(
            "\r\u{1b}[2K{}  {}",
            tint(Mark::Doing.hue(), frame),
            tint(Hue::Text, &running.label)
        );
        if let Some((done, total, note)) = &running.measure {
            let _ = write!(line, "  {}", meter(*done, *total, note, 24));
        }
        let mut out = std::io::stderr();
        let _ = write!(out, "{line}");
        let _ = out.flush();
    }
}

impl Console for Plain {
    fn line(&self, kind: Kind, text: &str) {
        // Whatever is spinning has to get out of the way of a real line, and go back
        // afterwards: otherwise the two fight over the same row.
        self.erase();
        match kind {
            Kind::Out => anstream::println!("{text}"),
            Kind::Err | Kind::Loud => anstream::eprintln!("{text}"),
        }
        if let Ok(held) = self.running.lock()
            && let Some(running) = held.as_ref()
        {
            self.draw(running);
        }
    }

    fn begin(&self, id: u64, label: &str) {
        let Ok(mut held) = self.running.lock() else {
            return;
        };
        // A second step while one is running replaces it on the line: `Plain` has one row to
        // animate, and the newest thing is the one worth watching.
        let frame = held
            .as_ref()
            .filter(|was| was.id == id)
            .map_or(0, |was| was.frame);
        let running = Running {
            id,
            label: label.to_owned(),
            measure: held
                .take()
                .filter(|was| was.id == id)
                .and_then(|was| was.measure),
            frame,
        };
        self.draw(&running);
        *held = Some(running);
    }

    fn measure(&self, id: u64, done: u64, total: Option<u64>, note: &str) {
        let Ok(mut held) = self.running.lock() else {
            return;
        };
        if let Some(running) = held.as_mut().filter(|running| running.id == id) {
            running.measure = Some((done, total, note.to_owned()));
            self.draw(running);
        }
    }

    fn settle(&self, id: u64, mark: Mark, label: &str, note: &str) {
        let Ok(mut held) = self.running.lock() else {
            return;
        };
        if held.as_ref().is_some_and(|running| running.id == id) {
            *held = None;
        }
        drop(held);

        self.erase();
        if mark == Mark::Plain && label.is_empty() {
            // A step dropped without an answer. It said nothing on the way in and says
            // nothing on the way out.
            return;
        }
        anstream::eprintln!("{}", settled(mark, label, note));
    }

    fn tick(&self) {
        let Ok(mut held) = self.running.lock() else {
            return;
        };
        if let Some(running) = held.as_mut() {
            running.frame = running.frame.wrapping_add(1);
            self.draw(running);
        }
    }

    fn animates(&self) -> bool {
        std::io::stderr().is_terminal() && !crate::report::is_quiet() && !crate::report::is_json()
    }

    fn ask(&self, prompt: &str, mask: bool) -> Outcome<String> {
        self.erase();
        if mask {
            return rpassword::prompt_password(prompt)
                .map_err(|error| Failure::usage(format!("could not read the answer: {error}")));
        }

        crate::report::ask(prompt);
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;
        // Only the line ending comes off. Rule 5 compares what was typed against a name, and
        // a name with a trailing space is a different name.
        Ok(answer.trim_end_matches(['\n', '\r']).to_owned())
    }
}

// ---------------------------------------------------------------- shared drawing

/// Where the grey note beside a settled label starts, counted from the label's own column.
pub const NOTE_AT: usize = 36;

/// How much a settled line spends before its label: the mark, as wide as a spinner frame,
/// and the two columns after it.
pub const LABEL_AT: usize = mark::FRAME_WIDTH + 2;

/// A settled step, as the line both consoles print.
///
/// Shared so that `sloop backup shop` in a shell and the same job on the menu's live screen
/// read identically — one mark, one past-tense label, one grey note in a column.
#[must_use]
pub fn settled(mark: Mark, label: &str, note: &str) -> String {
    // **As wide as a spinner frame**, so a step that has settled and one that is still
    // running start their labels in the same column and the list has one left edge.
    let glyph = format!(
        "{}{}",
        tint(mark.hue(), mark.glyph()),
        " ".repeat(mark::FRAME_WIDTH.saturating_sub(Mark::WIDTH))
    );
    if note.is_empty() {
        return format!("{glyph}  {}", tint(Hue::Text, label));
    }
    // The note sits in a column of its own where the label leaves room for one, and two
    // spaces after it where it does not: a wrapped line is worse than a ragged one.
    let gap = NOTE_AT.saturating_sub(label.chars().count()).max(2);
    format!(
        "{glyph}  {}{}{}",
        tint(Hue::Text, label),
        " ".repeat(gap),
        tint(Hue::Dim, note)
    )
}

/// A measure as a bar and its numbers: `▕████▏  58%  241 MB of 416 MB`.
#[must_use]
pub fn meter(done: u64, total: Option<u64>, note: &str, width: usize) -> String {
    let Some(total) = total.filter(|total| *total > 0) else {
        // Work of unknown size still has something to say: how much of it has happened.
        return if note.is_empty() {
            tint(Hue::Dim, &bytes(done))
        } else {
            tint(Hue::Dim, note)
        };
    };

    #[allow(clippy::cast_precision_loss)]
    let fraction = done as f64 / total as f64;
    let (filled, empty) = mark::bar(fraction, width);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as u8;

    let mut out = format!(
        "{}{}{}{}  {}",
        tint(Hue::Dim, mark::bar_start()),
        tint(Hue::Brand, &filled),
        empty,
        tint(Hue::Dim, mark::bar_end()),
        tint(Hue::Text, &format!("{percent}%")),
    );
    if !note.is_empty() {
        let _ = write!(out, "   {}", tint(Hue::Dim, note));
    }
    out
}

/// A byte count a person can read at a glance.
///
/// **Powers of ten, not of two.** The number beside it is the one the download's own
/// progress showed and the one the vendor's page advertises, and a bar that said 396 MB for
/// a file everybody else calls 415 MB is a bar somebody has to reconcile.
#[must_use]
pub fn bytes(count: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let size = count as f64;
    for (limit, suffix) in [
        (1_000_000_000.0, "GB"),
        (1_000_000.0, "MB"),
        (1_000.0, "kB"),
    ] {
        if size >= limit {
            return format!("{:.1} {suffix}", size / limit);
        }
    }
    format!("{count} B")
}

/// Colour, honouring everything the user said about colour.
fn tint(hue: Hue, text: &str) -> String {
    if crate::ui::paint::coloured() {
        style::in_hue(hue, text)
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
