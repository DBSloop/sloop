//! What a screen looks like, checked without a terminal to look at.

use super::{Banner, Header, Line, MEASURE, VERSION, column_for, frame, option, wrap};
use crate::style::Hue;

/// Strip every SGR sequence, the way a reader's eye does.
fn plain(painted: &str) -> String {
    let mut out = String::new();
    let mut rest = painted;
    while let Some(start) = rest.find('\x1b') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find('m') else { break };
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn home() -> Header {
    Header {
        banner: Banner::Wordmark,
        crumbs: Vec::new(),
        strap: "your databases, backed up and moved about".to_owned(),
        lines: vec![
            Line::fact("working in", "the global store"),
            Line::told("registered", "2 databases", Hue::Ok),
        ],
    }
}

fn deeper() -> Header {
    Header {
        banner: Banner::Word,
        crumbs: vec!["Databases"],
        strap: String::new(),
        lines: vec![Line::Quiet(
            "A database sloop knows about can be backed up, copied and checked. Everything \
             here is about that list."
                .to_owned(),
        )],
    }
}

#[test]
fn the_home_screen_carries_the_wordmark() {
    let drawn = plain(&frame(&home(), Some(100)));
    assert!(
        drawn.contains(r"\__ \ | (_) | (_) | |_) |"),
        "the wordmark is missing from the home screen:\n{drawn}"
    );
    assert_eq!(
        drawn.lines().filter(|line| line.contains("___")).count(),
        2,
        "the block lost rows"
    );
}

/// Below 26 columns the block cannot be drawn without breaking, so it is the plain word.
/// The owner's decision, recorded under "The wordmark".
#[test]
fn a_narrow_terminal_gets_the_word() {
    let drawn = plain(&frame(&home(), Some(20)));
    assert!(drawn.contains("sloop"), "{drawn}");
    assert!(
        !drawn.contains("|_|"),
        "the block was drawn into a terminal too narrow to hold it:\n{drawn}"
    );
}

/// The wordmark is the home screen's alone. Everywhere else carries the word and a
/// breadcrumb, which is the rhythm: one screen is the front door, the rest are rooms.
#[test]
fn only_the_top_carries_the_block() {
    let drawn = plain(&frame(&deeper(), Some(100)));
    assert!(!drawn.contains("|_|"), "{drawn}");
    assert!(drawn.contains("sloop"), "{drawn}");
    assert!(drawn.contains("Databases"), "{drawn}");
}

#[test]
fn prose_is_capped_at_a_measure_however_wide_the_terminal_is() {
    for columns in [80, 120, 200, 400] {
        for line in plain(&frame(&deeper(), Some(columns))).lines() {
            assert!(
                line.chars().count() <= MEASURE + 4,
                "a {columns}-column terminal produced a {}-character line: {line:?}",
                line.chars().count()
            );
        }
    }
}

#[test]
fn nothing_overflows_a_narrow_terminal() {
    for columns in [26, 30, 40, 60] {
        for line in plain(&frame(&deeper(), Some(columns))).lines() {
            assert!(
                line.chars().count() <= columns,
                "a {columns}-column terminal produced a {}-character line: {line:?}",
                line.chars().count()
            );
        }
    }
}

#[test]
fn a_header_never_ends_in_blank_rows() {
    let mut header = home();
    header.lines.push(Line::Gap);
    header.lines.push(Line::Gap);
    let drawn = frame(&header, Some(100));
    assert!(
        !drawn.ends_with('\n') && !drawn.ends_with(' '),
        "the header trails whitespace: {:?}",
        drawn.chars().rev().take(8).collect::<String>()
    );
}

#[test]
fn a_menu_item_lines_its_phrase_up_in_a_column() {
    let column = column_for(["Rename one", "Check one answers"].into_iter());
    assert_eq!(column, "Check one answers".chars().count());

    for title in ["Rename one", "Check one answers"] {
        let drawn = plain(&option(
            title,
            "the name sloop files it under",
            column,
            Some(100),
        ));
        assert!(
            drawn.starts_with(&format!("{}{title}", " ".repeat(super::UNDER))),
            "an item is not stepped in under its heading: {drawn:?}"
        );
        assert_eq!(
            drawn.find("the name"),
            Some(super::UNDER + column + 2),
            "the phrase did not land in its column: {drawn:?}"
        );
    }
}

/// A phrase that cannot fit is dropped rather than wrapped: `inquire` gives one row per
/// item, so a phrase that overflows becomes a second item's worth of mess.
#[test]
fn a_narrow_menu_drops_the_phrase_rather_than_wrapping_it() {
    let drawn = option("Rename one", "the name sloop files it under", 10, Some(30));
    assert_eq!(drawn.trim_start(), "Rename one");
    assert!(drawn.starts_with(&" ".repeat(super::UNDER)), "{drawn:?}");
}

#[test]
fn a_menu_item_fits_the_terminal_it_is_drawn_in() {
    for columns in [40usize, 60, 80, 120] {
        let drawn = plain(&option(
            "Delete one from the server",
            "the database itself, gone for good. `sloop backup` first if you want a copy",
            26,
            Some(columns),
        ));
        // Two columns for the prefix `inquire` draws in front of every row, and the row
        // still has to stop short of the right edge.
        assert!(
            drawn.chars().count() + 2 < columns,
            "a {columns}-column terminal got a {}-character item: {drawn:?}",
            drawn.chars().count()
        );
    }
}

#[test]
fn wrapping_keeps_every_word_and_breaks_only_between_them() {
    let text = "the destination ends up identical and anything only it had is gone";
    let rows = wrap(text, 20);

    assert!(rows.iter().all(|row| row.chars().count() <= 20), "{rows:?}");
    assert_eq!(rows.join(" "), text, "a word went missing: {rows:?}");
}

/// A path longer than the measure overhangs rather than being cut in half — half a path is
/// worse than a ragged edge.
#[test]
fn a_word_longer_than_the_measure_is_left_whole() {
    let long = "C:/Users/somebody/very/deep/directory/that/will/not/fit/anywhere/.sloop";
    assert_eq!(wrap(long, 20), vec![long.to_owned()]);
}

/// **The owner asked for the version under the mark**, and for a reason `--version` does
/// not cover: somebody looking at the menu should not have to leave it to find out which
/// build is in front of them. See "Colour, and the version under the wordmark" in
/// `docs/OWNER-DECISIONS.md`.
#[test]
fn the_version_sits_under_the_wordmark() {
    let drawn = plain(&frame(&home(), Some(100)));
    let rows: Vec<&str> = drawn.lines().collect();

    let block = rows
        .iter()
        .rposition(|row| row.contains("|_|"))
        .expect("the wordmark is drawn");
    let version = rows
        .iter()
        .position(|row| row.contains(VERSION))
        .expect("the version is drawn");

    assert!(
        version > block,
        "the version is not under the mark:\n{drawn}"
    );
    assert!(
        version - block <= 2,
        "the version drifted away from the mark:\n{drawn}"
    );
    assert!(VERSION.starts_with('v'), "{VERSION}");
}

/// Every deeper screen carries the word and a breadcrumb; only the top carries the block,
/// and only the top carries the version with it.
#[test]
fn a_deeper_screen_carries_neither_the_block_nor_the_version() {
    let drawn = plain(&frame(&deeper(), Some(100)));
    assert!(!drawn.contains(VERSION), "{drawn}");
}

/// **Colour, and it is the palette's.** Every escape in a drawn screen has to be one of
/// the six — an ANSI colour name or a stray attribute would be a seventh thing on screen
/// that nobody chose.
#[test]
fn every_colour_on_a_screen_is_one_of_the_six() {
    use crate::style::{Ink, ink};

    let allowed: Vec<String> = [
        Hue::Brand,
        Hue::Text,
        Hue::Dim,
        Hue::Ok,
        Hue::Warn,
        Hue::Bad,
    ]
    .into_iter()
    .map(|hue| match ink(hue) {
        Ink::True(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        Ink::Cube(index) => format!("\x1b[38;5;{index}m"),
    })
    .collect();

    let drawn = frame(&home(), Some(100));
    assert!(
        drawn.contains('\x1b'),
        "nothing was coloured at all:\n{drawn}"
    );

    let mut rest = drawn.as_str();
    while let Some(start) = rest.find('\x1b') {
        let after = &rest[start..];
        let end = after.find('m').expect("an SGR sequence ends in m") + 1;
        let sequence = &after[..end];
        assert!(
            sequence == "\x1b[0m"
                || sequence == "\x1b[1m"
                || allowed.iter().any(|colour| colour == sequence),
            "{sequence:?} is not one of the six"
        );
        rest = &after[end..];
    }
}

/// The wordmark is the brand's colour and the labels beside it are not — the whole point
/// of a palette is that two things which mean different things look different.
#[test]
fn the_mark_and_the_labels_are_not_the_same_colour() {
    let drawn = frame(&home(), Some(100));
    let brand = crate::style::in_hue(Hue::Brand, "|_|");
    assert!(
        drawn.contains(brand.split("|_|").next().unwrap_or_default()),
        "the wordmark lost the accent"
    );
    assert!(
        drawn.contains(&crate::style::in_hue(Hue::Dim, "working in")),
        "a label is not grey"
    );
    assert!(
        drawn.contains(&crate::style::in_hue(Hue::Ok, "2 databases")),
        "a count that is good news is not green"
    );
}

/// Nothing may be coloured once the user has said not to. The decision itself, with the
/// three answers handed in — `coloured` may only be asked once per process.
#[test]
fn the_user_always_gets_the_last_word_on_colour() {
    use super::wanted;
    use anstream::ColorChoice::{Always, Auto, Never};

    assert!(wanted(Auto, None, None), "a plain terminal session");
    assert!(wanted(Always, None, None));
    assert!(wanted(Auto, Some(""), None), "an empty NO_COLOR is not set");

    assert!(!wanted(Never, None, None), "--no-color");
    assert!(!wanted(Auto, Some("1"), None), "NO_COLOR");
    assert!(!wanted(Always, Some("anything"), None), "NO_COLOR outranks");
    assert!(!wanted(Auto, None, Some("0")), "CLICOLOR=0");
    assert!(wanted(Auto, None, Some("1")), "CLICOLOR=1");
}

/// **A strapline is never cut.** It moves under the version when it will not fit beside
/// it, because three words quietly disappearing off the end of a narrow terminal is the
/// kind of defect nobody notices for a year.
#[test]
fn a_strapline_keeps_every_word_at_every_width() {
    for columns in [40usize, 60, 80, 120] {
        let drawn = plain(&frame(&home(), Some(columns)));
        for word in home().strap.split_whitespace() {
            assert!(
                drawn.contains(word),
                "a {columns}-column terminal lost {word:?}:\n{drawn}"
            );
        }
    }
}

/// And it still fits: beside the version on an ordinary terminal, under it on a narrow one.
#[test]
fn the_strapline_sits_beside_the_version_when_there_is_room() {
    let wide = plain(&frame(&home(), Some(100)));
    assert!(
        wide.lines()
            .any(|row| row.contains(VERSION) && row.contains("backed up")),
        "the strapline left the version's line on a wide terminal:\n{wide}"
    );

    let narrow = plain(&frame(&home(), Some(44)));
    assert!(
        narrow.lines().any(|row| row.trim() == VERSION),
        "the strapline did not move under the version on a narrow one:\n{narrow}"
    );
}

/// **The defect this exists for.** A registry path is as long as somebody's home
/// directory. Left unwrapped it ran off the right edge and came back around the left one
/// in the middle of a word, which is the single worst thing a screen can look like.
#[test]
fn a_long_value_wraps_into_its_own_column_rather_than_off_the_edge() {
    let header = Header {
        banner: Banner::Word,
        crumbs: vec!["Databases"],
        strap: String::new(),
        lines: vec![
            Line::fact(
                "working in",
                r"C:\Users\somebody\Documents\work\a-fairly-deep-project\services\orders\.sloop",
            ),
            Line::under("the nearest .sloop at or above the working directory"),
        ],
    };

    for columns in [40usize, 60, 80, 120] {
        let drawn = plain(&frame(&header, Some(columns)));
        for row in drawn.lines() {
            assert!(
                row.chars().count() <= columns,
                "a {columns}-column terminal produced a {}-character row: {row:?}",
                row.chars().count()
            );
        }

        // And the continuation lines line up under the value rather than under the label.
        let value_at = drawn
            .lines()
            .find(|row| row.contains("working in"))
            .map(|row| row.find("C:").expect("the value is on the label's row"));
        for row in drawn
            .lines()
            .filter(|row| !row.contains("working in") && row.contains(".sloop"))
        {
            assert_eq!(
                row.len() - row.trim_start().len(),
                value_at.expect("the label row was drawn"),
                "a wrapped value did not line up: {row:?}"
            );
        }
    }
}

/// Prose keeps its long words whole; a value does not. The two are different jobs and the
/// screen would be wrong either way round.
#[test]
fn a_value_breaks_where_prose_would_overhang() {
    let long = "C:/Users/somebody/very/deep/directory/that/will/not/fit/anywhere/.sloop";

    assert_eq!(wrap(long, 20), vec![long.to_owned()], "prose kept it whole");

    let broken = super::wrap_value(long, 20);
    assert!(broken.len() > 1, "a value was left to overhang: {broken:?}");
    assert!(
        broken.iter().all(|row| row.chars().count() <= 20),
        "{broken:?}"
    );
    assert_eq!(broken.concat(), long, "a value lost characters: {broken:?}");
}

/// The highlighted line is repainted whole, not tinted.
///
/// A row carries colours of its own — a grey command beside a plain title — and a highlight
/// that only changed the first of them would leave half a row looking unselected.
#[test]
fn the_highlighted_line_is_all_one_colour() {
    let row = option("Rename one", "sloop db rename <from> <to>", 12, Some(100));
    let lit = super::chosen(&row);

    assert_eq!(
        plain(&lit),
        plain(&row),
        "the highlight changed what the line says"
    );
    assert_ne!(lit, row, "the highlight changed nothing");
    assert_eq!(
        lit.matches("\x1b[0m").count(),
        1,
        "the highlighted line still carries the colours underneath it: {lit:?}"
    );
    assert!(
        lit.contains(&crate::style::heading("Rename one")[..6]),
        "the highlight is not the accent: {lit:?}"
    );
}
