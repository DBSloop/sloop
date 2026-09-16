//! What a screen looks like above the list.
//!
//! **Colour is on, and that is decided here rather than sniffed.** Everything `report`
//! prints goes out through `anstream`, which asks whether the destination looks like it
//! would take escapes and strips them if it is not sure. That is the right question for a
//! line that might be going into a pipe and the wrong one here: the shell refuses to open
//! at all unless standard error is a terminal, and it has already told Windows to parse
//! escape sequences by the time a byte is written. So the only reasons left not to colour
//! are the ones the *user* gave — `--no-color`, `NO_COLOR`, `CLICOLOR=0` — and those are
//! the only three [`coloured`] looks at.
//!
//! **The palette is [`crate::style`]'s, which is the website's.** Orange is the brand and
//! the thing you are about to do; green worked; amber is worth knowing; red did not work;
//! grey is a label rather than the value beside it. Nothing is coloured for the sake of
//! being coloured, and nothing that carries meaning is left grey.
//!
//! **The layout is a function of the terminal's width and nothing else**, which is what
//! lets it be tested without a terminal. Every screen is built as a `String` here and only
//! then handed to the screen.

use std::sync::OnceLock;

use crate::style::{self, Hue};
use crate::wordmark;

/// The widest a paragraph is allowed to get.
///
/// Prose past about seventy columns stops being read and starts being scanned; the rows of
/// facts and the menu below are free to be wider, because neither is prose.
pub const MEASURE: usize = 66;

/// How far everything is inset from the left edge.
///
/// Two, because `inquire` draws a two-column prefix in front of its own prompt and every
/// option — so a header inset by two lands in the same column as the list under it, and the
/// whole screen has one left edge instead of two.
const INSET: usize = 2;

/// What a terminal that will not say gets.
const ASSUMED_COLUMNS: usize = 80;

/// This build, under the mark.
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

/// What sits between the version and the line beside it.
const SEPARATOR: &str = "  ·  ";

/// The column a fact's value starts in. The longest label in the tree is ten.
const LABEL: usize = 10;

/// The mark at the top of a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Banner {
    /// The full block, with the version under it. The first screen of a session, and the
    /// menu it comes home to.
    Wordmark,
    /// The word, on one line with the breadcrumb beside it. Every screen below the top.
    Word,
}

/// One line of a header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// The sentence that names the screen. Wrapped to [`MEASURE`].
    Lead(String),
    /// Support underneath it. Wrapped, and grey.
    Quiet(String),
    /// A label and the value beside it.
    Fact {
        /// The grey half.
        label: String,
        /// The half worth reading.
        value: String,
        /// Which colour the value carries.
        hue: Hue,
    },
    /// It worked.
    Good(String),
    /// It did not.
    Wrong(String),
    /// Deliberate space.
    Gap,
}

impl Line {
    /// A fact whose value is ordinary.
    #[must_use]
    pub fn fact(label: &str, value: &str) -> Self {
        Self::Fact {
            label: label.to_owned(),
            value: value.to_owned(),
            hue: Hue::Text,
        }
    }

    /// A fact whose value is worth a colour of its own.
    #[must_use]
    pub fn told(label: &str, value: &str, hue: Hue) -> Self {
        Self::Fact {
            label: label.to_owned(),
            value: value.to_owned(),
            hue,
        }
    }

    /// A grey note in the value's own column, under the fact above it.
    #[must_use]
    pub fn under(note: &str) -> Self {
        Self::Fact {
            label: String::new(),
            value: note.to_owned(),
            hue: Hue::Dim,
        }
    }
}

/// Everything above the list or the box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Which mark.
    pub banner: Banner,
    /// Where in the tree this is, outermost first.
    pub crumbs: Vec<&'static str>,
    /// The one line under the mark. Empty for none.
    pub strap: String,
    /// The body.
    pub lines: Vec<Line>,
}

/// Is this session allowed to write colour at all?
///
/// Three answers the user gave, in the order they override each other, and nothing else.
/// `OnceLock` because none of them can change mid-session and because the menu asks this
/// for every line of every redraw.
#[must_use]
pub fn coloured() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        wanted(
            anstream::ColorChoice::global(),
            std::env::var("NO_COLOR").ok().as_deref(),
            std::env::var("CLICOLOR").ok().as_deref(),
        )
    })
}

