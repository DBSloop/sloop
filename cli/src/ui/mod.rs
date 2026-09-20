//! The interactive shell: one alternate screen, one render loop, one stack of screens.
//!
//! ```text
//!   sloop                     the menu opens
//!     Home ──► Databases ──► Tell sloop about a database
//!          ◄── ← Back     ◄── ← Back
//! ```
//!
//! **Three rules shape everything in here, and all three are `CLAUDE.md`'s.**
//!
//! *The alternate screen, not `clear`.* Whatever the user had in their terminal is still
//! there when sloop exits, and nothing the menu drew is in the scrollback. The corollary is
//! that **nothing printed inside the screen survives it**, so anything worth keeping is
//! collected as it happens and printed afterwards, once the terminal has been handed back.
//!
//! *And the screen is never given back until the session ends.* A job used to leave the
//! alternate screen, run on the terminal underneath, and take it again — so its output, its
//! progress and its password prompts all landed in the scrollback. The owner reversed that:
//! *"sloop will not come to the traditional console again until we quit"*. A job now runs on
//! [`live`], every line and every question with it, and what it did is read afterwards on a
//! screen of its own. See "The job runs inside the screen" in `docs/OWNER-DECISIONS.md`.
//!
//! *A screen stack, not nested prompts.* [`screen`] holds the tree; this module holds the
//! `Vec<Screen>` behind the one being shown. Going forward pushes the current screen as it
//! stands, going back pops it out again, and because a screen is a value rather than a call
//! frame, nothing is lost either way.
//!
//! *Never prompt without a terminal.* The menu is nothing but prompts, so it refuses at the
//! door and names what to do instead — rule 4, one screen earlier than usual.

pub mod ask;
pub mod equivalent;
pub mod flow;
pub mod live;
pub mod paint;
pub mod screen;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::io::{IsTerminal as _, Write};
use std::time::{Duration, Instant};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use ask::{Answer, Asking};
use flow::{Answers, Doing, Job};
use screen::{Face, Flow, Kept, Leaf, Screen, Shell};

/// The last item on every menu below the root.
const BACK: &str = "← Back";

/// The last item on the root, where there is nothing behind.
const QUIT: &str = "Quit";

/// The last item on a screen a job has finished on.
///
/// **Not `← Back`, because there is nothing behind a result worth stepping through.** See
/// [`Screen::is_an_outcome`].
const HOME: &str = "← Home";

/// Open the menu.
///
/// Returns the code the session leaves with and whatever is worth printing once the
/// terminal is its own again. A menu somebody looked at and left exits `0`; a menu they ran
/// a command from exits with what that command exited with, because the last thing a run
/// did is the useful thing to report.
pub fn run(shell: &mut Shell, world: &mut dyn Doing) -> Outcome<(Exit, Vec<Kept>)> {
    at_a_terminal()?;

    // Windows before the VT-enabled console needs telling that escape sequences are
    // sequences rather than text. `crossterm` owns that switch, asks the console once, and
    // caches the answer; on everything else it is not compiled in at all.
    #[cfg(windows)]
    let _ = crossterm::ansi_support::supports_ansi();

    ask::dress();

    let mut screen = Alternate::entered(std::io::stderr())?;
    let mut asking = ask::Terminal::default();
    let outcome = walk(shell, &mut asking, world);

    // Explicit as well as on drop, so the terminal is back before anything is printed
    // about what happened in it — and still on drop, because a panic has to put it back
    // too. `panic = "abort"` is deliberately not set in `Cargo.toml` for this reason.
    screen.leave();
    drop(screen);

    outcome
}

