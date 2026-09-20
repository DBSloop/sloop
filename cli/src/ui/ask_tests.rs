//! The edge where `inquire` stops and the screen stack starts.

use super::{Answer, navigate};
use crate::exit::Exit;

/// Esc and Ctrl-C are things the user did on purpose, so they come back as navigation.
/// Everything else is a session that cannot go on.
#[test]
fn a_cancelled_prompt_is_a_navigation_and_not_a_failure() {
    assert_eq!(
        navigate::<usize>(&inquire::InquireError::OperationCanceled).expect("esc is not an error"),
        Answer::Back
    );
    assert_eq!(
        navigate::<usize>(&inquire::InquireError::OperationInterrupted)
            .expect("ctrl-c is not an error"),
        Answer::Quit
    );
}

/// **Rule 4, one layer in.** `inquire` refuses to enable raw mode without a terminal; that
/// has to arrive as the frozen `2` with the fix named, not as a generic crash.
#[test]
fn no_terminal_is_a_usage_error_that_says_what_to_do_instead() {
    let refused =
        navigate::<usize>(&inquire::InquireError::NotTTY).expect_err("no terminal is a failure");

    assert_eq!(refused.exit(), Exit::Usage);
    assert!(
        refused
            .hint_text()
            .is_some_and(|hint| hint.contains("sloop --help")),
        "the refusal did not name the way out"
    );
}

/// Anything else is a failure, and not one of the frozen codes — none of them describes a
/// terminal that stopped working halfway through a menu.
#[test]
fn a_broken_screen_is_an_ordinary_failure() {
    let broken = navigate::<usize>(&inquire::InquireError::InvalidConfiguration(
        "no".to_owned(),
    ))
    .expect_err("a broken prompt is a failure");
    assert_eq!(broken.exit(), Exit::Failure);
}

/// `dress` reads the same decision every other line in the shell reads, and setting the
/// config twice is not a thing `inquire` minds — which is what makes it safe to call once
/// at the top of a session rather than per prompt.
#[test]
fn dressing_the_prompts_is_settled_once_and_costs_nothing_to_repeat() {
    dress();
    dress();
}

/// Every colour `inquire` is handed is one of the six, not a name out of its own enum —
/// otherwise a list drawn by `inquire` and a header drawn by `paint` are two palettes.
#[test]
fn inquire_is_handed_the_projects_own_colours() {
    for hue in [
        Hue::Brand,
        Hue::Text,
        Hue::Dim,
        Hue::Ok,
        Hue::Warn,
        Hue::Bad,
    ] {
        let handed = ink(hue);
        let expected = match crate::style::ink(hue) {
            crate::style::Ink::True(r, g, b) => inquire::ui::Color::Rgb { r, g, b },
            crate::style::Ink::Cube(index) => inquire::ui::Color::AnsiValue(index),
        };
        assert_eq!(
            handed, expected,
            "{hue:?} was handed over as something else"
        );
    }
}

use super::{Hue, dress, ink};

