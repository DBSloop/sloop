//! What `sloop server install` accepts before it reaches the network, and what it refuses.
//!
//! Every refusal here happens **before** a byte is downloaded, which is the point of testing
//! them: a version that does not exist should be a sentence, not four hundred megabytes and
//! then a checksum failure.

use super::{Installing, engine_named, install, named_version};
use crate::engine::Engine;
use crate::tools::catalogue::Choice;

/// A version row with no build behind it, which is what MariaDB's list is made of.
fn major(version: &str) -> Choice {
    Choice {
        version: version.to_owned(),
        note: "Long Term Support".to_owned(),
        build: None,
    }
}

/// The name a person types, however they type it.
#[test]
fn an_engine_is_named_the_way_anybody_would_name_it() {
    for spelling in ["postgres", "PostgreSQL", "  Postgres  "] {
        assert_eq!(engine_named(spelling).expect(spelling), Engine::Postgres);
    }
    assert_eq!(engine_named("mysql").expect("mysql"), Engine::Mysql);
    assert_eq!(engine_named("MariaDB").expect("mariadb"), Engine::Mariadb);
}

/// **An engine sloop does not speak yet is told apart from a typo**, because somebody who
/// asked for MongoDB did not misspell anything and a usage error that reads like they did is
/// a worse answer than the true one.
#[test]
fn an_engine_that_is_not_supported_yet_is_told_so_by_name() {
    let failure = engine_named("mongodb").expect_err("sloop does not speak MongoDB");
    assert!(
        failure.message().contains("MongoDB") && failure.message().contains("not speak"),
        "{}",
        failure.message()
    );

    let typo = engine_named("postgresss").expect_err("that is not an engine");
    assert!(
        typo.message().contains("is not an engine sloop knows"),
        "{}",
        typo.message()
    );
}

/// **A version that is not published is a refusal, never a guess.** Building a URL out of
/// whatever was typed would download a 404 page and fail its checksum — the same answer,
/// arrived at expensively.
#[test]
fn a_version_nobody_publishes_is_refused_before_anything_is_downloaded() {
    let choices = vec![major("11.8"), major("11.4"), major("10.11")];

    let failure = named_version(&choices, Engine::Mariadb, "7.1")
        .expect_err("MariaDB 7.1 is not on that list");
    assert_eq!(failure.exit(), crate::exit::Exit::Usage);
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("11.8")),
        "the refusal should say what it does have: {failure:?}"
    );
}

/// MariaDB's index lists majors, so a full version somebody read off a release note finds the
/// major it belongs to rather than nothing at all.
#[test]
fn a_full_version_finds_the_major_it_belongs_to() {
    let choices = vec![major("11.8"), major("11.4")];

    assert_eq!(
        named_version(&choices, Engine::Mariadb, "11.4.4")
            .expect("11.4.4 is in 11.4")
            .version,
        "11.4"
    );
    assert_eq!(
        named_version(&choices, Engine::Mariadb, "11.8")
            .expect("the major itself")
            .version,
        "11.8"
    );
    // And not by prefix alone: `11.44` is not in `11.4`.
    assert!(named_version(&choices, Engine::Mariadb, "11.44").is_err());
}

/// **Rule 4, and this is the sharpest case of it.** A scheduled run stopped on a question
/// about a four-hundred-megabyte download is the worst thing this tool can do, so without a
/// terminal a half-given answer is a refusal that names the flags rather than a prompt.
///
/// `cargo test` runs with standard input redirected, so this is the real condition rather
/// than a simulated one.
#[test]
fn without_a_terminal_a_missing_answer_names_the_flags_instead_of_asking() {
    use std::io::IsTerminal as _;

    if std::io::stdin().is_terminal() {
        eprintln!("skipping: this test binary was run with a terminal attached");
        return;
    }

    let store = std::env::temp_dir().join(format!("sloop-server-tty-{}", std::process::id()));

    for asked in [
        Installing {
            engine: None,
            version: None,
            yes: false,
        },
        Installing {
            engine: Some("mysql"),
            version: None,
            yes: true,
        },
        Installing {
            engine: Some("mysql"),
            version: Some("8.4.11"),
            yes: false,
        },
    ] {
        let failure = install(&store, &asked).expect_err("it should refuse rather than ask");
        assert_eq!(failure.exit(), crate::exit::Exit::Usage);
        assert!(
            failure.message().contains("no terminal to ask at"),
            "{}",
            failure.message()
        );
        assert!(
            failure
                .hint_text()
                .is_some_and(|hint| hint.contains("--version") && hint.contains("--yes")),
            "the refusal has to name the exact flags: {failure:?}"
        );
    }

    assert!(
        !store.exists(),
        "nothing should have been created on the way to refusing"
    );
}

/// **Every engine is on the screen, and the ones sloop does not speak say so.** This is
/// `R19d`'s own line — *"the ones sloop does not yet speak, named as not yet supported"* —
/// and the reason for it is that a menu which silently omits MongoDB reads as a tool that has
/// never heard of it.
#[test]
fn the_engine_screen_lists_what_sloop_cannot_install_as_well_as_what_it_can() {
    let rows = super::engine_rows();
    assert_eq!(rows.len(), crate::tools::catalogue::ENGINES.len());

    for listed in crate::tools::catalogue::ENGINES {
        assert!(
            rows.iter().any(|(_, line)| line.contains(listed.name)),
            "{} is not on the screen",
            listed.name
        );
    }

    for (engine, line) in &rows {
        if engine.is_none() {
            assert!(
                line.contains("not yet"),
                "an unsupported row has to say so where somebody reads it: {line}"
            );
            assert!(
                line.trim_start().starts_with("--"),
                "an unsupported row must not carry a number anybody can type: {line}"
            );
        }
    }
}

/// **The numbering counts only what can be chosen**, so nobody selects MongoDB by typing the
/// position it happens to sit at.
#[test]
fn the_numbers_on_the_engine_screen_skip_the_rows_nobody_can_choose() {
    let rows = super::engine_rows();

    let numbered: Vec<&str> = rows
        .iter()
        .filter(|(engine, _)| engine.is_some())
        .map(|(_, line)| line.split_whitespace().next().unwrap_or(""))
        .collect();

    assert_eq!(
        numbered,
        ["1.", "2.", "3."],
        "three engines, numbered one to three whatever sits between them"
    );

    // And the engine behind each number is the one on that line.
    let choosable: Vec<Engine> = rows.iter().filter_map(|(engine, _)| *engine).collect();
    assert_eq!(choosable, Engine::ALL.to_vec());
}

/// **A password is only printed where somebody is reading it.**
///
/// Under `cargo test` both streams are pipes the harness is capturing, which is exactly the
/// shape this refuses — so the test needs nothing mocked to be the real case. `--force` is
/// what answers it, because overriding a refusal that is there to protect something is the
/// one thing that flag is for.
#[test]
fn a_password_is_refused_where_it_would_land_in_a_pipe() {
    let failure = super::refuse_unless_somebody_is_reading(false)
        .expect_err("a test harness is not a terminal");

    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure.message().contains("somebody to read"),
        "{}",
        failure.message()
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("--force")),
        "the refusal never says what would answer it"
    );

    super::refuse_unless_somebody_is_reading(true).expect("--force answers it");
}
