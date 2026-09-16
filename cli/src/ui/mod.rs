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
pub mod paint;
pub mod screen;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::io::{IsTerminal as _, Write};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use ask::{Answer, Asking};
use screen::{Face, Flow, Kept, Screen, Shell};

/// The last item on every menu below the root.
const BACK: &str = "← Back";

/// The last item on the root, where there is nothing behind.
const QUIT: &str = "Quit";

/// Open the menu.
///
/// Returns what the session produced, for `main` to print once the terminal is its own
/// again. The exit code is `0`: a menu somebody looked at and left has not failed.
pub fn run(shell: &mut Shell) -> Outcome<(Exit, Vec<Kept>)> {
    at_a_terminal()?;

    // Windows before the VT-enabled console needs telling that escape sequences are
    // sequences rather than text. `crossterm` owns that switch, asks the console once, and
    // caches the answer; on everything else it is not compiled in at all.
    #[cfg(windows)]
    let _ = crossterm::ansi_support::supports_ansi();

    ask::dress();

    let screen = Alternate::entered(std::io::stderr())?;
    let mut asking = ask::Terminal::default();
    let outcome = walk(shell, &mut asking);

    // Explicit as well as on drop, so the terminal is back before anything is printed
    // about what happened in it — and still on drop, because a panic has to put it back
    // too. `panic = "abort"` is deliberately not set in `Cargo.toml` for this reason.
    drop(screen);

    outcome.map(|kept| (Exit::Success, kept))
}

/// The render loop.
///
/// Split out so that the whole of the navigation can be driven by a written-down list of
/// answers in a test, with no terminal anywhere near it.
fn walk(shell: &mut Shell, asking: &mut dyn Asking) -> Outcome<Vec<Kept>> {
    let mut history: Vec<Screen> = Vec::new();
    let mut here = shell.opening();
    let mut kept: Vec<Kept> = Vec::new();

    loop {
        let flow = show(&mut here, shell, asking, &mut kept, history.is_empty())?;

        match flow {
            Flow::Stay => {}
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
            Flow::Quit => break,
        }
    }

    Ok(kept)
}

/// Draw one screen and answer it.
fn show(
    here: &mut Screen,
    shell: &mut Shell,
    asking: &mut dyn Asking,
    kept: &mut Vec<Kept>,
    at_root: bool,
) -> Outcome<Flow> {
    asking.frame(&here.header(shell))?;

    // **The way out is added here, not by the screen.** That is what makes "`← Back` on
    // every menu" a property of the loop rather than a thing each new screen has to
    // remember — a screen added in `R19` cannot forget it, because it never had it.
    let out_of_it = if at_root { QUIT } else { BACK };
    let leaving = if at_root { Flow::Quit } else { Flow::Back };

    match here.face() {
        Face::Menu(menu) => {
            let last = menu.items.len();
            match asking.choose(menu.question, &menu.items, out_of_it, here.cursor())? {
                Answer::Given(index) if index >= last => Ok(leaving),
                Answer::Given(index) => {
                    // Written back before the screen is pushed, so coming back finds the
                    // highlight where it was left rather than at the top.
                    here.point_at(index);
                    Ok(here.chose(shell, index))
                }
                Answer::Back => Ok(leaving),
                Answer::Quit => Ok(Flow::Quit),
            }
        }

        Face::Ask(ask) => match asking.text(&ask)? {
            Answer::Given(given) => {
                // Same reason: what was typed is kept on the screen, so that going
                // forward and coming back finds it still typed.
                here.holding(&given);
                Ok(here.typed(shell, kept, &given))
            }
            Answer::Back => Ok(leaving),
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