/// **The highlight never lands on something that is not a choice.** There are no headings
/// any more, but there is still the gap above the way out — and an arrow resting on a blank
/// row is a menu that looks broken for one keystroke.
#[test]
fn the_highlight_steps_over_the_gap_above_the_way_out() {
    use super::{Drawn, ends, lay_out, step, window};
    use crate::ui::paint::Widths;
    use crate::ui::screen::Item;

    let items = vec![
        Item::new("Tell sloop about a database", ""),
        Item::new("See the ones sloop knows", ""),
        Item::new("Check one answers", ""),
    ];
    let widths = Widths {
        title: 30,
        note: 0,
        ..Widths::default()
    };
    let lines = lay_out(&items, "\u{2190} Back", "", widths, Some(100));
    let reachable: Vec<usize> = (0..lines.len())
        .filter(|line| lines[*line].picks.is_some())
        .collect();

    // Three items and the way out, and nothing else can be landed on at all.
    assert_eq!(reachable.len(), 4, "{lines:?}");

    // Down from the last item is the way out, stepping over the gap between them.
    assert_eq!(step(&lines, &reachable, 2, 1), Some(items.len()));
    assert_eq!(step(&lines, &reachable, 3, -1), Some(2));

    assert_eq!(ends(&lines, &reachable, true), Some(0));
    assert_eq!(
        ends(&lines, &reachable, false),
        Some(items.len()),
        "the way out"
    );

    // And there is nowhere past either end.
    assert_eq!(step(&lines, &reachable, 0, -1), None);
    assert_eq!(step(&lines, &reachable, reachable.len() - 1, 1), None);

    // A list that fits does not scroll, and one that does scrolls by as little as it can
    // — the way out is its last line, so it has to be reachable from the bottom.
    assert_eq!(window(0, 3, lines.len(), lines.len()), 0);
    assert_eq!(
        window(0, lines.len() - 1, lines.len(), 3),
        lines.len() - 3,
        "the way out scrolled off the bottom"
    );

    assert!(Drawn::gap().picks.is_none());
}

/// A filter narrows the list to what matches, and never takes the way out with it.
#[test]
fn filtering_narrows_the_list_and_never_the_way_out() {
    use super::lay_out;
    use crate::ui::paint::Widths;
    use crate::ui::screen::Item;

    let items = vec![
        Item::new("Tell sloop about a database", ""),
        Item::new("See the ones sloop knows", ""),
    ];

    let narrowed = lay_out(
        &items,
        "\u{2190} Back",
        "tell",
        Widths {
            title: 30,
            note: 0,
            ..Widths::default()
        },
        Some(100),
    );
    let drawn: Vec<&str> = narrowed.iter().map(|line| line.text.trim()).collect();
    assert!(
        drawn.iter().any(|line| line.contains("Tell sloop")),
        "{drawn:?}"
    );
    assert!(
        !drawn.iter().any(|line| line.contains("See the")),
        "{drawn:?}"
    );
    assert!(
        drawn.iter().any(|line| line.contains("Back")),
        "the way out went with it"
    );
}

/// **The objection that reopened the doors, answered.** A front page of six lines was tried
/// before and put back, because `mirror` was not written on it and so could not be found by
/// typing. A door carries the names of everything behind it in `finds`, which the filter
/// reads and the screen never draws.
#[test]
fn the_filter_reads_what_is_behind_a_door() {
    use super::lay_out;
    use crate::ui::paint::Widths;
    use crate::ui::screen::Item;

    let mut door = Item::new("Copy a database", "mirror exactly, or merge");
    door.finds = "mirror sync clone".to_owned();
    let items = vec![Item::new("Databases", "3 registered"), door];

    let narrowed = lay_out(
        &items,
        "\u{2190} Back",
        "sync",
        Widths {
            title: 20,
            note: 26,
            ..Widths::default()
        },
        Some(100),
    );
    let drawn: Vec<&str> = narrowed.iter().map(|line| line.text.trim()).collect();
    assert!(
        drawn.iter().any(|line| line.contains("Copy a database")),
        "typing `sync` did not find the door that holds it: {drawn:?}"
    );
    assert!(
        !drawn.iter().any(|line| line.contains("Databases")),
        "{drawn:?}"
    );
    assert!(
        !narrowed.iter().any(|line| line.text.contains("clone")),
        "the filter's own words were drawn on the screen: {drawn:?}"
    );
}

