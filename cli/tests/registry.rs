//! `sloop init` and the resolution order, driven through the real binary on a real disk.
//!
//! The unit tests in `src/registry/tests.rs` cover every branch against a fake world; this
//! file checks that the real one behaves the same — that a `.sloop` really appears, that
//! git really cannot see it, and that running from three directories down really finds the
//! project the way `git` does.
//!
//! **The probe is whichever command is still a stub.** Resolution is checked by running a
//! command that reads a registry and says which one it read, which only a command with no
//! body left to run will do. It was `backup` until R9, `restore` until R13 and `mirror` until
//! R14, so it is
//! `sync` now and will be something further down the list later. What is being tested
//! is the resolution order, and that is the same whichever command asks for it.

mod support;

use std::path::Path;
use std::process::Command;

use support::Sandbox;

/// A slice of the wordmark with no backslashes in it, so this file needs no escaping.
const WORDMARK_FRAGMENT: &str = "___| | ___";

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn init_creates_the_registry_and_says_where() {
    let sandbox = Sandbox::new("init");
    let project = sandbox.make_dir("demo");

    let run = sandbox.sloop_in(&project, &["init"]);

    run.expect_code(0);
    assert!(project.join(".sloop").is_dir(), "no .sloop appeared");
    run.expect_said("Project registry created");
    run.expect_said(&project.join(".sloop").display().to_string());
}

#[test]
fn init_prints_the_wordmark() {
    let sandbox = Sandbox::new("init-wordmark");
    let project = sandbox.make_dir("demo");

    let run = sandbox.sloop_in(&project, &["init"]);

    assert!(
        run.stdout().contains(WORDMARK_FRAGMENT),
        "the init screen lost its header:\n{}",
        run.stdout()
    );
}

#[test]
fn the_registry_ignores_itself_and_leaves_the_users_gitignore_alone() {
    let sandbox = Sandbox::new("ignore");
    let project = sandbox.make_dir("demo");
    let theirs = project.join(".gitignore");
    std::fs::write(&theirs, "target/\n").unwrap();

    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    // Ours ignores everything beside it, itself included.
    let ours = read(&project.join(".sloop").join(".gitignore"));
    let rules: Vec<&str> = ours
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .collect();
    assert_eq!(rules, ["*"]);

    // Theirs is exactly as they left it.
    assert_eq!(read(&theirs), "target/\n");
}

#[test]
fn git_really_cannot_see_the_registry() {
    let Some(git) = git_or_skip() else { return };

    let sandbox = Sandbox::new("git");
    let project = sandbox.make_dir("demo");
    assert!(
        Command::new(&git)
            .args(["init", "--quiet"])
            .current_dir(&project)
            .status()
            .expect("git init")
            .success()
    );

    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let status = Command::new(&git)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(&project)
        .output()
        .expect("git status");
    let listed = String::from_utf8_lossy(&status.stdout);

    assert!(
        listed.trim().is_empty(),
        "git noticed the registry:\n{listed}"
    );
}

#[test]
fn running_it_twice_is_not_an_error() {
    let sandbox = Sandbox::new("twice");
    let project = sandbox.make_dir("demo");

    sandbox.sloop_in(&project, &["init"]).expect_code(0);
    sandbox
        .sloop_in(&project, &["init"])
        .expect_code(0)
        .expect_said("already");
}

#[test]
fn init_records_the_project_under_its_directory_name() {
    let sandbox = Sandbox::new("name");
    let project = sandbox.make_dir("demo");

    sandbox
        .sloop_in(&project, &["init"])
        .expect_code(0)
        .expect_said("sloop -C demo");

    let pointer = sandbox.pointer("demo");
    assert!(pointer.is_file(), "no pointer at {}", pointer.display());
    assert_eq!(read(&pointer).trim(), project.display().to_string());
}

