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