/// The decision itself, with the three answers passed in rather than read.
///
/// Apart from [`coloured`] because that one may only be asked once per process, and a rule
/// with three branches deserves more than one of them tested.
fn wanted(global: anstream::ColorChoice, no_color: Option<&str>, clicolor: Option<&str>) -> bool {
    // `--no-color`, which `report::settle` writes here before a command runs.
    if global == anstream::ColorChoice::Never {
        return false;
    }
    // The no-color.org convention: set to anything non-empty means no colour.
    if no_color.is_some_and(|value| !value.is_empty()) {
        return false;
    }
    // And the older one it grew out of.
    clicolor != Some("0")
}

/// Text in one of the six, or the plain text when this session is not colouring.
#[must_use]
pub fn hue(hue: Hue, text: &str) -> String {
    if coloured() {
        style::in_hue(hue, text)
    } else {
        text.to_owned()
    }
}

/// The accent.
#[must_use]
pub fn accent(text: &str) -> String {
    hue(Hue::Brand, text)
}

/// The accent with weight behind it.
#[must_use]
pub fn strong(text: &str) -> String {
    if coloured() {
        style::heading(text)
    } else {
        text.to_owned()
    }
}

/// Quieter than the text around it.
#[must_use]
pub fn dim(text: &str) -> String {
    hue(Hue::Dim, text)
}

/// Something that did not work, with the prefix every other failure in this tool carries.
#[must_use]
pub fn wrong(text: &str) -> String {
    if coloured() {
        format!("{} {text}", style::error_prefix())
    } else {
        format!("error: {text}")
    }
}

