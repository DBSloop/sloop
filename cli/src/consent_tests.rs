//! The three flags, and what each one may not do.
//!
//! **Everything here runs with stdin closed**, which is what `cargo test` gives a test
//! binary and also the state every scheduled run is in. So these cover exactly the branch
//! rule 4 is about: no terminal, and a question that has to be answered in advance or not
//! at all. The terminal halves — a typed name, a `y` — are driven against the real binary
//! in `tests/consent.rs` and against a real cluster in `engine::cluster_tests`.

use super::{Consent, Consented, Destroying};
use crate::exit::Exit;

/// Nothing given at all.
fn nothing() -> Consent<'static> {
    Consent::given(false, false, None)
}

#[test]
fn a_question_with_no_flag_and_no_terminal_exits_two_naming_the_flag() {
    let failure = nothing()
        .asked("Forget it?", "--yes")
        .expect_err("there is nothing to ask at");

    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure.message().contains("Forget it?"),
        "{}",
        failure.message()
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("--yes")),
        "the hint has to name the flag"
    );
}

#[test]
fn yes_answers_a_question() {
    assert!(
        Consent::given(true, false, None)
            .asked("Forget it?", "--yes")
            .expect("--yes answers it")
    );
}

/// **The rule this module exists for.** `-y` is not a way to destroy something: rule 5 says
/// the name is typed, and a flag that stood in for a name nobody typed would be the click.
#[test]
fn yes_is_never_enough_to_destroy_a_named_thing() {
    let failure = Consent::given(true, false, None)
        .typed(&destroying("app_production"))
        .expect_err("--yes cannot answer for a name");

    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("--confirm app_production")),
        "it has to say which name to type: {:?}",
        failure.hint_text()
    );
}

/// Nor is `--force`. It overrides a refusal; it does not answer a question, and it has never
/// been a name.
#[test]
fn force_answers_nothing() {
    let forced = Consent::given(false, true, None);

    assert!(forced.forced());
    assert!(
        forced.asked("Forget it?", "--yes").is_err(),
        "--force answered a question"
    );
    assert!(
        forced.typed(&destroying("app")).is_err(),
        "--force destroyed something"
    );
}

/// And `--yes` is not a way past a refusal either — the two concepts stay apart in both
/// directions.
#[test]
fn yes_does_not_override_a_refusal() {
    assert!(!Consent::given(true, false, None).forced());
    assert!(!Consent::given(true, false, Some("app")).forced());
}

#[test]
fn a_confirm_that_matches_is_permission() {
    assert_eq!(
        Consent::given(false, false, Some("app_production"))
            .typed(&destroying("app_production"))
            .expect("the name matches"),
        Consented::ByFlag
    );
}

/// **The mistake this shape exists to catch.** A cron line that names the wrong database is
/// a cron line somebody edited carelessly, and it is refused before anything is contacted.
#[test]
fn a_confirm_naming_the_wrong_thing_refuses_and_says_both_names() {
    let failure = Consent::given(false, false, Some("app_staging"))
        .typed(&destroying("app_production"))
        .expect_err("the names differ");

    assert_eq!(failure.exit(), Exit::Usage);
    let said = failure.message();
    assert!(
        said.contains("app_staging") && said.contains("app_production"),
        "{said}"
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("nothing was contacted")),
        "it has to say that nothing happened"
    );
}

/// Compared exactly. A database name is case-sensitive on most of the platforms this runs
/// against, and "close enough" is not a standard to destroy data by.
#[test]
fn a_confirm_is_compared_exactly() {
    for given in [
        "App_Production",
        "app_production ",
        " app_production",
        "app-production",
    ] {
        assert!(
            Consent::given(false, false, Some(given))
                .typed(&destroying("app_production"))
                .is_err(),
            "{given} was accepted for app_production"
        );
    }
}

/// The early check is the same verdict as the full one, minus the prompt — so a command can
/// fail a scheduled run before it fetches a password.
#[test]
fn the_early_check_agrees_with_the_full_one() {
    let cases = [
        (Consent::given(false, false, Some("app")), true),
        (Consent::given(false, false, Some("other")), false),
        (Consent::given(true, true, None), false),
    ];

    for (consent, expected) in cases {
        assert_eq!(
            consent.checked_early(&destroying("app")).is_ok(),
            expected,
            "{consent:?}"
        );
        assert_eq!(
            consent.typed(&destroying("app")).is_ok(),
            expected,
            "{consent:?} disagreed with its own early check"
        );
    }
}

#[test]
fn declining_is_not_a_failure() {
    assert!(!Consented::Declined.granted());
    assert!(Consented::ByFlag.granted());
    assert!(Consented::AtTheTerminal.granted());
}

/// The thing these tests are pretending to destroy.
fn destroying(named: &str) -> Destroying<'_> {
    Destroying {
        named,
        noun: "database",
        action: "dropping a database",
    }
}
