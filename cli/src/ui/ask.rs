//! How the render loop puts a question to whoever is sitting there.
//!
//! **A trait, because the loop has to be testable and a terminal is not.** Driving the real
//! thing needs a pseudo-terminal, which needs a crate, which is not a trade this project
//! makes for a menu. So the loop asks an [`Asking`] for each answer, the terminal is one
//! implementation of that and a written-down list of answers is another — and the walk that
//! proves `← Back` comes home from every screen in the tree is an ordinary unit test.
//!
//! **Esc is `Back` and Ctrl-C is `Quit`, once, here.** `inquire` reports both as errors,
//! and an error is the wrong shape for something the user did on purpose: every screen
//! would have to remember which two errors are not failures. They become an [`Answer`] at
//! the edge instead, and past this module a cancelled prompt is a navigation.
//!
//! **The list is drawn here and the box is `inquire`'s, and the arrow is why.** A menu
//! under headings needs a highlight that steps over them, and `inquire` owns its own key
//! loop with no notion of a row the cursor skips: a heading could be made harmless to
//! *choose*, but it could still be *landed on*, and an arrow resting beside a word that
//! does nothing is a menu somebody has to be told about. So the list below is `crossterm`,
//! moving between the lines that can be chosen and drawing the rest. A text box has no such
//! problem and stays `inquire`'s, along with its editing and its cursor handling.

use std::fmt::Write as _;
use std::io::Write as _;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::style::Hue;

use super::paint::{self, Header};
use super::screen::{Ask, Item};

/// What came back from a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer<T> {
    /// They answered it.
    Given(T),
    /// Esc. One screen back.
    Back,
    /// Ctrl-C. Put the terminal back and leave.
    Quit,
}

/// Somebody to put a question to.
pub trait Asking {
    /// Wipe the screen and draw the header on it.
    fn frame(&mut self, header: &Header) -> Outcome<()>;

    /// A list. `at` is the row the highlight starts on, and `way_out` is the last row —
    /// `← Back` everywhere but the root, where there is nothing behind it and it says
    /// `Quit`. What comes back is the row that was chosen, or `items.len()` for the way out.
    fn choose(
        &mut self,
        question: &str,
        items: &[Item],
        way_out: &str,
        at: usize,
    ) -> Outcome<Answer<usize>>;

    /// One line to type.
    fn text(&mut self, ask: &Ask) -> Outcome<Answer<String>>;
}

/// The real terminal.
#[derive(Debug, Default)]
pub struct Terminal {
    /// How many rows the last header took, so the list below it knows what is left.
    drawn: usize,
    /// The header itself, because the list redraws the whole screen on every keystroke and
    /// the header is part of the screen.
    header: String,
}

/// How wide the screen is, or `None` if it will not say.
pub(super) fn columns() -> Option<usize> {
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
}

/// How tall it is, the same way.
pub(super) fn rows() -> Option<usize> {
    crossterm::terminal::size()
        .ok()
        .map(|(_, rows)| usize::from(rows))
}

impl Terminal {
    /// How wide the screen is, or `None` if it will not say.
    fn columns() -> Option<usize> {
        columns()
    }

    /// How many rows the list itself may use.
    ///
    /// The header, a blank line, the question, a blank line and the help line are already
    /// spoken for. Never fewer than three, or the list is a keyhole.
    fn list_height(&self) -> usize {
        let height = crossterm::terminal::size()
            .ok()
            .map_or(24, |(_, rows)| usize::from(rows));
        height.saturating_sub(self.drawn + 5).max(3)
    }