/// The render loop.
///
/// Split out so that the whole of the navigation can be driven by a written-down list of
/// answers in a test, with no terminal anywhere near it.
fn walk(
    shell: &mut Shell,
    asking: &mut dyn Asking,
    world: &mut dyn Doing,
) -> Outcome<(Exit, Vec<Kept>)> {
    let mut history: Vec<Screen> = Vec::new();
    let mut here = shell.opening();
    let mut kept: Vec<Kept> = Vec::new();
    let mut last = Exit::Success;

    loop {
        let flow = show(
            &mut here,
            shell,
            asking,
            world,
            &mut kept,
            history.is_empty(),
        )?;

        match flow {
            Flow::Stay => {}
            Flow::Same(next) => here = next,
            Flow::To(next) => {
                history.push(here);
                here = next;
            }
            Flow::Back => match history.pop() {
                Some(previous) => here = previous,
                // Nothing behind the root: back out of it and the session is over.
                None => break,
            },
            // **The top of the tree, whichever screen that is.** A machine that has not
            // been set up has no working home screen — every door on it would open a menu
            // whose every item fails — so `Home` there means Setup, and it stops meaning
            // Setup the moment Setup has run.
            Flow::Home => {
                history.clear();
                here = shell.opening();
            }
            Flow::Run(leaf, answers) => {
                // **What `← Back` means from an outcome.** A flow already has the door it
                // came from behind it on the history, and its questions are answered, so it
                // is dropped; anything else — Setup, or a second run started from an outcome
                // — is what going back should land on.
                let from = std::mem::replace(&mut here, Screen::Home { cursor: 0 });
                if !matches!(
                    from,
                    Screen::Doing { .. } | Screen::Done { .. } | Screen::Failed { .. }
                ) {
                    history.push(from);
                }
                here = did(leaf, answers, world, &mut last);
                shell.standing = world.standing();
                // **Setup is over the moment it works.** The shell worked out `set_up` when
                // the session opened; without this the machine stays "not set up" for the
                // rest of it, and `Home` keeps meaning the screen it just came from.
                if leaf.job == Job::Setup && matches!(here, Screen::Done { .. }) {
                    shell.set_up = true;
                }
            }
            Flow::Quit => break,
        }
    }

    Ok((last, kept))
}

/// Run the job, on a screen of its own, and hand back the screen that says what it did.
///
/// **Nothing is handed back to the terminal underneath.** The job's lines, its steps and its
/// questions all go to [`live::Live`], which draws them inside the alternate buffer; what it
/// collected then goes onto the outcome screen, which is the thing somebody reads.
fn did(leaf: Leaf, answers: Box<Answers>, world: &mut dyn Doing, last: &mut Exit) -> Screen {
    let started = Instant::now();
    let live = live::Live::opened(&[leaf.under, leaf.title], leaf.blurb);
    let outcome = live::under(&live, || world.run(leaf.job, &answers));
    let told = live.transcript();
    let took = spoken(started.elapsed());

    match outcome {
        Ok(exit) => {
            *last = exit;
            Screen::Done {
                leaf,
                // **`R20`, moved onto the screen.** It used to be printed after the terminal
                // was handed back, which is a place that no longer exists — so the line a
                // session becomes lives on the screen that reports the session.
                same: equivalent::line(leaf.job, &answers, world),
                told,
                took,
                cursor: 0,
            }
        }
        Err(failure) => {
            *last = failure.exit();
            Screen::Failed {
                leaf,
                answers,
                told,
                said: failure.message().to_owned(),
                hint: failure.hint_text().map(ToOwned::to_owned),
                exit: failure.exit(),
                cursor: 0,
            }
        }
    }
}

/// How long something took, for the corner of an outcome.
fn spoken(took: Duration) -> String {
    let seconds = took.as_secs();
    if seconds < 60 {
        return format!("{}.{:01}s", seconds, took.subsec_millis() / 100);
    }
    format!("{}m {:02}s", seconds / 60, seconds % 60)
}

