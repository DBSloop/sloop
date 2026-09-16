//! The `sloop` wordmark.
//!
//! The owner supplied this as a figlet block and fixed it as the header of the init
//! screen and the home screen; it is recorded verbatim in `docs/OWNER-DECISIONS.md`.
//!
//! The lines are stored without their trailing spaces and padded to [`COLUMNS`] when they
//! are rendered. That produces the same rectangle the owner drew while keeping the source
//! free of trailing whitespace, which editors, formatters and diff tools all eat sooner
//! or later — the rectangle is now guaranteed rather than hoped for.

use crate::style;

/// The width of the block. A terminal narrower than this cannot hold it.
pub const COLUMNS: usize = 26;

const ART: [&str; 8] = [
    r"      _",
    r"     | |",
    r"  ___| | ___   ___  _ __",
    r" / __| |/ _ \ / _ \| '_ \",
    r" \__ \ | (_) | (_) | |_) |",
    r" |___/_|\___/ \___/| .__/",
    r"                   | |",
    r"                   |_|",
];

/// The block with nothing but spaces and glyphs, or the plain word when the terminal is
/// too narrow to hold it without breaking it.
///
/// `columns` is the terminal width, or `None` when there is no terminal to measure — a
/// pipe or a file, where the full block is right because nothing will wrap it.
///
/// **Uncoloured, and that is the point of it.** [`crate::report`] writes through
/// `anstream`, which strips escapes when the destination will not take them; the
/// interactive shell writes through `crossterm`, which does not. So the shell takes the
/// art from here and paints it itself, and there is still only one copy of it.
#[must_use]
pub fn plain(columns: Option<usize>) -> String {
    if columns.is_some_and(|columns| columns < COLUMNS) {
        return "sloop".to_owned();
    }

    ART.iter()
        .map(|line| format!("{line:<COLUMNS$}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The wordmark in the accent, for a line on its way out through [`crate::report`].
#[must_use]
pub fn render(columns: Option<usize>) -> String {
    plain(columns)
        .lines()
        .map(style::paint)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{ART, COLUMNS, plain, render};

    /// The block as the owner drew it, with no width to have to fit inside.
    fn block() -> String {
        plain(None)
    }

    #[test]
    fn no_line_is_wider_than_the_block() {
        let widest = ART
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        assert_eq!(widest, COLUMNS, "the block is {COLUMNS} columns wide");
    }

    #[test]
    fn the_rendered_block_is_a_rectangle() {
        for line in block().lines() {
            assert_eq!(
                line.chars().count(),
                COLUMNS,
                "{line:?} is not {COLUMNS} wide"
            );
        }
    }

    #[test]
    fn the_source_carries_no_trailing_whitespace() {
        for line in ART {
            assert_eq!(line.trim_end(), line, "{line:?} has trailing whitespace");
        }
    }

    #[test]
    fn a_narrow_terminal_gets_the_word_instead_of_a_broken_block() {
        assert!(!render(Some(COLUMNS - 1)).contains('\n'));
        assert_eq!(render(Some(COLUMNS)).lines().count(), ART.len());
        assert_eq!(render(None).lines().count(), ART.len());
    }

    #[test]
    fn it_still_spells_sloop() {
        // Read the block back the way a person does: the glyphs, ignoring the gaps.
        let drawn: String = block().chars().filter(|c| !c.is_whitespace()).collect();
        assert!(drawn.contains("___|"), "the block lost its shape: {drawn}");
        assert_eq!(block().lines().count(), 8);
    }
}
