//! The marks a line can carry, and what each one becomes where its glyph is not there.
//!
//! **A status is a glyph and a colour together, and neither one alone.** A tick that is not
//! green and a green line with no tick are both half a signal: the first is read by somebody
//! scanning for shape, the second by somebody scanning for colour, and a terminal has both
//! kinds of reader — sometimes the same person, at different distances from the screen. So
//! every status in this program comes from [`Mark`], which answers both questions at once,
//! and no call site writes `"✓ "` by hand.
//!
//! **Every glyph has an ASCII twin, because some consoles have no font for the good one.**
//! A Windows console still running a raster font draws `●` as a hollow box, and a box is
//! worse than an honest `o`. The choice is made once, in [`unicode`], and the rule it uses is
//! written down in [`wanted`] where it can be tested rather than guessed at.
//!
//! The colours are `crate::style`'s six and nothing else: green worked, amber is worth
//! knowing, red did not work, orange is the thing happening now, grey is waiting its turn.

use std::sync::OnceLock;

use crate::style::Hue;

/// One status, as the glyph and the colour that say it together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// It worked.
    Ok,
    /// It did not work.
    Bad,
    /// It worked, and there is something to know about it.
    Warn,
    /// It is happening now. Drawn as the spinner's current frame rather than a fixed glyph.
    Doing,
    /// It has not started yet.
    Todo,
    /// The thing the highlight is on.
    Here,
    /// A line that is none of the above: output, a fact, a note.
    Plain,
}

impl Mark {
    /// The glyph, in whichever alphabet this terminal can draw.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        let fancy = unicode();
        match self {
            Self::Ok => {
                if fancy {
                    "\u{2713}"
                } else {
                    "+"
                }
            }
            Self::Bad => {
                if fancy {
                    "\u{2717}"
                } else {
                    "x"
                }
            }
            Self::Warn => {
                if fancy {
                    "\u{25b2}"
                } else {
                    "!"
                }
            }
            // Never drawn: a step that is running shows the spinner's frame instead, and
            // this is what it falls back to the moment before the first tick.
            Self::Doing => {
                if fancy {
                    "\u{25cf}"
                } else {
                    "o"
                }
            }
            Self::Todo => {
                if fancy {
                    "\u{00b7}"
                } else {
                    "."
                }
            }
            Self::Here => {
                if fancy {
                    "\u{203a}"
                } else {
                    ">"
                }
            }
            Self::Plain => " ",
        }
    }

    /// The colour that goes with it.
    #[must_use]
    pub const fn hue(self) -> Hue {
        match self {
            Self::Ok => Hue::Ok,
            Self::Bad => Hue::Bad,
            Self::Warn => Hue::Warn,
            Self::Doing | Self::Here => Hue::Brand,
            Self::Todo => Hue::Dim,
            Self::Plain => Hue::Text,
        }
    }

    /// How wide the glyph is drawn, so a column of marks lines up whatever they are.
    ///
    /// One, for every one of them. That is not an accident of the characters chosen — it is
    /// the reason those characters were chosen, and [`every_glyph_is_one_column`] holds it
    /// to it.
    pub const WIDTH: usize = 1;
}

/// The rail drawn down the left of a result, marking which lines belong to it.
///
/// **Heavy rather than light**, because the hairline under a header is already `─` and a
/// result that used the same weight would read as another divider rather than as a margin.
#[must_use]
pub fn rail() -> &'static str {
    if unicode() { "\u{2503}" } else { "|" }
}

/// The frames of the spinner, in order, and how long each one is on screen.
///
/// **A dot travelling along three positions** — the owner's pick. It reads as motion at a
/// glance without redrawing a different shape every frame, which is what makes a spinner
/// look like static rather than like work.
///
/// Four frames rather than three: the dot goes out and comes back, so the eye follows one
/// object moving rather than three lights blinking in sequence.
#[must_use]
pub fn frames() -> &'static [&'static str] {
    if unicode() {
        &[
            "\u{25cf}\u{2219}\u{2219}",
            "\u{2219}\u{25cf}\u{2219}",
            "\u{2219}\u{2219}\u{25cf}",
            "\u{2219}\u{25cf}\u{2219}",
        ]
    } else {
        &["o..", ".o.", "..o", ".o."]
    }
}

