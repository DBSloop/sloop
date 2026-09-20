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

use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::mark::{self, Mark};
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
pub const INSET: usize = 2;

/// What a terminal that will not say gets.
const ASSUMED_COLUMNS: usize = 80;

/// This build, under the mark.
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

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
    /// A row of status chips, each with its own mark and colour.
    ///
    /// **The home screen's top line.** Three short facts with a tick or a triangle in front
    /// of each says more at a glance than three rows of label and value, and it is what the
    /// owner picked: the mark, and the facts beside it.
    Chips(Vec<(Mark, String)>),
    /// The headline of an outcome: what happened, to what, and the tag on the right.
    Verdict {
        /// Worked, did not, or worth knowing about.
        mark: Mark,
        /// What happened, in capitals — `BACKED UP`.
        what: String,
        /// What it happened to.
        subject: String,
        /// The right-hand corner: how long it took, or which code it failed with.
        tag: String,
    },
    /// One step of a result, on the rail down the left.
    Step {
        /// How that step went.
        mark: Mark,
        /// What it was.
        text: String,
        /// The detail beside it, in its own column.
        note: String,
    },
    /// A line a job printed, kept as it printed it.
    Said(String),
    /// A command, ready to paste.
    Command(String),
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

/// The line the highlight is on.
///
/// **Repainted rather than overlaid**, because the line already carries colours of its own
/// — a grey command beside a plain title — and a highlight that only changed the first of
/// them would leave half a row looking unselected. Every escape in it is stripped and the
/// whole line is set in the accent.
#[must_use]
pub fn chosen(text: &str) -> String {
    if !coloured() {
        return text.to_owned();
    }
    strong(&anstream::adapter::strip_str(text).to_string())
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
///
/// **The mark and the facts sit side by side, which is the owner's Home A.** Stacked, the
/// block took eight rows of art, a rule and three rows of facts — twenty of a twenty-four-row
/// terminal, leaving the menu a five-row keyhole to scroll thirty commands through. Beside
/// each other they take eight rows together, and the list below has room to be a list. A
/// terminal too narrow to hold both stacks them again rather than breaking either.
///
/// **And there is no hairline under it any more.** Rule 3 of the owner's list was *"need
/// spacing"*; a rule is a line drawn where a gap would have done, and the gap reads quieter.
#[must_use]
pub fn frame(header: &Header, columns: Option<usize>) -> String {
    let columns = columns.unwrap_or(ASSUMED_COLUMNS);
    let room = columns.saturating_sub(INSET * 2);
    // **Prose is capped and a value is not.** Sixty-six columns is where a sentence stops
    // being read and starts being scanned; a registry path is neither, and breaking one at
    // the same place leaves a directory name cut in half with the terminal half empty.
    let measure = MEASURE.min(room);
    let mut out: Vec<String> = Vec::new();

    out.push(String::new());
    match header.banner {
        Banner::Wordmark => out.extend(marked(header, columns, measure)),
        Banner::Word => {
            out.push(format!(
                "{}{}",
                pad(INSET),
                trail(&strong("sloop"), &header.crumbs)
            ));
            out.push(String::new());
            for line in &header.lines {
                out.extend(drawn(line, measure, room, INSET));
            }
        }
    }

    // One screen, one shape: whatever a header ended with, the list below it starts after
    // exactly one blank line.
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

/// How far the block on the right sits from the mark on the left.
const GUTTER: usize = 5;

/// The narrowest right-hand block worth having. Under this, the two stack instead.
const BESIDE: usize = 34;

/// What sits between two chips on one row.
pub(super) const GAP: usize = 3;

/// The shortest a middle column is allowed to get before it is dropped instead.
///
/// Under about this much a phrase stops being a phrase and starts being two words and an
/// ellipsis, which says less than the space it takes.
const SHORTEST: usize = 18;

/// The wordmark, with everything else to the right of it.
fn marked(header: &Header, columns: usize, measure: usize) -> Vec<String> {
    let left = INSET + wordmark::COLUMNS + GUTTER;
    let room = columns.saturating_sub(left + INSET);
    let beside = room >= BESIDE;
    let side_measure = if beside { room.min(MEASURE) } else { measure };
    let side_left = if beside { left } else { INSET };

    let mut side: Vec<String> = vec![
        String::new(),
        format!("{}{}  {}", pad(side_left), strong("sloop"), dim(VERSION)),
    ];
    if !header.strap.is_empty() {
        side.extend(
            wrap(&header.strap, side_measure)
                .into_iter()
                .map(|row| format!("{}{}", pad(side_left), hue(Hue::Text, &row))),
        );
    }
    side.push(String::new());
    for line in &header.lines {
        side.extend(drawn(line, side_measure, room, side_left));
    }

    let art = art(columns);
    if !beside {
        // Stacked: the art, then everything that would have sat beside it.
        let mut out: Vec<String> = art.iter().map(|row| art_row(row, false)).collect();
        out.extend(side);
        return tidied(out);
    }

    // Beside: one row of art against one row of the block, and whichever runs on carries on
    // alone. The art is eight rows and the block is rarely more, so this is usually one
    // rectangle.
    let filled = wordmark::COLUMNS + INSET - 1;
    let mut out = Vec::with_capacity(art.len().max(side.len()));
    for at in 0..art.len().max(side.len()) {
        match (art.get(at), side.get(at)) {
            (Some(art), Some(beside)) if beside.trim().is_empty() => {
                out.push(art_row(art, false));
            }
            (Some(art), Some(beside)) => out.push(format!(
                "{}{}{}",
                art_row(art, true),
                pad(left.saturating_sub(filled)),
                beside.trim_start()
            )),
            (Some(art), None) => out.push(art_row(art, false)),
            (None, Some(beside)) => out.push(beside.clone()),
            (None, None) => break,
        }
    }
    tidied(out)
}

/// Take the trailing spaces off every row.
///
/// **The block is a rectangle and the rows beside it are not.** `wordmark::plain` pads its
/// art to a fixed width so the two columns line up, which leaves a tail of spaces on any row
/// with nothing to its right — invisible on screen, and exactly the kind of thing that turns
/// up later as a diff nobody meant to make.
fn tidied(rows: Vec<String>) -> Vec<String> {
    rows.into_iter()
        .map(|row| row.trim_end().to_owned())
        .collect()
}

/// The block itself, in the accent, inset so its leftmost glyph lines up with the text.
///
/// Inset by one rather than two: every row of the block but one begins with a space of its
/// own, so one here puts its leftmost glyph in the same column as the text underneath. The
/// owner's art, unchanged — see "The wordmark".
fn art(columns: usize) -> Vec<String> {
    wordmark::plain(Some(columns.saturating_sub(INSET)))
        .lines()
        .map(|line| format!("{}{line}", pad(INSET - 1)))
        .collect()
}

/// One row of the block, painted — trimmed when it stands alone, and kept as the rectangle
/// it is when something sits beside it.
///
/// **The trimming has to happen before the colour, not after.** The block is padded so the
/// column beside it lines up, so a row with nothing to its right ends in spaces — and those
/// spaces are *inside* the escape, where trimming the finished line cannot reach them.
fn art_row(row: &str, beside: bool) -> String {
    if beside {
        accent(row)
    } else {
        accent(row.trim_end())
    }
}

/// One header line, as the rows it occupies, starting in column `left`.
fn drawn(line: &Line, measure: usize, room: usize, left: usize) -> Vec<String> {
    match line {
        Line::Lead(text) => wrap(text, measure)
            .into_iter()
            .map(|row| format!("{}{}", pad(left), hue(Hue::Text, &row)))
            .collect(),
        Line::Quiet(text) => wrap(text, measure)
            .into_iter()
            .map(|row| format!("{}{}", pad(left), dim(&row)))
            .collect(),
        Line::Fact {
            label,
            value,
            hue: ink,
        } => {
            // **Wrapped into its own column, not off the edge.** A registry path is as
            // long as somebody's home directory, and a value that ran past the right edge
            // came back around the left one in the middle of a word.
            let column = left + LABEL + 2;
            wrap_value(value, room.saturating_sub(LABEL + 2))
                .into_iter()
                .enumerate()
                .map(|(row, text)| {
                    let head = if row == 0 {
                        format!("{}{}  ", pad(left), dim(&format!("{label:<LABEL$}")))
                    } else {
                        pad(column)
                    };
                    format!("{head}{}", hue(*ink, &text))
                })
                .collect()
        }
        Line::Good(text) => vec![format!("{}{}", pad(left), hue(Hue::Ok, text))],
        Line::Wrong(text) => wrap(text, measure.saturating_sub(7))
            .into_iter()
            .enumerate()
            .map(|(row, text)| {
                if row == 0 {
                    format!("{}{}", pad(left), wrong(&text))
                } else {
                    format!("{}{}", pad(left + 7), hue(Hue::Bad, &text))
                }
            })
            .collect(),
        Line::Chips(chips) => chips_rows(chips, room, left),
        Line::Verdict {
            mark,
            what,
            subject,
            tag,
        } => vec![verdict(*mark, what, subject, tag, room, left)],
        // **Clipped, never wrapped.** A rail row that ran onto a second line would put the
        // continuation outside the rail, which reads as the rail having ended.
        Line::Step { mark, text, note } => {
            let spent = 3
                + crate::console::LABEL_AT
                + crate::console::NOTE_AT.max(text.chars().count() + 2);
            let over = room.saturating_sub(spent);
            let note = if note.chars().count() > over {
                let cut = wrap(note, over.saturating_sub(1))
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                format!("{cut}\u{2026}")
            } else {
                note.clone()
            };
            vec![format!(
                "{}{}  {}",
                pad(left),
                hue(mark.hue(), mark::rail()),
                crate::console::settled(*mark, text, &note)
            )]
        }
        Line::Said(text) => vec![format!("{}{text}", pad(left + 3))],
        Line::Command(text) => vec![format!("{}{}", pad(left), accent(text))],
        Line::Gap => vec![String::new()],
    }
}

/// A row of chips, wrapped onto a second row rather than off the edge.
fn chips_rows(chips: &[(Mark, String)], room: usize, left: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    let mut width = 0;

    for (mark, text) in chips {
        let cost = Mark::WIDTH + 1 + text.chars().count() + GAP;
        if width > 0 && width + cost > room {
            rows.push(format!("{}{row}", pad(left)));
            row = String::new();
            width = 0;
        }
        if width > 0 {
            row.push_str(&" ".repeat(GAP));
        }
        let _ = write!(
            row,
            "{} {}",
            hue(mark.hue(), mark.glyph()),
            hue(Hue::Text, text)
        );
        width += cost;
    }
    if !row.is_empty() {
        rows.push(format!("{}{row}", pad(left)));
    }
    rows
}

/// The headline of an outcome: the mark, what happened, what it happened to, and the tag.
fn verdict(mark: Mark, what: &str, subject: &str, tag: &str, room: usize, left: usize) -> String {
    let used = Mark::WIDTH + 2 + what.chars().count() + 3 + subject.chars().count();
    let space = room.saturating_sub(used + tag.chars().count()).max(2);
    format!(
        "{}{}  {}   {}{}{}",
        pad(left),
        hue(mark.hue(), mark.glyph()),
        hue(mark.hue(), what),
        accent(subject),
        pad(space),
        dim(tag)
    )
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

/// `sloop › Backups › Back one up now`, for a screen that draws its own header.
#[must_use]
pub fn trail_of(crumbs: &[&str]) -> String {
    trail(&strong("sloop"), crumbs)
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

/// How far a row sits in from the arrow beside it.
///
/// **The structure the owner drew** — the arrow in its own two columns, and everything that
/// can be chosen stepped in from it.
pub const UNDER: usize = 2;

/// Everything a list row spends before the title starts: the two columns the arrow lives in,
/// the step in under it, and two kept clear at the right edge so a full row never touches it.
const ROW_OVERHEAD: usize = 4 + UNDER;

/// How wide each column of a list is, worked out once for the whole list.
///
/// **Once, so the columns line up.** A row that sized itself would be a list where every
/// row's middle column started somewhere different, which is the thing that made the old
/// screen look cluttered even when every line on it was correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Widths {
    /// The longest title.
    pub title: usize,
    /// The longest middle column.
    pub note: usize,
    /// The longest command.
    ///
    /// **Measured across the list, not per row.** A row that sized its own middle column
    /// from its own command would start the command wherever that row happened to leave off,
    /// and a list of five would have five different right-hand columns — which is most of
    /// what "cluttered" looks like.
    pub command: usize,
}

/// The widths a list needs, given every row in it.
#[must_use]
pub fn widths<'a>(rows: impl Iterator<Item = (&'a str, &'a str, &'a str)>) -> Widths {
    let mut widths = Widths::default();
    for (title, note, command) in rows {
        widths.title = widths.title.max(title.chars().count());
        widths.note = widths.note.max(note.chars().count());
        widths.command = widths.command.max(command.chars().count());
    }
    widths
}

/// One row of a list: what it is called, what it is doing, and the command that does it.
///
/// **Three columns, and the middle one carries the colour.** Rule 4 of the owner's list —
/// *"need more rich colors"* — is answered here more than anywhere else: the title is
/// ordinary text, the command is grey reference, and the fact between them is green when a
/// thing is working, amber when it wants attention and grey when it is neither.
///
/// **The command is never the first thing dropped.** Rule 11: *"showing command in right side
/// seems good, don't remove"*. A terminal too narrow for all three loses the middle column,
/// and one too narrow for two loses everything but the title.
///
/// `inquire`'s successor draws this with an ANSI-aware counter, so the escapes in the
/// coloured halves cost nothing and the list still knows how wide it is.
#[must_use]
pub fn option(
    title: &str,
    note: &str,
    ink: Hue,
    command: &str,
    widths: Widths,
    columns: Option<usize>,
) -> String {
    let step = pad(UNDER);
    let room = columns
        .unwrap_or(ASSUMED_COLUMNS)
        .saturating_sub(ROW_OVERHEAD);

    let title_column = widths.title.max(title.chars().count());
    let note_column = widths.note.max(note.chars().count());
    let wants = title_column + GAP + note_column + GAP + command.chars().count();

    // Everything fits: three columns, each starting where every other row's does.
    if wants <= room && !note.is_empty() && !command.is_empty() {
        return format!(
            "{step}{title:<title_column$}{}{}{}{}",
            pad(GAP),
            hue(ink, &format!("{note:<note_column$}")),
            pad(GAP),
            dim(command)
        );
    }

    // **Shortened before it is dropped.** A door's row says what is behind it and a command's
    // row says what it does; losing that entirely on an eighty-column terminal is a worse
    // trade than losing the end of the sentence.
    let spare = room
        .saturating_sub(title_column + GAP + GAP + widths.command.max(command.chars().count()))
        .min(note_column);
    if !note.is_empty() && !command.is_empty() && spare >= SHORTEST {
        // At a word, not mid-syllable: `dumps it, checks every row arriv` reads as a bug.
        let shortened = wrap(note, spare).into_iter().next().unwrap_or_default();
        return format!(
            "{step}{title:<title_column$}{}{}{}{}",
            pad(GAP),
            hue(ink, &format!("{shortened:<spare$}")),
            pad(GAP),
            dim(command)
        );
    }

    // Two columns. The command keeps its place; the note gives way.
    let beside = if command.is_empty() { note } else { command };
    let beside_ink = if command.is_empty() { ink } else { Hue::Dim };
    if beside.is_empty() {
        return format!("{step}{title}");
    }
    if title_column + GAP + beside.chars().count() > room {
        // Under about twenty-four columns of room the phrase stops being a phrase and starts
        // being a word per line, so the title carries the row on its own.
        if room < title_column + GAP + 24 {
            return format!("{step}{title}");
        }
        let cut = room.saturating_sub(title_column + GAP);
        let beside = wrap(beside, cut).into_iter().next().unwrap_or_default();
        return format!(
            "{step}{title:<title_column$}{}{}",
            pad(GAP),
            hue(beside_ink, &beside)
        );
    }
    format!(
        "{step}{title:<title_column$}{}{}",
        pad(GAP),
        hue(beside_ink, beside)
    )
}

#[cfg(test)]
#[path = "paint_tests.rs"]
mod tests;