/// **An item is stepped in from the arrow, and the way out is not.**
///
/// The step in is what makes a list read as a list rather than as a flat run; the way out
/// belongs to nothing above it, so it sits back at the arrow's own column.
#[test]
fn an_item_is_stepped_in_and_the_way_out_is_not() {
    use super::lay_out;
    use crate::ui::paint::Widths;
    use crate::ui::screen::Item;

    let items = vec![Item::new("Tell sloop about a database", "sloop db add")];
    let lines = lay_out(
        &items,
        "\u{2190} Back",
        "",
        Widths {
            title: 30,
            note: 12,
            ..Widths::default()
        },
        Some(100),
    );

    let starts = |text: &str| {
        let plain = anstream::adapter::strip_str(text).to_string();
        plain.len() - plain.trim_start().len()
    };

    let item = starts(&lines[0].text);
    let back = starts(&lines.last().expect("the way out is drawn").text);

    assert_eq!(
        item,
        crate::ui::paint::UNDER,
        "the step in is not the one the layout says it is"
    );
    assert_eq!(
        back, 0,
        "the way out belongs to no group and is not stepped in"
    );
}

// ---------------------------------------------------------------------------------------
// The menu does not blink
// ---------------------------------------------------------------------------------------

use super::redrawn_in_place;

/// What the terminal is actually told to do, so the sequences below read as themselves.
const HOME: &str = "\u{1b}[1;1H";
const ERASE_LINE: &str = "\u{1b}[K";
const ERASE_BELOW: &str = "\u{1b}[J";
const ERASE_EVERYTHING: &str = "\u{1b}[2J";

/// **The defect, named.** Blanking the screen and then drawing it leaves a real empty frame
/// in between — the terminal paints the blank, then paints the text — so every arrow key
/// made the whole menu blink. A frame that clears everything is the bug; this is the test
/// that would have caught it.
#[test]
fn a_frame_never_blanks_the_screen() {
    let frame = redrawn_in_place("one\r\ntwo\r\n", "\r\n", true);

    assert!(
        !frame.contains(ERASE_EVERYTHING),
        "the frame blanks the screen, which is what makes it blink: {frame:?}"
    );
}

/// Overwriting in place only works if each line erases its own tail: the frame before it may
/// have been wider, and what is left of it would stay on screen.
#[test]
fn every_line_erases_to_the_right_edge() {
    let frame = redrawn_in_place("one\r\ntwo\r\n", "\r\n", true);

    assert_eq!(
        frame.matches(ERASE_LINE).count(),
        2,
        "one erase per line, and no more: {frame:?}"
    );
    assert!(frame.contains(&format!("one{ERASE_LINE}\r\n")), "{frame:?}");
    assert!(frame.contains(&format!("two{ERASE_LINE}\r\n")), "{frame:?}");
}

/// Home first, so the frame lands on top of the one already there, and the rows below it
/// cleared last, so a frame that has become shorter leaves nothing behind.
#[test]
fn a_frame_starts_at_the_top_and_clears_what_is_left_under_it() {
    let frame = redrawn_in_place("only\r\n", "\r\n", true);

    let home = frame.find(HOME).expect("it goes home first");
    let text = frame.find("only").expect("it draws the line");
    let below = frame.find(ERASE_BELOW).expect("it clears below itself");

    assert!(
        home < text,
        "the cursor moves home before drawing: {frame:?}"
    );
    assert!(
        text < below,
        "it clears below itself after drawing, not before: {frame:?}"
    );
}

/// The list hides the cursor while it draws; the header does not, because a text box is
/// about to want it back.
#[test]
fn only_the_list_hides_the_cursor() {
    let hidden = redrawn_in_place("x\r\n", "\r\n", true);
    let shown = redrawn_in_place("x\n", "\n", false);

    assert!(hidden.contains("\u{1b}[?25l"), "{hidden:?}");
    assert!(!shown.contains("\u{1b}[?25l"), "{shown:?}");
}

/// A header's lines end in `\n` rather than `\r\n`, and they have to be erased too.
#[test]
fn the_header_erases_its_lines_as_well() {
    let frame = redrawn_in_place("a\nb\n", "\n", false);

    assert_eq!(frame.matches(ERASE_LINE).count(), 2, "{frame:?}");
    assert!(!frame.contains(ERASE_EVERYTHING), "{frame:?}");
}
