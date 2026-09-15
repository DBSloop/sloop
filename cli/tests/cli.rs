//! The command surface, exercised through the real binary.
//!
//! `CARGO_BIN_EXE_sloop` is the binary this test was compiled alongside, so these run
//! against the artefact that ships rather than against the library behind it — and they
//! cost no dependency to do it, which matters in a project whose headline claim is what
//! its dependency graph does not contain.

use std::process::{Command, Output};

/// Every command that does something. Each one is a stub today; each one is a task.
const LEAVES: &[&[&str]] = &[
    &["init"],
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

/// The commands that only hold other commands.
const GROUPS: &[&[&str]] = &[&["db"], &["backups"], &["key"]];

fn sloop(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sloop"))
        .args(args)
        // Whatever the machine running the tests has set, these decide colour, and a
        // test asserting on plain text should not be at their mercy.
        .env_remove("CLICOLOR_FORCE")
        .env_remove("CLICOLOR")
        .env_remove("NO_COLOR")
        .output()
        .expect("the binary these tests were built alongside should run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("output should be UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("output should be UTF-8")
}

/// `--help` is wrapped to the terminal, so a phrase in it can be split by a newline at
/// any moment. Collapsing runs of whitespace asks what the help *says* rather than what
/// width it happened to be rendered at.
fn unwrapped_stdout(output: &Output) -> String {
    stdout(output)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn code(output: &Output) -> Option<i32> {
    output.status.code()
}

#[test]
fn the_version_names_the_command_not_the_crate() {
    let out = sloop(&["--version"]);

    assert_eq!(code(&out), Some(0));
    assert_eq!(
        stdout(&out).trim(),
        format!("sloop {}", env!("CARGO_PKG_VERSION"))
    );
    // The crate is `dbsloop` because crates.io already had `sloop`. Nobody types that.
    assert!(!stdout(&out).contains("dbsloop"));
}

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    for flag in ["-h", "--help"] {
        let out = sloop(&[flag]);
        assert_eq!(code(&out), Some(0), "{flag}");
        assert!(!stdout(&out).is_empty(), "{flag} printed nothing");
        assert!(stderr(&out).is_empty(), "{flag} wrote to stderr");
    }
}

#[test]
fn the_whole_surface_documents_itself() {
    for path in LEAVES.iter().chain(GROUPS) {
        for flag in ["-h", "--help"] {
            let args: Vec<&str> = path.iter().copied().chain([flag]).collect();
            let out = sloop(&args);
            assert_eq!(code(&out), Some(0), "`sloop {}` {flag}", path.join(" "));
            assert!(
                !stdout(&out).is_empty(),
                "`sloop {}` {flag} printed nothing",
                path.join(" ")
            );
        }
    }
}

#[test]
fn every_command_is_reachable_and_admits_it_is_a_stub() {
    for path in LEAVES {
        let out = sloop(path);
        let name = path.join(" ");

        assert_eq!(code(&out), Some(1), "`sloop {name}` should exit 1 for now");
        assert!(
            stdout(&out).is_empty(),
            "`sloop {name}` wrote to stdout; a stub has nothing to pipe"
        );
        assert!(
            stderr(&out).contains("not implemented yet"),
            "`sloop {name}` said {:?} instead",
            stderr(&out)
        );
        assert!(
            stderr(&out).contains(&name),
            "`sloop {name}` never named the command back"
        );
    }
}

#[test]
fn no_command_at_all_is_the_interactive_menu() {
    let out = sloop(&[]);

    assert_eq!(code(&out), Some(1));
    assert!(stderr(&out).contains("interactive menu"));
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    let out = sloop(&["definitely-not-a-command"]);

    // 2 is frozen. A scheduler reads it and stops retrying.
    assert_eq!(code(&out), Some(2));
    assert!(
        stdout(&out).is_empty(),
        "a usage error must not pollute stdout"
    );
    assert!(stderr(&out).contains("definitely-not-a-command"));
}

#[test]
fn a_group_with_no_command_under_it_is_a_usage_error() {
    for group in GROUPS {
        let out = sloop(group);
        assert_eq!(code(&out), Some(2), "`sloop {}`", group.join(" "));
    }
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    let out = sloop(&["--not-a-flag"]);

    assert_eq!(code(&out), Some(2));
    assert!(stdout(&out).is_empty());
}

#[test]
fn help_is_plain_text_when_nothing_is_watching() {
    // These tests read stdout through a pipe, which is exactly the case that has to come
    // out clean: `sloop --help | less`, `sloop --help > notes.txt`, and every CI log.
    for args in [vec!["-h"], vec!["--help"], vec!["db", "--help"]] {
        let out = sloop(&args);
        assert!(
            !stdout(&out).contains('\x1b'),
            "`sloop {}` leaked an escape sequence into a pipe",
            args.join(" ")
        );
    }
}

#[test]
fn the_guarantee_is_the_first_thing_long_help_says() {
    let help = unwrapped_stdout(&sloop(&["--help"]));

    assert!(help.contains("credentials never leave this machine"));
    assert!(help.contains("no HTTP client"));
    assert!(help.contains("cargo tree"));
}

#[test]
fn long_help_prints_the_frozen_exit_codes() {
    let help = unwrapped_stdout(&sloop(&["--help"]));

    for line in [
        "0 success",
        "2 usage",
        "3 connect",
        "4 dump",
        "5 restore",
        "6 mismatch",
        "7 locked",
    ] {
        assert!(help.contains(line), "long help never mentioned `{line}`");
    }
}

#[test]
fn long_help_separates_mirror_from_sync() {
    let help = unwrapped_stdout(&sloop(&["--help"]));

    assert!(help.contains("An exact copy"));
    assert!(help.contains("A merge"));
}