/// Draw one screen and answer it.
fn show(
    here: &mut Screen,
    shell: &mut Shell,
    asking: &mut dyn Asking,
    world: &mut dyn Doing,
    kept: &mut Vec<Kept>,
    at_root: bool,
) -> Outcome<Flow> {
    asking.frame(&here.header(shell, world))?;

    // **The way out is added here, not by the screen.** That is what makes "`← Back` on
    // every menu" a property of the loop rather than a thing each new screen has to
    // remember — a screen added later cannot forget it, because it never had it.
    let homeward = here.is_an_outcome();
    let out_of_it = if homeward {
        HOME
    } else if at_root {
        QUIT
    } else {
        BACK
    };
    let leaving = || {
        if homeward {
            Flow::Home
        } else if at_root {
            Flow::Quit
        } else {
            Flow::Back
        }
    };

    // Inside a flow, back is one *question* back and the flow says so itself. Only when it
    // has no answers left to drop does the loop take over and leave the screen.
    let back = |here: &mut Screen| {
        if here.stepped_back() {
            Flow::Stay
        } else {
            leaving()
        }
    };

    match here.face(&shell.standing, world) {
        Face::Menu(menu) => {
            let items = menu.items;
            match asking.choose(&menu.question, &items, out_of_it, here.cursor())? {
                Answer::Given(row) if row >= items.len() => Ok(back(here)),
                Answer::Given(row) => {
                    // Written back before the screen is pushed, so coming back finds the
                    // highlight where it was left rather than at the top.
                    here.point_at(row);
                    Ok(here.chose(shell, world, row))
                }
                Answer::Back => Ok(back(here)),
                Answer::Quit => Ok(Flow::Quit),
            }
        }

        Face::Ask(ask) => match asking.text(&ask)? {
            Answer::Given(given) => {
                // Same reason: what was typed is kept on the screen, so that going
                // forward and coming back finds it still typed.
                here.holding(&given);
                Ok(here.typed(shell, world, kept, &given))
            }
            Answer::Back => Ok(back(here)),
            Answer::Quit => Ok(Flow::Quit),
        },
    }
}

/// Rule 4, at the door.
///
/// **Both halves are checked.** Standard input is where the keys come from and standard
/// error is where the screen is drawn, and a run with one but not the other cannot hold a
/// conversation either way. There is no flag that opens a menu without a terminal — the
/// answer is a command — so that is what the hint names.
fn at_a_terminal() -> Outcome<()> {
    if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        return Ok(());
    }

    Err(Failure::new(
        Exit::Usage,
        "the menu needs a terminal, and this is not one",
    )
    .hint(
        "name a command instead — `sloop --help` lists every one of them, and every \
             one of them runs without a menu",
    ))
}

/// The alternate screen buffer, entered on the way in and left on the way out.
///
/// **Left on drop, which is the whole reason it is a type.** A `LeaveAlternateScreen` at
/// the end of a function is a `LeaveAlternateScreen` that an early return or a panic skips,
/// and the cost of skipping it is a user's terminal left in a state they have to `reset`.
struct Alternate<W: Write> {
    to: W,
    inside: bool,
}

impl<W: Write> Alternate<W> {
    /// Enter it.
    fn entered(mut to: W) -> Outcome<Self> {
        crossterm::execute!(to, crossterm::terminal::EnterAlternateScreen).map_err(|error| {
            Failure::new(
                Exit::Failure,
                format!("this terminal would not give sloop a screen to draw on: {error}"),
            )
        })?;
        Ok(Self { to, inside: true })
    }

    /// Give it back. Idempotent, because `drop` calls it after anything else has.
    fn leave(&mut self) {
        if !self.inside {
            return;
        }
        self.inside = false;
        // Best effort, and deliberately so: a terminal that has gone away cannot be put
        // back, and failing the run over it would replace one lost session with two.
        let _ = crossterm::execute!(self.to, crossterm::terminal::LeaveAlternateScreen);
        let _ = self.to.flush();
    }
}

impl<W: Write> Drop for Alternate<W> {
    fn drop(&mut self) {
        self.leave();
    }
}