/// How wide one frame is. Every frame is this wide, whichever alphabet is in use.
pub const FRAME_WIDTH: usize = 3;

/// How long one frame stays on screen.
///
/// **120 ms, which is slow for a spinner on purpose.** The thing being waited on here is a
/// dump or a download that takes minutes, and a fast spinner beside a slow job reads as
/// panic. It is also cheap: eight repaints a second of a screen that is one string.
pub const FRAME_TIME: std::time::Duration = std::time::Duration::from_millis(120);

/// A progress bar `width` columns wide, filled to `fraction` of the way across.
///
/// **The ends are capped and the fill is an eighth of a character precise.** A bar drawn
/// only in whole blocks jumps in steps of a percent and a half on a forty-column bar, which
/// on a slow download looks like a bar that has stopped. The eighth-blocks make it move
/// continuously, which is the whole job of the thing.
///
/// Returns the bar between its caps, ready to be coloured by the caller: the filled part
/// first, then the empty part, so the two can carry different hues.
///
/// **Every cast here is bounded by the line above it.** `fraction` is clamped to `0.0..=1.0`
/// and `width` is a column count on a terminal, so the product is a small non-negative number
/// and the rounding cannot overflow, lose a sign or lose precision that matters. The `allow`s
/// say so rather than `as`-casting quietly.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn bar(fraction: f64, width: usize) -> (String, String) {
    let width = width.max(1);
    let fraction = fraction.clamp(0.0, 1.0);

    if !unicode() {
        let full = ((fraction * width as f64).round() as usize).min(width);
        return ("#".repeat(full), ".".repeat(width - full));
    }

    // Eighths of a column across the whole bar, so the remainder picks a partial block.
    let eighths = (fraction * (width * 8) as f64).round() as usize;
    let full = (eighths / 8).min(width);
    let part = u32::try_from(eighths % 8).unwrap_or(0);

    let mut filled = "\u{2588}".repeat(full);
    let mut empty = width - full;
    if part > 0 && full < width {
        // U+2588 is the full block and the seven before it are the eighths of the way
        // there, so the partial block is the full one minus however many eighths are left.
        filled.push(char::from_u32(0x2588 - (8 - part)).unwrap_or('\u{2588}'));
        empty -= 1;
    }

    (filled, " ".repeat(empty))
}

/// The left cap of a bar.
#[must_use]
pub fn bar_start() -> &'static str {
    if unicode() { "\u{2595}" } else { "[" }
}

/// The right cap of a bar.
#[must_use]
pub fn bar_end() -> &'static str {
    if unicode() { "\u{258f}" } else { "]" }
}

/// Can this terminal draw the good glyphs?
///
/// Asked once per process: nothing it looks at can change while sloop is running, and the
/// answer is wanted on every line of every repaint.
#[must_use]
pub fn unicode() -> bool {
    static FANCY: OnceLock<bool> = OnceLock::new();
    *FANCY.get_or_init(|| {
        let read = |name: &str| std::env::var(name).ok();
        wanted(
            read("SLOOP_ASCII").as_deref(),
            read("SLOOP_UNICODE").as_deref(),
            cfg!(windows),
            [
                "WT_SESSION",
                "WT_PROFILE_ID",
                "TERM_PROGRAM",
                "ConEmuANSI",
                "TERM",
            ]
            .iter()
            .any(|name| read(name).is_some()),
        )
    })
}

