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
//! *A screen stack, not nested prompts.* [`screen`] holds the tree; this module holds the
//! `Vec<Screen>` behind the one being shown. Going forward pushes the current screen as it
//! stands, going back pops it out again, and because a screen is a value rather than a call
//! frame, nothing is lost either way.
//!
//! *Never prompt without a terminal.* The menu is nothing but prompts, so it refuses at the
//! door and names what to do instead — rule 4, one screen earlier than usual.

pub mod ask;
pub mod flow;
pub mod paint;
pub mod screen;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::io::{IsTerminal as _, Write};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use ask::{Answer, Asking};
use flow::{Answers, Doing};
use screen::{Face, Flow, Kept, Leaf, Row, Screen, Shell};

/// The last item on every menu below the root.
const BACK: &str = "← Back";

/// The last item on the root, where there is nothing behind.
const QUIT: &str = "Quit";

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
    let outcome = walk(shell, &mut asking, world, &mut screen);

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
    stage: &mut dyn Stage,
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
            Flow::Home => {
                history.clear();
                here = Screen::Home { cursor: 0 };
            }
            Flow::Run(leaf, answers) => {
                here = did(leaf, &answers, world, stage, &mut last).map_or_else(
                    |said| here.troubled(said),
                    |()| {
                        // A job that worked is done with: back to the group it came from,
                        // with its questions forgotten. Coming back to a filled-in flow
                        // would be an invitation to run it twice.
                        history.pop().unwrap_or(Screen::Home { cursor: 0 })
                    },
                );
                shell.holds = world.databases().len();
            }
            Flow::Quit => break,
        }
    }

    Ok((last, kept))
}

/// Hand the terminal back, run the job on it, and take the terminal again.
///
/// **This is why a command's output is worth anything.** Inside the alternate screen every
/// line printed is gone the moment the menu redraws; out here it lands in the scrollback the
/// user keeps, alongside the prompts the command asks for itself — a password, or the name
/// of a database being destroyed. Those stay the command's own questions, asked the same way
/// they are asked from a shell, which is the only way rules 3 and 5 have one implementation.
///
/// `Err` is the sentence to put on the flow's screen. The job has already said it in full on
/// the way past; this is the reminder, once the menu is back.
fn did(
    leaf: Leaf,
    answers: &Answers,
    world: &mut dyn Doing,
    stage: &mut dyn Stage,
    last: &mut Exit,
) -> Result<(), String> {
    stage.step_out();
    let outcome = world.run(leaf.job, answers);
    let said = match &outcome {
        Ok(exit) => {
            *last = *exit;
            None
        }
        Err(failure) => {
            *last = failure.exit();
            failure.mention();
            Some(failure.message().to_owned())
        }
    };
    stage.pause();
    stage.step_in();

    match said {
        None => Ok(()),
        Some(said) => Err(said),
    }
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
    let out_of_it = if at_root { QUIT } else { BACK };
    let leaving = if at_root { Flow::Quit } else { Flow::Back };

    // Inside a flow, back is one *question* back and the flow says so itself. Only when it
    // has no answers left to drop does the loop take over and leave the screen.
    let back = |here: &mut Screen| {
        if here.stepped_back() {
            Flow::Stay
        } else {
            leaving
        }
    };

    match here.face(world) {
        Face::Menu(menu) => {
            let rows = menu.rows();
            match asking.choose(
                &menu.question,
                &rows,
                out_of_it,
                settled(&rows, here.cursor()),
            )? {
                Answer::Given(row) if row >= rows.len() => Ok(back(here)),
                Answer::Given(row) => {
                    // Written back before the screen is pushed, so coming back finds the
                    // highlight where it was left rather than at the top.
                    here.point_at(row);
                    match rows.get(row) {
                        // **A heading is not a thing that can be chosen.** `inquire` owns
                        // the key loop and has no notion of a row the cursor skips, so
                        // landing on one moves to the first thing under it and draws again
                        // — which is what Enter on a heading should do anyway.
                        Some(Row::Heading(_)) => {
                            here.point_at(row + 1);
                            Ok(Flow::Stay)
                        }
                        Some(Row::Item(_, chosen)) => Ok(here.chose(shell, world, *chosen)),
                        None => Ok(Flow::Stay),
                    }
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

/// Where the highlight starts, given where it was left.
///
/// **Never on a heading.** A list that opens with the cursor on one is a list whose first
/// Enter does nothing but move down, and that is a bad first keystroke for somebody who has
/// just arrived. The same rule catches a highlight restored from a screen whose sections
/// have since changed shape.
fn settled(rows: &[Row], at: usize) -> usize {
    let first_choice = rows
        .iter()
        .position(|row| matches!(row, Row::Item(..)))
        .unwrap_or(0);

    match rows.get(at) {
        Some(Row::Item(..)) => at,
        // Forward to the next thing that can be chosen, and failing that the first one:
        // a cursor past the end belongs at the top rather than nowhere.
        _ => rows
            .iter()
            .enumerate()
            .skip(at)
            .find(|(_, row)| matches!(row, Row::Item(..)))
            .map_or(first_choice, |(row, _)| row),
    }
}

/// The screen the menu is drawn on, and how to step off it and back.
///
/// A trait for the same reason [`Asking`] is one: a test drives the whole loop, jobs and
/// all, and there is no terminal anywhere near it.
pub trait Stage {
    /// Hand the terminal back.
    fn step_out(&mut self);
    /// Wait for whoever is there to finish reading what the job printed.
    fn pause(&mut self);
    /// Take it again.
    fn step_in(&mut self);
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

    /// Take it again, after a job has run on the terminal underneath.
    fn enter(&mut self) {
        if self.inside {
            return;
        }
        self.inside = true;
        let _ = crossterm::execute!(self.to, crossterm::terminal::EnterAlternateScreen);
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

impl<W: Write> Stage for Alternate<W> {
    fn step_out(&mut self) {
        self.leave();
    }

    /// **Wait, before the menu paints over what just happened.** A backup's report, a row
    /// count, a generated password — every one of them is on the screen for as long as it
    /// takes to redraw unless somebody says they have read it. It is still in the scrollback
    /// afterwards; this is so it does not have to be hunted for.
    fn pause(&mut self) {
        if !std::io::stdin().is_terminal() {
            return;
        }
        let _ = writeln!(self.to);
        let _ = write!(self.to, "{}", paint::dim("  Enter to go back to the menu "));
        let _ = self.to.flush();
        let _ = std::io::stdin().read_line(&mut String::new());
    }

    fn step_in(&mut self) {
        self.enter();
    }
}