    /// Draw the whole screen: the header, the question, the list and the help line.
    ///
    /// **Built as one string and written once.** A screen drawn in twenty writes flickers
    /// on every keystroke, and a menu that flickers reads as a menu that is struggling.
    ///
    /// **And it is never blanked first, which is the other half of that.** Clearing the
    /// screen and then drawing it leaves a real, visible empty frame in between — the
    /// terminal paints the blank, then paints the text — so every arrow key made the whole
    /// menu blink. Instead every line is overwritten where it already is and erased to the
    /// right edge as it goes ([`erase_line`]), and only the rows *below* the new frame are
    /// cleared. Nothing is ever blank, so there is nothing to flicker.
    fn paint(&self, screen: &Screenful<'_>) -> Outcome<()> {
        let mut drawn = String::new();
        for line in self.header.lines() {
            drawn.push_str(line);
            drawn.push_str("\r\n");
        }
        let _ = write!(
            drawn,
            "\r\n{} {}\r\n",
            paint::accent("?"),
            paint::hue(Hue::Text, screen.question)
        );

        let last = (screen.top + screen.height).min(screen.lines.len());
        for (at, line) in screen.lines.iter().enumerate().take(last).skip(screen.top) {
            // The arrow, and the two columns it lives in. Only a line that can be chosen
            // ever gets one, which is the whole of "the gap is not a navigation item".
            let (lead, text) = if Some(at) == screen.here {
                (
                    paint::accent(&format!("{} ", crate::mark::Mark::Here.glyph())),
                    paint::chosen(&line.text),
                )
            } else {
                ("  ".to_owned(), line.text.clone())
            };
            let _ = write!(drawn, "{lead}{text}\r\n");
        }

        // Which way there is more, when there is more.
        let more = match (screen.top > 0, last < screen.lines.len()) {
            (true, true) => Some("↑↓ more above and below"),
            (true, false) => Some("↑ more above"),
            (false, true) => Some("↓ more below"),
            (false, false) => None,
        };
        if let Some(more) = more {
            let _ = write!(drawn, "  {}\r\n", paint::dim(more));
        }

        let _ = write!(drawn, "\r\n  {}\r\n", paint::dim(&help(screen.filter)));

        let mut out = std::io::stderr();
        write!(out, "{}", redrawn_in_place(&drawn, "\r\n", true))
            .map_err(|error| drawing(&error))?;
        out.flush().map_err(|error| drawing(&error))
    }
}

/// A frame that replaces the one already on screen without ever blanking it.
///
/// **One string, cursor movement included.** `queue!` against standard error is not buffered,
/// so each of its commands would reach the terminal as its own write and the frame would
/// arrive in pieces. Everything goes into the string instead — which is also where the
/// colours already are, so this carries no assumption the rest of the drawing did not already
/// make.
///
/// Home, then every line overwritten where it already is and erased to the right edge as it
/// goes, then the rows below the new frame cleared. There is no moment at which the screen is
/// empty, which is the whole of the fix: `Clear(All)` followed by a redraw paints a real blank
/// frame first, and that is what made every arrow key blink the menu.
pub(super) fn redrawn_in_place(drawn: &str, ending: &str, hide_cursor: bool) -> String {
    let erase = ansi(crossterm::terminal::Clear(
        crossterm::terminal::ClearType::UntilNewLine,
    ));

    format!(
        "{}{}{}{}",
        if hide_cursor {
            ansi(crossterm::cursor::Hide)
        } else {
            String::new()
        },
        ansi(crossterm::cursor::MoveTo(0, 0)),
        drawn.replace(ending, &format!("{erase}{ending}")),
        ansi(crossterm::terminal::Clear(
            crossterm::terminal::ClearType::FromCursorDown
        )),
    )
}

/// One `crossterm` command, as the characters that perform it.
///
/// Taken from `crossterm` rather than written out by hand, so there is still exactly one
/// place in this program that knows how a terminal spells "move the cursor".
fn ansi(command: impl crossterm::Command) -> String {
    let mut written = String::new();
    let _ = command.write_ansi(&mut written);
    written
}

/// Everything one drawing of a list needs to know.
struct Screenful<'a> {
    question: &'a str,
    lines: &'a [Drawn],
    filter: &'a str,
    /// The line the highlight is on, if any of them is on screen.
    here: Option<usize>,
    /// The first line shown.
    top: usize,
    /// How many are shown.
    height: usize,
}