/// Draw a header for a terminal `columns` wide.
///
/// Returns the block with no trailing newline, so the caller decides how much air sits
/// between it and the list.
#[must_use]
pub fn frame(header: &Header, columns: Option<usize>) -> String {
    let columns = columns.unwrap_or(ASSUMED_COLUMNS);
    let measure = MEASURE.min(columns.saturating_sub(INSET * 2));
    let mut out: Vec<String> = Vec::new();

    out.push(String::new());
    match header.banner {
        Banner::Wordmark => {
            // Inset by one rather than two: every row of the block but one begins with a
            // space of its own, so one here puts its leftmost glyph in the same column as
            // the text underneath. The owner's art, unchanged — see "The wordmark".
            for line in wordmark::plain(Some(columns.saturating_sub(INSET))).lines() {
                out.push(format!("{}{}", pad(INSET - 1), accent(line)));
            }
            out.push(String::new());
            out.extend(strapline(&header.strap, measure));
        }
        Banner::Word => {
            out.push(format!(
                "{}{}",
                pad(INSET),
                trail(&strong("sloop"), &header.crumbs)
            ));
        }
    }

    out.push(String::new());
    out.push(format!("{}{}", pad(INSET), dim(&rule(measure))));
    out.push(String::new());

    for line in &header.lines {
        out.extend(drawn(line, measure));
    }

    // One screen, one shape: whatever a header ended with, the list below it starts after
    // exactly one blank line.
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

/// The version, and the one line beside it.
///
/// **The version sits under the mark and nowhere else.** `--version` answers a script;
/// this answers the person looking at the screen, who wants to know which build is in
/// front of them without leaving it. The owner asked for it there — see
/// "Colour, and the version under the wordmark" in `docs/OWNER-DECISIONS.md`.
fn strapline(strap: &str, measure: usize) -> Vec<String> {
    let version = format!("{}{}", pad(INSET), accent(VERSION));
    if strap.is_empty() {
        return vec![version];
    }

    // **Beside it when it fits, underneath it when it does not.** Never cut: a strapline
    // that quietly loses its last three words on a narrow terminal is worse than one that
    // takes a second line, and it is the kind of thing nobody notices for a year.
    let together = VERSION.chars().count() + SEPARATOR.chars().count() + strap.chars().count();
    if together <= measure {
        return vec![format!(
            "{version}{}{}",
            dim(SEPARATOR),
            hue(Hue::Text, strap)
        )];
    }

    let mut rows = vec![version];
    rows.extend(
        wrap(strap, measure)
            .into_iter()
            .map(|row| format!("{}{}", pad(INSET), hue(Hue::Text, &row))),
    );
    rows
}

/// One header line, as the rows it occupies.
fn drawn(line: &Line, measure: usize) -> Vec<String> {
    match line {
        Line::Lead(text) => wrap(text, measure)
            .into_iter()
            .map(|row| format!("{}{}", pad(INSET), hue(Hue::Text, &row)))
            .collect(),
        Line::Quiet(text) => wrap(text, measure)
            .into_iter()
            .map(|row| format!("{}{}", pad(INSET), dim(&row)))
            .collect(),
        Line::Fact {
            label,
            value,
            hue: ink,
        } => {
            // **Wrapped into its own column, not off the edge.** A registry path is as
            // long as somebody's home directory, and a value that ran past the right edge
            // came back around the left one in the middle of a word.
            let column = INSET + LABEL + 2;
            wrap_value(value, measure.saturating_sub(LABEL + 2))
                .into_iter()
                .enumerate()
                .map(|(row, text)| {
                    let head = if row == 0 {
                        format!("{}{}  ", pad(INSET), dim(&format!("{label:<LABEL$}")))
                    } else {
                        pad(column)
                    };
                    format!("{head}{}", hue(*ink, &text))
                })
                .collect()
        }
        Line::Good(text) => vec![format!("{}{}", pad(INSET), hue(Hue::Ok, text))],
        Line::Wrong(text) => wrap(text, measure.saturating_sub(7))
            .into_iter()
            .enumerate()
            .map(|(row, text)| {
                if row == 0 {
                    format!("{}{}", pad(INSET), wrong(&text))
                } else {
                    format!("{}{}", pad(INSET + 7), hue(Hue::Bad, &text))
                }
            })
            .collect(),
        Line::Gap => vec![String::new()],
    }
}

/// `sloop › Databases`, the accent on the mark and the rest ordinary.
fn trail(mark: &str, crumbs: &[&str]) -> String {
    let mut out = mark.to_owned();
    for crumb in crumbs {
        out.push_str(&dim("  ›  "));
        out.push_str(&hue(Hue::Text, crumb));
    }
    out
}

/// A hairline, `width` columns of it.
fn rule(width: usize) -> String {
    "─".repeat(width)
}

fn pad(width: usize) -> String {
    " ".repeat(width)
}

/// Break `text` into rows no wider than `measure`.
///
/// By words, and a word longer than the measure is left to overhang rather than cut: the
/// only things that long in this tool are paths and commands, and half a path is worse
/// than a ragged edge. Counts characters rather than bytes — nothing here contains an
/// escape, because [`frame`] wraps first and paints afterwards.
fn wrap(text: &str, measure: usize) -> Vec<String> {
    let measure = measure.max(1);
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();

    for word in text.split_whitespace() {
        let would = if row.is_empty() {
            word.chars().count()
        } else {
            row.chars().count() + 1 + word.chars().count()
        };
        if !row.is_empty() && would > measure {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push(' ');
        }
        row.push_str(word);
    }

    if !row.is_empty() {
        rows.push(row);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

/// Break `text` into rows no wider than `measure`, cutting a word longer than the whole
/// measure instead of letting it overhang.
///
/// **For a value, where [`wrap`] is for prose.** A path is one word and it is routinely
/// longer than any measure; left whole it runs off the right edge, and the terminal brings
/// it back around the left one mid-word and outside its column, which looks like the screen
/// has broken. A clean break at the edge is the lesser loss. A command is the one value
/// this must never be done to, and it does not come through here.
fn wrap_value(text: &str, measure: usize) -> Vec<String> {
    let measure = measure.max(1);
    wrap(text, measure)
        .into_iter()
        .flat_map(|row| {
            let letters: Vec<char> = row.chars().collect();
            letters
                .chunks(measure)
                .map(|chunk| chunk.iter().collect::<String>())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Everything a list row spends before the phrase starts: the two columns `inquire` draws
/// its own prefix in, the two between the title and the phrase, and two more kept clear at
/// the right edge so a full row never touches it.
const ROW_OVERHEAD: usize = 6;

/// A menu item, padded so the phrases line up in a column of their own.
///
/// `inquire` measures what it renders with an ANSI-aware counter, so the escapes in the
/// grey half cost nothing and the list still knows how wide it is.
#[must_use]
pub fn option(title: &str, blurb: &str, column: usize, columns: Option<usize>) -> String {
    if blurb.is_empty() {
        return title.to_owned();
    }

    let room = columns
        .unwrap_or(ASSUMED_COLUMNS)
        .saturating_sub(column + ROW_OVERHEAD);

    // Under about twenty-four columns of room the phrase stops being a phrase and starts
    // being a word per line, so the title carries the item on its own.
    if room < 24 {
        return title.to_owned();
    }

    let blurb = wrap(blurb, room).into_iter().next().unwrap_or_default();
    format!("{title:<column$}  {}", dim(&blurb))
}

/// The column the phrases start in: the longest title, so nothing is ragged.
#[must_use]
pub fn column_for<'a>(titles: impl Iterator<Item = &'a str>) -> usize {
    titles.map(|title| title.chars().count()).max().unwrap_or(0)
}

#[cfg(test)]
#[path = "paint_tests.rs"]
mod tests;
