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

/// **The finding this replaced `inquire`'s list for.** The highlight moves between things
/// that can be chosen, and a heading is not one of them — so stepping down past the last
/// item of a section lands on the first item of the next, never on the word between them.
#[test]
fn the_highlight_steps_over_a_heading_rather_than_onto_it() {
    use super::{Drawn, ends, lay_out, step, window};
    use crate::ui::screen::{Item, Row};

    let rows = vec![
        Row::Heading("ADD ONE".to_owned()),
        Row::Item(Item::new("Tell sloop about a database", ""), 0),
        Row::Heading("LOOK".to_owned()),
        Row::Item(Item::new("See the ones sloop knows", ""), 1),
        Row::Item(Item::new("Check one answers", ""), 2),
    ];
    let lines = lay_out(&rows, "← Back", "", 30, Some(100));
    let reachable: Vec<usize> = (0..lines.len())
        .filter(|line| lines[*line].picks.is_some())
        .collect();

    // Three items and the way out, and nothing else can be landed on at all.
    assert_eq!(reachable.len(), 4, "{lines:?}");

    // Down from the last item of a section is the first item of the next, stepping over
    // both the gap and the heading between them.
    assert_eq!(
        step(&lines, &reachable, 0, 1),
        Some(3),
        "it stopped at LOOK"
    );
    assert_eq!(
        step(&lines, &reachable, 1, -1),
        Some(1),
        "and again coming back"
    );

    // The ends are items too, never headings.
    assert_eq!(ends(&lines, &reachable, true), Some(1));
    assert_eq!(
        ends(&lines, &reachable, false),
        Some(rows.len()),
        "the way out"
    );

    // And there is nowhere past either end.
    assert_eq!(step(&lines, &reachable, 0, -1), None);
    assert_eq!(step(&lines, &reachable, reachable.len() - 1, 1), None);

    // A list that fits does not scroll, and one that does scrolls by as little as it can
    // — the way out is its last line, so it has to be reachable from the bottom.
    assert_eq!(window(0, 6, lines.len(), lines.len()), 0);
    assert_eq!(
        window(0, lines.len() - 1, lines.len(), 3),
        lines.len() - 3,
        "the way out scrolled off the bottom"
    );

    assert!(Drawn::gap().picks.is_none());
}

/// A filter narrows the list to what matches, and a heading whose section has been emptied
/// goes with it — a heading over nothing makes a narrowed list look broken.
#[test]
fn filtering_takes_an_emptied_heading_with_it() {
    use super::lay_out;
    use crate::ui::screen::{Item, Row};

    let rows = vec![
        Row::Heading("ADD ONE".to_owned()),
        Row::Item(Item::new("Tell sloop about a database", ""), 0),
        Row::Heading("LOOK".to_owned()),
        Row::Item(Item::new("See the ones sloop knows", ""), 1),
    ];

    let narrowed = lay_out(&rows, "← Back", "tell", 30, Some(100));
    let drawn: Vec<&str> = narrowed.iter().map(|line| line.text.trim()).collect();
    assert!(
        drawn.iter().any(|line| line.contains("Tell sloop")),
        "{drawn:?}"
    );
    assert!(!drawn.iter().any(|line| line.contains("LOOK")), "{drawn:?}");
    assert!(
        drawn.iter().any(|line| line.contains("Back")),
        "the way out went with it"
    );
}