impl Asking for Terminal {
    fn frame(&mut self, header: &Header) -> Outcome<()> {
        let drawn = paint::frame(header, Self::columns());
        self.drawn = drawn.lines().count();
        self.header.clone_from(&drawn);

        // Overwritten in place and cleared below, not blanked and redrawn — the same reason
        // as [`Terminal::paint`], and it matters here too: this runs on every move between
        // screens, and a blank frame between two menus is the same blink.
        //
        // Through `crossterm`, not through `report`: this is inside the alternate screen,
        // where nothing survives and nothing should be logged. What is worth keeping is
        // printed after the screen has been handed back.
        let mut out = std::io::stderr();
        write!(
            out,
            "{}",
            redrawn_in_place(&format!("{drawn}\n\n"), "\n", false)
        )
        .map_err(|error| drawing(&error))?;
        out.flush().map_err(|error| drawing(&error))
    }

    fn choose(
        &mut self,
        question: &str,
        items: &[Item],
        way_out: &str,
        at: usize,
    ) -> Outcome<Answer<usize>> {
        let widths = paint::widths(items.iter().map(|item| {
            (
                item.title.as_str(),
                item.note.as_str(),
                item.command.as_str(),
            )
        }));
        let columns = Self::columns();

        let _raw = Raw::on()?;
        let mut filter = String::new();
        let mut wanted = at;
        let mut top = 0;

        loop {
            let lines = lay_out(items, way_out, &filter, widths, columns);
            let reachable: Vec<usize> = (0..lines.len())
                .filter(|line| lines[*line].picks.is_some())
                .collect();

            // Where the highlight actually is. It follows the row it was on for as long as
            // that row is still drawn, and falls back to the first one that is when a
            // filter has taken it away.
            let here = reachable
                .iter()
                .position(|line| lines[*line].picks == Some(wanted))
                .unwrap_or(0);
            if let Some(settled) = reachable.get(here).and_then(|line| lines[*line].picks) {
                wanted = settled;
            }

            let height = self.list_height();
            let on = reachable.get(here).copied();
            top = window(top, on.unwrap_or(0), lines.len(), height);
            self.paint(&Screenful {
                question,
                lines: &lines,
                filter: &filter,
                here: on,
                top,
                height,
            })?;

            let Some(key) = pressed()? else { continue };
            match key {
                Key::Up => wanted = step(&lines, &reachable, here, -1).unwrap_or(wanted),
                Key::Down => wanted = step(&lines, &reachable, here, 1).unwrap_or(wanted),
                Key::Home => wanted = ends(&lines, &reachable, true).unwrap_or(wanted),
                Key::End => wanted = ends(&lines, &reachable, false).unwrap_or(wanted),
                Key::Page(by) => {
                    let leap = by * i64::try_from(height).unwrap_or(1);
                    wanted = step(&lines, &reachable, here, leap)
                        .unwrap_or(ends(&lines, &reachable, by < 0).unwrap_or(wanted));
                }
                Key::Enter => return Ok(Answer::Given(wanted)),
                Key::Back => return Ok(Answer::Back),
                Key::Quit => return Ok(Answer::Quit),
                Key::Typed(letter) => filter.push(letter.to_ascii_lowercase()),
                Key::Rubbed => {
                    filter.pop();
                }
                Key::Redraw => {}
            }
        }
    }

    fn text(&mut self, ask: &Ask) -> Outcome<Answer<String>> {
        let answer = inquire::Text::new(&ask.question)
            .with_initial_value(&ask.initial)
            .with_help_message(&ask.help)
            .prompt();

        match answer {
            Ok(given) => Ok(Answer::Given(given)),
            Err(error) => navigate(&error),
        }
    }
}

/// One drawn line of a list, and what choosing it means.
///
/// **A gap has nothing to choose, and that is the whole point of this type.** The highlight
/// only ever moves between lines carrying a `picks`, so the blank row above the way out is
/// drawn, scrolled past and stepped over, and the arrow can no more rest on it than on the
/// space between two words.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Drawn {
    /// The line, painted.
    text: String,
    /// The item it stands for, or nothing when it is a gap. The way out is `items.len()`,
    /// which is what [`Asking::choose`] promises its caller.
    picks: Option<usize>,
}