/// The rule itself, with what it looks at passed in rather than read.
///
/// **The user's word first, both ways.** `SLOOP_ASCII` forces the plain alphabet and
/// `SLOOP_UNICODE` forces the good one, because whatever this guesses, the person sitting in
/// front of the terminal knows better and needs a way to say so.
///
/// **Then the one guess, and only on Windows.** Every other platform has had a UTF-8 locale
/// and a font with box-drawing in it for twenty years. Windows has two consoles: the modern
/// one, which sets a variable naming itself, and `conhost` with a raster font, which sets
/// nothing and draws a hollow box for anything past Latin-1. Silence there means the old one,
/// and the old one gets `+`, `x` and `!` — which are ugly and legible, in that order of
/// importance.
#[must_use]
pub fn wanted(ascii: Option<&str>, forced: Option<&str>, windows: bool, modern: bool) -> bool {
    if forced.is_some_and(|value| !value.is_empty()) {
        return true;
    }
    if ascii.is_some_and(|value| !value.is_empty()) {
        return false;
    }
    !windows || modern
}

#[cfg(test)]
mod tests {
    use super::{FRAME_WIDTH, Mark, bar, frames, wanted};

    /// Every mark, for the tests that have to sweep all of them.
    const ALL: [Mark; 7] = [
        Mark::Ok,
        Mark::Bad,
        Mark::Warn,
        Mark::Doing,
        Mark::Todo,
        Mark::Here,
        Mark::Plain,
    ];

    #[test]
    fn every_glyph_is_one_column() {
        for mark in ALL {
            assert_eq!(
                mark.glyph().chars().count(),
                Mark::WIDTH,
                "{mark:?} is not {} column wide",
                Mark::WIDTH
            );
        }
    }

    #[test]
    fn every_frame_is_the_same_width() {
        for frame in frames() {
            assert_eq!(
                frame.chars().count(),
                FRAME_WIDTH,
                "{frame:?} is not {FRAME_WIDTH} columns"
            );
        }
    }

    /// A spinner whose first and last frames are the same stutters once per cycle.
    #[test]
    fn the_cycle_does_not_repeat_a_frame_at_the_join() {
        let all = frames();
        assert_ne!(all[0], all[all.len() - 1]);
    }

    #[test]
    fn the_user_has_the_last_word_on_the_alphabet() {
        // Forced on, even on a console that would otherwise be refused it.
        assert!(wanted(Some("1"), Some("1"), true, false));
        // Forced off, even where it would have been offered.
        assert!(!wanted(Some("1"), None, false, true));
        // An empty value is not an answer.
        assert!(wanted(Some(""), None, false, true));
    }

    #[test]
    fn only_a_windows_console_that_named_itself_gets_the_good_glyphs() {
        assert!(wanted(None, None, true, true), "Windows Terminal");
        assert!(!wanted(None, None, true, false), "legacy conhost");
        assert!(wanted(None, None, false, false), "everything else");
    }

    #[test]
    fn a_bar_is_always_exactly_as_wide_as_it_was_asked_for() {
        for width in [1_usize, 7, 20, 41] {
            for step in 0..=20 {
                let (filled, empty) = bar(f64::from(step) / 20.0, width);
                assert_eq!(
                    filled.chars().count() + empty.chars().count(),
                    width,
                    "{step}/20 of a {width}-wide bar"
                );
            }
        }
    }

    #[test]
    fn a_bar_is_empty_at_nothing_and_full_at_everything() {
        let (filled, empty) = bar(0.0, 10);
        assert_eq!(filled.chars().count(), 0);
        assert_eq!(empty.chars().count(), 10);

        let (filled, empty) = bar(1.0, 10);
        assert_eq!(filled.chars().count(), 10);
        assert_eq!(empty.chars().count(), 0);
    }

    /// A fraction outside the range is a bug upstream, and a bar that panicked over it
    /// would take a working backup down with it.
    #[test]
    fn a_fraction_outside_the_range_is_clamped_rather_than_fatal() {
        assert_eq!(bar(-1.0, 8).0.chars().count(), 0);
        assert_eq!(bar(9.0, 8).0.chars().count(), 8);
    }
}