#[test]
fn a_second_project_of_the_same_name_is_told_rather_than_silently_stealing_it() {
    let sandbox = Sandbox::new("collide");
    let first = sandbox.make_dir("one/demo");
    let second = sandbox.make_dir("two/demo");

    sandbox.sloop_in(&first, &["init"]).expect_code(0);
    let run = sandbox.sloop_in(&second, &["init"]);

    // The registry is still made — the name is a convenience on top of it.
    run.expect_code(0);
    assert!(second.join(".sloop").is_dir());
    run.expect_said("already");
    run.expect_said(&second.display().to_string());

    // And the first project keeps the name.
    assert_eq!(
        read(&sandbox.pointer("demo")).trim(),
        first.display().to_string()
    );
}

#[test]
fn init_can_be_pointed_at_another_directory() {
    let sandbox = Sandbox::new("init-elsewhere");
    let project = sandbox.make_dir("demo");

    sandbox
        .sloop(&["-C", &project.display().to_string(), "init"])
        .expect_code(0);

    assert!(project.join(".sloop").is_dir());
}

#[test]
fn init_refuses_global_rather_than_quietly_doing_something_else() {
    let sandbox = Sandbox::new("init-global");
    sandbox
        .sloop(&["--global", "init"])
        .expect_code(2)
        .expect_said("--global");
}

#[test]
fn init_refuses_a_directory_that_is_not_there() {
    let sandbox = Sandbox::new("init-missing");
    sandbox
        .sloop(&["-C", "no-such-directory", "init"])
        .expect_code(2);
}

/// The headline of the whole entry: from three levels down it finds the project, the way
/// `git` finds `.git`.
#[test]
fn a_command_run_from_a_subdirectory_finds_the_project() {
    let sandbox = Sandbox::new("walk-up");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let deep = sandbox.make_dir("demo/src/inner/deeper");
    let run = sandbox.sloop_in(&deep, &["sync"]);

    run.expect_said(&project.join(".sloop").display().to_string());
    run.expect_said("nearest .sloop");
}

#[test]
fn the_nearest_project_wins_when_one_is_inside_another() {
    let sandbox = Sandbox::new("nested");
    let outer = sandbox.make_dir("outer");
    let inner = sandbox.make_dir("outer/inner");
    sandbox.sloop_in(&outer, &["init"]).expect_code(0);
    sandbox.sloop_in(&inner, &["init"]).expect_code(0);

    let below = sandbox.make_dir("outer/inner/src");
    sandbox
        .sloop_in(&below, &["sync"])
        .expect_said(&inner.join(".sloop").display().to_string());
}

#[test]
fn with_no_project_anywhere_it_reads_the_global_store() {
    let sandbox = Sandbox::new("no-project");
    let run = sandbox.sloop(&["sync"]);

    run.expect_said(&sandbox.global_dir().display().to_string());
    run.expect_said("no .sloop");
}

#[test]
fn global_ignores_a_project_that_is_right_here() {
    let sandbox = Sandbox::new("global-flag");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let run = sandbox.sloop_in(&project, &["--global", "sync"]);

    run.expect_said(&sandbox.global_dir().display().to_string());
    run.expect_said("--global");
    run.expect_silent_about(".sloop —");
}

#[test]
fn a_name_is_looked_for_in_the_project_before_the_global_store() {
    let sandbox = Sandbox::new("collision-rule");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    sandbox
        .sloop_in(&project, &["sync"])
        .expect_said("this project, then the global store");

    sandbox
        .sloop_in(&project, &["--global", "sync"])
        .expect_said("looked for in the global store")
        .expect_silent_about("this project, then");
}

#[test]
fn the_flag_reaches_a_project_by_name_from_anywhere() {
    let sandbox = Sandbox::new("by-name");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let elsewhere = sandbox.make_dir("somewhere-else");
    sandbox
        .sloop_in(&elsewhere, &["-C", "demo", "sync"])
        .expect_said(&project.join(".sloop").display().to_string())
        .expect_said("named by -C");
}

#[test]
fn the_flag_reaches_a_project_by_path_from_anywhere() {
    let sandbox = Sandbox::new("by-path");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let elsewhere = sandbox.make_dir("somewhere-else");
    sandbox
        .sloop_in(&elsewhere, &["-C", &project.display().to_string(), "sync"])
        .expect_said(&project.join(".sloop").display().to_string());
}

#[test]
fn a_flag_that_names_nothing_is_a_usage_error() {
    let sandbox = Sandbox::new("bad-flag");
    sandbox
        .sloop(&["-C", "no-such-project", "sync"])
        .expect_code(2)
        .expect_said("no-such-project");
}