/// Lay the items out as lines, keeping only what matches `filter`.
///
/// **The filter reads more than it draws.** `Item::finds` carries the names of every command
/// behind a door, so typing `mirror` on the home screen lands on the door that holds it —
/// which is the objection that reopened the doors the first time.
fn lay_out(
    items: &[Item],
    way_out: &str,
    filter: &str,
    widths: paint::Widths,
    columns: Option<usize>,
) -> Vec<Drawn> {
    let mut lines: Vec<Drawn> = Vec::new();
    for (at, item) in items.iter().enumerate() {
        if item.matches(filter) {
            lines.push(Drawn {
                text: paint::option(
                    &item.title,
                    &item.note,
                    item.hue,
                    &item.command,
                    widths,
                    columns,
                ),
                picks: Some(at),
            });
        }
    }

    // The way out, always last and always reachable, set apart from the list above it.
    lines.push(Drawn::gap());
    lines.push(Drawn {
        text: way_out.to_owned(),
        picks: Some(items.len()),
    });
    lines
}

impl Drawn {
    fn gap() -> Self {
        Self {
            text: String::new(),
            picks: None,
        }
    }
}

/// What a keypress means to a list.
pub(super) enum Key {
    Up,
    Down,
    Home,
    End,
    /// A screenful, up or down.
    Page(i64),
    Enter,
    Back,
    Quit,
    /// A letter typed into the filter.
    Typed(char),
    /// Backspace.
    Rubbed,
    /// Something that is not a key — a resize — and the screen needs drawing again.
    Redraw,
}

/// The next thing the user did, or nothing when it was a key coming back up.
///
/// **Windows sends both halves of every keystroke.** A loop that acted on the release as
/// well as the press would move the highlight two rows for one press of the down arrow,
/// which on a five-item menu is a menu that skips every other thing on it.
pub(super) fn pressed() -> Outcome<Option<Key>> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    let event = crossterm::event::read().map_err(|error| drawing(&error))?;
    let Event::Key(key) = event else {
        return Ok(Some(Key::Redraw));
    };
    if key.kind == KeyEventKind::Release {
        return Ok(None);
    }

    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    Ok(Some(match key.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::Page(-1),
        KeyCode::PageDown => Key::Page(1),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Back,
        // Ctrl-C leaves, and so does Ctrl-D: in raw mode nothing else is going to read
        // either of them, and a terminal that ignored both is one somebody has to kill.
        KeyCode::Char('c' | 'd') if control => Key::Quit,
        KeyCode::Backspace => Key::Rubbed,
        KeyCode::Char(letter) if !control => Key::Typed(letter),
        _ => Key::Redraw,
    }))
}

/// The row `by` places from `here`, or nothing when there is nowhere to go.
///
/// **Counted in things that can be chosen, not in rows.** That is what makes a heading
/// unreachable rather than merely harmless: it is not in this list at all, so the highlight
/// steps straight over it and the arrow is never beside one.
fn step(lines: &[Drawn], reachable: &[usize], here: usize, by: i64) -> Option<usize> {
    let to = usize::try_from(i64::try_from(here).ok()? + by).ok()?;
    lines.get(*reachable.get(to)?).and_then(|line| line.picks)
}

/// The first thing that can be chosen, or the last.
fn ends(lines: &[Drawn], reachable: &[usize], first: bool) -> Option<usize> {
    let line = if first {
        reachable.first()?
    } else {
        reachable.last()?
    };
    lines.get(*line).and_then(|line| line.picks)
}

/// Which line the visible window starts at, given where the highlight is.
///
/// Scrolls by as little as it can: the window only moves when the highlight would leave it,
/// so a list that fits never scrolls and one that does not stays where the eye left it.
fn window(top: usize, here: usize, lines: usize, height: usize) -> usize {
    if lines <= height {
        return 0;
    }
    let top = top.min(lines.saturating_sub(height));
    if here < top {
        here
    } else if here >= top + height {
        here.saturating_sub(height - 1)
    } else {
        top
    }
}

