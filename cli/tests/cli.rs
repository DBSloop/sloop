//! The command surface, exercised through the real binary.
//!
//! `CARGO_BIN_EXE_sloop` is the binary this test was compiled alongside, so these run
//! against the artefact that ships rather than against the library behind it — and they
//! cost no dependency to do it, which matters in a project whose headline claim is what
//! its dependency graph does not contain.
//!
//! Everything runs inside a [`Sandbox`], which gives the child its own `HOME`, `APPDATA`
//! and working directory. No test may touch the machine it is running on.

mod support;

use support::Sandbox;

/// Every command that will do something one day and does not yet.
const STUBS: &[&[&str]] = &[
    &["doctor"],
    &["db", "add"],
    &["db", "list"],
    &["db", "test"],
    &["db", "edit"],
    &["db", "rename"],
    &["db", "remove"],
    &["db", "drop"],
    &["backup"],
    &["backups", "list"],
    &["backups", "prune"],
    &["restore"],
    &["mirror"],
    &["sync"],
    &["key", "export"],
    &["key", "import"],
    &["uninstall"],
];

/// Commands with a body. They move here one task at a time.
const IMPLEMENTED: &[&[&str]] = &[&["init"]];

/// The commands that only hold other commands.
const GROUPS: &[&[&str]] = &[&["db"], &["backups"], &["key"]];

/// Every path in the tree.
fn every_command() -> Vec<&'static [&'static str]> {
    STUBS
        .iter()
        .chain(IMPLEMENTED)
        .chain(GROUPS)
        .copied()
        .collect()
}

#[test]
fn the_version_names_the_command_not_the_crate() {
    let sandbox = Sandbox::new("version");
    let run = sandbox.sloop(&["--version"]);

    run.expect_code(0);
    assert_eq!(
        run.stdout().trim(),
        format!("sloop {}", env!("CARGO_PKG_VERSION"))
    );
    // The crate is `dbsloop` because crates.io already had `sloop`. Nobody types that.
    run.expect_silent_about("dbsloop");
}

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    let sandbox = Sandbox::new("help");
    for flag in ["-h", "--help"] {
        let run = sandbox.sloop(&[flag]);
        run.expect_code(0);
        assert!(!run.stdout().is_empty(), "{flag} printed nothing");
        assert!(run.stderr().is_empty(), "{flag} wrote to stderr");
    }
}

#[test]
fn the_whole_surface_documents_itself() {
    let sandbox = Sandbox::new("surface");
    for path in every_command() {
        for flag in ["-h", "--help"] {
            let args: Vec<&str> = path.iter().copied().chain([flag]).collect();
            let run = sandbox.sloop(&args);
            run.expect_code(0);
            assert!(!run.stdout().is_empty(), "{path:?} {flag} printed nothing");
        }
    }
}

#[test]
fn every_unimplemented_command_is_reachable_and_admits_it() {
    let sandbox = Sandbox::new("stubs");
    for path in STUBS {
        let run = sandbox.sloop(path);
        let name = path.join(" ");

        run.expect_code(1);
        assert!(
            run.stdout().is_empty(),
            "`sloop {name}` wrote to stdout; a stub has nothing to pipe"
        );
        run.expect_said("not implemented yet");
        run.expect_said(&name);
    }
}

#[test]
fn no_command_at_all_is_the_interactive_menu() {
    let sandbox = Sandbox::new("menu");
    sandbox
        .sloop(&[])
        .expect_code(1)
        .expect_said("interactive menu");
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    let sandbox = Sandbox::new("unknown");
    let run = sandbox.sloop(&["definitely-not-a-command"]);

    // 2 is frozen. A scheduler reads it and stops retrying.
    run.expect_code(2).expect_said("definitely-not-a-command");
    assert!(
        run.stdout().is_empty(),
        "a usage error must not pollute stdout"
    );
}

#[test]
fn a_group_with_no_command_under_it_is_a_usage_error() {
    let sandbox = Sandbox::new("group");
    for group in GROUPS {
        sandbox.sloop(group).expect_code(2);
    }
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    let sandbox = Sandbox::new("flag");
    let run = sandbox.sloop(&["--not-a-flag"]);

    run.expect_code(2);
    assert!(run.stdout().is_empty());
}

#[test]
fn output_is_plain_text_when_nothing_is_watching() {
    // These tests read the streams through a pipe, which is exactly the case that has to
    // come out clean: `sloop --help | less`, `sloop init > notes.txt`, and every CI log.
    let sandbox = Sandbox::new("plain");
    for args in [
        vec!["-h"],
        vec!["--help"],
        vec!["db", "--help"],
        vec!["init"],
    ] {
        let run = sandbox.sloop(&args);
        assert!(
            !run.stdout().contains('\u{1b}'),
            "`sloop {}` leaked an escape sequence into a pipe",
            args.join(" ")
        );
        assert!(
            !run.stderr().contains('\u{1b}'),
            "`sloop {}` leaked an escape sequence into a pipe",
            args.join(" ")
        );
    }
}

#[test]
fn the_guarantee_is_the_first_thing_long_help_says() {
    let sandbox = Sandbox::new("guarantee");
    sandbox
        .sloop(&["--help"])
        .expect_said("credentials never leave this machine")
        .expect_said("no HTTP client")
        .expect_said("cargo tree");
}

#[test]
fn long_help_prints_the_frozen_exit_codes() {
    let sandbox = Sandbox::new("codes");
    let run = sandbox.sloop(&["--help"]);

    for code in [
        "0 success",
        "2 usage",
        "3 connect",
        "4 dump",
        "5 restore",
        "6 mismatch",
        "7 locked",
    ] {
        run.expect_said(code);
    }
}

#[test]
fn long_help_separates_mirror_from_sync() {
    let sandbox = Sandbox::new("copying");
    sandbox
        .sloop(&["--help"])
        .expect_said("An exact copy")
        .expect_said("A merge");
}

#[test]
fn help_offers_both_ways_of_choosing_a_registry() {
    let sandbox = Sandbox::new("registry-flags");
    sandbox
        .sloop(&["--help"])
        .expect_said("--global")
        .expect_said("-C");
}