#[test]
fn the_environment_variable_does_what_the_flag_does() {
    let sandbox = Sandbox::new("env");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let elsewhere = sandbox.make_dir("somewhere-else");
    sandbox
        .command(&elsewhere, &["sync"])
        .env("SLOOP_PROJECT", "demo")
        .run()
        .expect_said(&project.join(".sloop").display().to_string())
        .expect_said("named by SLOOP_PROJECT");
}

#[test]
fn an_environment_variable_that_names_nothing_is_a_usage_error() {
    let sandbox = Sandbox::new("env-bad");
    sandbox
        .command(sandbox.work(), &["sync"])
        .env("SLOOP_PROJECT", "no-such-project")
        .run()
        .expect_code(2)
        .expect_said("SLOOP_PROJECT");
}

#[test]
fn an_empty_environment_variable_means_unset() {
    let sandbox = Sandbox::new("env-empty");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    sandbox
        .command(&project, &["sync"])
        .env("SLOOP_PROJECT", "")
        .run()
        .expect_code(1)
        .expect_said("nearest .sloop");
}

#[test]
fn the_flag_outranks_the_environment_variable() {
    let sandbox = Sandbox::new("precedence");
    let wanted = sandbox.make_dir("wanted");
    let other = sandbox.make_dir("other");
    sandbox.sloop_in(&wanted, &["init"]).expect_code(0);
    sandbox.sloop_in(&other, &["init"]).expect_code(0);

    let elsewhere = sandbox.make_dir("somewhere-else");
    sandbox
        .command(&elsewhere, &["-C", "wanted", "sync"])
        .env("SLOOP_PROJECT", "other")
        .run()
        .expect_said(&wanted.join(".sloop").display().to_string())
        .expect_silent_about(&other.join(".sloop").display().to_string());
}

#[test]
fn asking_for_both_registries_at_once_is_a_usage_error() {
    let sandbox = Sandbox::new("conflict");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    sandbox
        .sloop_in(&project, &["--global", "-C", "demo", "sync"])
        .expect_code(2);
}

#[test]
fn commands_that_read_no_registry_do_not_resolve_one() {
    let sandbox = Sandbox::new("no-registry");

    // The point is what these do *not* do, so the exit code is not part of it: `doctor`
    // reports on the machine and `uninstall` is still a stub, and either could change its
    // mind about what to exit with without changing the thing being asserted.
    for path in [["doctor"], ["uninstall"]] {
        sandbox.sloop(&path).expect_silent_about("would have read");
    }
}

/// A pointer written by a Windows editor has a carriage return on the end of it. The
/// failure that causes reads as "no such project" and is invisible in the file, so it is
/// checked on a real disk and not only in a unit test.
#[test]
fn a_pointer_written_with_windows_line_endings_still_resolves() {
    let sandbox = Sandbox::new("crlf");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let pointer = sandbox.pointer("demo");
    let crlf = format!(
        "{}
",
        project.display()
    );
    std::fs::write(&pointer, crlf).unwrap();

    let elsewhere = sandbox.make_dir("somewhere-else");
    sandbox
        .sloop_in(&elsewhere, &["-C", "demo", "sync"])
        .expect_said(&project.join(".sloop").display().to_string());
}

#[test]
fn a_project_path_with_spaces_in_it_survives_the_round_trip() {
    let sandbox = Sandbox::new("spaces");
    let project = sandbox.make_dir("My Projects/the demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    assert_eq!(
        read(&sandbox.pointer("the demo")).trim(),
        project.display().to_string()
    );

    let elsewhere = sandbox.make_dir("somewhere-else");
    sandbox
        .sloop_in(&elsewhere, &["-C", "the demo", "sync"])
        .expect_said(&project.join(".sloop").display().to_string());
}

/// `git` is present on every machine this project is developed or built on, but a test
/// that hard-fails without it would be a bad neighbour.
fn git_or_skip() -> Option<String> {
    let found = Command::new("git").arg("--version").output().ok()?;
    found.status.success().then(|| "git".to_owned())
}