/// The line under the list, and the filter when there is one.
fn help(filter: &str) -> String {
    if filter.is_empty() {
        "↑↓ to move, enter to choose, esc to go back, or type a few letters to filter".to_owned()
    } else {
        format!("showing what matches {filter:?} — backspace to widen it again")
    }
}

/// Raw mode, and the promise that it is turned off again.
///
/// The same reason the alternate screen is a type: an early return or a panic that left the
/// terminal in raw mode would leave a shell that does not echo what is typed at it, and the
/// user would have to `reset` it.
pub(super) struct Raw;

impl Raw {
    pub(super) fn on() -> Outcome<Self> {
        crossterm::terminal::enable_raw_mode().map_err(|error| drawing(&error))?;
        Ok(Self)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stderr(), crossterm::cursor::Show);
    }
}

/// `inquire` reports a finished prompt that produced no answer as an error. Two of its
/// variants are not failures at all — they are the user navigating — and the rest end the
/// session.
fn navigate<T>(error: &inquire::InquireError) -> Outcome<Answer<T>> {
    match error {
        inquire::InquireError::OperationCanceled => Ok(Answer::Back),
        inquire::InquireError::OperationInterrupted => Ok(Answer::Quit),
        inquire::InquireError::NotTTY => Err(Failure::new(
            Exit::Usage,
            "the menu needs a terminal, and this is not one",
        )
        .hint("run the command directly instead — `sloop --help` lists every one of them")),
        other => Err(Failure::new(
            Exit::Failure,
            format!("the menu could not draw itself: {other}"),
        )
        .hint("run the command directly instead — `sloop --help` lists every one of them")),
    }
}

/// Anything that went wrong writing to the screen.
fn drawing(error: &std::io::Error) -> Failure {
    Failure::new(
        Exit::Failure,
        format!("the menu could not draw itself: {error}"),
    )
}

/// The palette, in `inquire`'s own colour type.
fn ink(hue: Hue) -> inquire::ui::Color {
    match crate::style::ink(hue) {
        crate::style::Ink::True(r, g, b) => inquire::ui::Color::Rgb { r, g, b },
        crate::style::Ink::Cube(index) => inquire::ui::Color::AnsiValue(index),
    }
}

/// How `inquire` paints the one thing it still draws: a box to type in.
///
/// **Set once for the process**, because `inquire` keeps one global config and a per-prompt
/// copy would be the same six lines at every call site with one of them eventually wrong.
/// When the user has said not to colour, this is `RenderConfig::empty()`, which writes no
/// escapes at all rather than escapes somebody downstream has to strip.
///
/// **Every colour below is one of [`crate::style`]'s six**, so a box drawn by `inquire` and
/// a list drawn here are the same screen rather than two things that happen to be adjacent.
pub fn dress() {
    use inquire::ui::{RenderConfig, StyleSheet, Styled};

    if !paint::coloured() {
        inquire::set_global_render_config(RenderConfig::empty());
        return;
    }

    let config = RenderConfig::default_colored()
        // `?` in the accent, which is exactly what the flag surface prints in front of a
        // question, and exactly what the list above prints too. One tool, one way of
        // asking. See `consent`.
        .with_prompt_prefix(Styled::new("?").with_fg(ink(Hue::Brand)))
        .with_answered_prompt_prefix(Styled::new("·").with_fg(ink(Hue::Dim)))
        .with_answer(StyleSheet::new().with_fg(ink(Hue::Ok)))
        .with_help_message(StyleSheet::new().with_fg(ink(Hue::Dim)))
        .with_default_value(StyleSheet::new().with_fg(ink(Hue::Dim)))
        .with_text_input(StyleSheet::new().with_fg(ink(Hue::Text)))
        .with_canceled_prompt_indicator(Styled::new("← back").with_fg(ink(Hue::Dim)));

    inquire::set_global_render_config(config);
}

#[cfg(test)]
#[path = "ask_tests.rs"]
mod tests;
