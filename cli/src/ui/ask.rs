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

    /// A list. `at` is where the highlight starts, `way_out` is the last item — `← Back`
    /// everywhere but the root, where there is nothing behind and it says `Quit`.
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
}

impl Terminal {
    /// How wide the screen is, or `None` if it will not say.
    fn columns() -> Option<usize> {
        crossterm::terminal::size()
            .ok()
            .map(|(columns, _)| usize::from(columns))
    }

    /// How many items a list may show before it starts scrolling.
    ///
    /// The header, the question, the help line and a row of air at the bottom are already
    /// spoken for. Never fewer than three, or the list is a keyhole; never more than
    /// twelve, because a menu longer than that is a menu nobody reads.
    fn page(&self) -> usize {
        let rows = crossterm::terminal::size()
            .ok()
            .map_or(24, |(_, rows)| usize::from(rows));
        rows.saturating_sub(self.drawn + 4).clamp(3, 12)
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
        )),
    }
}

impl Asking for Terminal {
    fn frame(&mut self, header: &Header) -> Outcome<()> {
        let drawn = paint::frame(header, Self::columns());
        self.drawn = drawn.lines().count();

        let mut out = std::io::stderr();
        crossterm::execute!(
            out,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::cursor::MoveTo(0, 0),
        )
        .map_err(|error| drawing(&error))?;

        // Through `crossterm`, not through `report`: this is inside the alternate screen,
        // where nothing survives and nothing should be logged. What is worth keeping is
        // printed after the screen has been handed back.
        writeln!(out, "{drawn}\n").map_err(|error| drawing(&error))?;
        out.flush().map_err(|error| drawing(&error))
    }

    fn choose(
        &mut self,
        question: &str,
        items: &[Item],
        way_out: &str,
        at: usize,
    ) -> Outcome<Answer<usize>> {
        let column = paint::column_for(items.iter().map(|item| item.title.as_str()));
        let columns = Self::columns();

        let mut options: Vec<String> = items
            .iter()
            .map(|item| paint::option(&item.title, &item.blurb, column, columns))
            .collect();
        options.push(way_out.to_owned());

        let answer = inquire::Select::new(question, options)
            .with_starting_cursor(at.min(items.len()))
            .with_page_size(self.page())
            .with_help_message(
                "↑↓ to move, enter to choose, esc to go back, or type a few letters to filter",
            )
            // The line a choice leaves behind, in the moment before the screen is wiped
            // and drawn again. Without this it is the padded row, phrase and all, which
            // reads like a glitch.
            .with_formatter(&|chosen| {
                chosen
                    .value
                    .split_once("  ")
                    .map_or(chosen.value.as_str(), |(title, _)| title)
                    .trim_end()
                    .to_owned()
            })
            .raw_prompt();

        match answer {
            Ok(chosen) => Ok(Answer::Given(chosen.index)),
            Err(error) => navigate(&error),
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

/// How `inquire` is painted.
///
/// **Set once for the process**, because `inquire` keeps one global config and a per-prompt
/// copy would be the same eight lines at every call site with one of them eventually wrong.
/// When the user has said not to colour, this is `RenderConfig::empty()`, which writes no
/// escapes at all rather than escapes somebody downstream has to strip.
///
/// **Every colour below is one of [`crate::style`]'s six**, so a list drawn by `inquire`
/// and a header drawn by `paint` are the same screen rather than two things that happen to
/// be next to each other.
pub fn dress() {
    use inquire::ui::{Attributes, RenderConfig, StyleSheet, Styled};

    if !paint::coloured() {
        inquire::set_global_render_config(RenderConfig::empty());
        return;
    }

    let config = RenderConfig::default_colored()
        // `?` in the accent, which is exactly what the flag surface prints in front of a
        // question. One tool, one way of asking. See `consent`.
        .with_prompt_prefix(Styled::new("?").with_fg(ink(Hue::Brand)))
        .with_answered_prompt_prefix(Styled::new("·").with_fg(ink(Hue::Dim)))
        .with_highlighted_option_prefix(Styled::new("›").with_fg(ink(Hue::Brand)))
        .with_option(StyleSheet::new().with_fg(ink(Hue::Text)))
        .with_selected_option(Some(
            StyleSheet::new()
                .with_fg(ink(Hue::Brand))
                .with_attr(Attributes::BOLD),
        ))
        .with_scroll_up_prefix(Styled::new("↑").with_fg(ink(Hue::Dim)))
        .with_scroll_down_prefix(Styled::new("↓").with_fg(ink(Hue::Dim)))
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
