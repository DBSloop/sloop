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
    let run = sandbox.sloop_in(&deep, &["db", "list"]);

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
        .sloop_in(&below, &["db", "list"])
        .expect_said(&inner.join(".sloop").display().to_string());
}

#[test]
fn with_no_project_anywhere_it_reads_the_global_store() {
    let sandbox = Sandbox::new("no-project");
    let run = sandbox.sloop(&["db", "list"]);

    run.expect_said(&sandbox.global_dir().display().to_string());
    run.expect_said("no .sloop");
}

#[test]
fn global_ignores_a_project_that_is_right_here() {
    let sandbox = Sandbox::new("global-flag");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let run = sandbox.sloop_in(&project, &["--global", "db", "list"]);

    run.expect_said(&sandbox.global_dir().display().to_string());
    run.expect_said("--global");
    // The project's own registry, named in full: the global store is `~/.sloop` now, so
    // ".sloop" alone is a substring of the right answer as well as the wrong one.
    run.expect_silent_about(&project.join(".sloop").display().to_string());
}

#[test]
fn a_name_is_looked_for_in_the_project_before_the_global_store() {
    let sandbox = Sandbox::new("collision-rule");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    // **One database in each store**, because the line that says where a bare name is
    // looked for is printed with the listing — and an empty registry has no listing. That
    // is also the only state in which the sentence means anything.
    sandbox
        .sloop_in(
            &project,
            &[
                "db",
                "add",
                "here",
                "--url",
                "postgres://a@h/d",
                "--env",
                "PW",
            ],
        )
        .expect_code(0);
    sandbox
        .sloop_in(
            &project,
            &[
                "--global",
                "db",
                "add",
                "there",
                "--url",
                "postgres://a@h/d",
                "--env",
                "PW",
            ],
        )
        .expect_code(0);

    sandbox
        .sloop_in(&project, &["db", "list"])
        .expect_said("this project, then the global store");

    sandbox
        .sloop_in(&project, &["--global", "db", "list"])
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
        .sloop_in(&elsewhere, &["-C", "demo", "db", "list"])
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
        .sloop_in(
            &elsewhere,
            &["-C", &project.display().to_string(), "db", "list"],
        )
        .expect_said(&project.join(".sloop").display().to_string());
}

#[test]
fn a_flag_that_names_nothing_is_a_usage_error() {
    let sandbox = Sandbox::new("bad-flag");
    sandbox
        .sloop(&["-C", "no-such-project", "db", "list"])
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
        .command(&elsewhere, &["db", "list"])
        .env("SLOOP_PROJECT", "demo")
        .run()
        .expect_said(&project.join(".sloop").display().to_string())
        .expect_said("named by SLOOP_PROJECT");
}

#[test]
fn an_environment_variable_that_names_nothing_is_a_usage_error() {
    let sandbox = Sandbox::new("env-bad");
    sandbox
        .command(sandbox.work(), &["db", "list"])
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
        .command(&project, &["db", "list"])
        .env("SLOOP_PROJECT", "")
        .run()
        .expect_code(0)
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
        .command(&elsewhere, &["-C", "wanted", "db", "list"])
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
        .sloop_in(&project, &["--global", "-C", "demo", "db", "list"])
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
        .sloop_in(&elsewhere, &["-C", "demo", "db", "list"])
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
        .sloop_in(&elsewhere, &["-C", "the demo", "db", "list"])
        .expect_said(&project.join(".sloop").display().to_string());
}

/// `git` is present on every machine this project is developed or built on, but a test
/// that hard-fails without it would be a bad neighbour.
fn git_or_skip() -> Option<String> {
    let found = Command::new("git").arg("--version").output().ok()?;
    found.status.success().then(|| "git".to_owned())
}

// =======================================================================================
// R19b — the global store is `~/.sloop`, and local comes first
// =======================================================================================

/// Add a database to whichever store `where_` resolves to, with no password to fetch.
fn register(sandbox: &Sandbox, where_: &Path, args: &[&str]) {
    let mut all = args.to_vec();
    all.extend_from_slice(&["--url", "postgres://a@h/d", "--env", "PW"]);
    sandbox.sloop_in(where_, &all).expect_code(0);
}

#[test]
fn the_global_store_is_the_dot_sloop_in_the_home_directory() {
    let sandbox = Sandbox::new("home-store");
    register(&sandbox, sandbox.work(), &["db", "add", "orders"]);

    // Not `%APPDATA%\sloop`, not `~/.config/sloop`, not `~/Library/Application Support`.
    // `R19c4` moved the registry itself into PostgreSQL; what stays here is the record that
    // says which PostgreSQL, and it is still `~/.sloop` that holds it.
    assert!(
        sandbox.global_dir().join("server.toml").is_file(),
        "the global store should be at {}",
        sandbox.global_dir().display()
    );
    assert!(
        sandbox.registry_text().contains("[databases.orders]"),
        "the registration should be in sloop's own database"
    );
    assert!(
        !sandbox.legacy_dir().exists(),
        "nothing should have been written to {}",
        sandbox.legacy_dir().display()
    );
}

/// The defect `R18a` found, from the inside: the global store sits in the home directory
/// now, and a walk up the tree that reached it would call it a project — from the home
/// directory itself, and from every directory under it.
#[test]
fn the_global_store_is_never_mistaken_for_a_project() {
    let sandbox = Sandbox::new("home-not-project");
    register(&sandbox, sandbox.work(), &["db", "add", "orders"]);
    assert!(sandbox.global_dir().is_dir(), "the store should exist now");

    let deep = sandbox.make_dir("one/two/three");
    for standing_in in [sandbox.home(), sandbox.work(), deep.as_path()] {
        let run = sandbox.sloop_in(standing_in, &["db", "list"]);
        run.expect_code(0);
        // No project was resolved, from any of the three: the global store is the only
        // place a bare name is looked for.
        run.expect_said("looked for in the global store");
        run.expect_silent_about("this project, then");
    }

    // And nothing went on to treat the store as a project directory.
    assert!(
        !sandbox.global_dir().join(".sloop").exists(),
        "a .sloop appeared inside the global store"
    );
}

/// A real project under the home directory is still a project. The boundary stops the walk
/// at home; it does not stop it before it.
#[test]
fn a_project_under_the_home_directory_still_resolves_to_itself() {
    let sandbox = Sandbox::new("project-under-home");
    let project = sandbox.make_dir("code/app");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    let inside = sandbox.make_dir("code/app/src");
    let run = sandbox.sloop_in(&inside, &["db", "list"]);
    run.expect_code(0);
    run.expect_said(&project.join(".sloop").display().to_string());
}

/// Leave a store at the old path: let sloop write a real one, then move it there.
///
/// Hand-written TOML would be a second opinion about the file format; what has to be picked
/// up is a store somebody actually has. The project index goes in it too, because the whole
/// directory is what moves and a registry that arrived without its pointers would be a
/// migration that lost half the store.
fn leave_a_store_at_the_old_path(sandbox: &Sandbox, name: &str) {
    let old = sandbox.legacy_dir();
    std::fs::create_dir_all(old.join("projects")).expect("creatable");

    // **Written by hand, because an older release wrote a file.** `R19c4` moved the registry
    // into PostgreSQL, so `sloop db add` would put this database in the *machine's* store and
    // renaming a directory afterwards would not take it anywhere. What a machine upgrading
    // from the previous release actually has is a `registry.toml` at the old path — so that
    // is what these tests leave there, and the import is then a real import rather than a
    // rehearsal of one.
    std::fs::write(
        old.join("registry.toml"),
        format!(
            "version = 1

[databases.{name}]
engine = \"postgres\"
             host = \"h\"
port = 5432
database = \"d\"
user = \"a\"
             password = \"${{PW}}\"
"
        ),
    )
    .expect("writing the old registry");

    // The index that came with it, so "the index moved too" is provable by using it.
    let project = sandbox.make_dir("indexed");
    std::fs::create_dir_all(project.join(".sloop")).expect("creatable");
    std::fs::write(
        old.join("projects").join("indexed"),
        project.display().to_string(),
    )
    .expect("writing the pointer");
}

#[test]
fn an_older_store_is_moved_once_and_said_so() {
    let sandbox = Sandbox::new("adopt");
    leave_a_store_at_the_old_path(&sandbox, "orders");
    let old = sandbox.legacy_dir();

    let first = sandbox.sloop(&["db", "list"]);
    first.expect_code(0);
    first.expect_said(&format!(
        "Moved the global store from {} to {}",
        old.display(),
        sandbox.global_dir().display()
    ));
    // Moved, not copied: one store, and the databases in it are the ones that were there.
    first.expect_said("orders");
    assert!(!old.exists(), "{} should be gone", old.display());

    // The index came with it, which is only provable by using it.
    sandbox
        .sloop(&["-C", "indexed", "db", "list"])
        .expect_code(0)
        .expect_said("this project, then the global store");

    // Once. A second run has nothing left to move and says nothing about moving.
    let second = sandbox.sloop(&["db", "list"]);
    second.expect_code(0);
    second.expect_said("orders");
    second.expect_silent_about("Moved the global store");
}

/// An empty directory at the old path is not a store, and announcing a migration that
/// migrated nothing is worse than staying quiet.
#[test]
fn an_empty_directory_at_the_old_path_is_left_alone() {
    let sandbox = Sandbox::new("adopt-empty");
    std::fs::create_dir_all(sandbox.legacy_dir()).expect("creatable");

    let run = sandbox.sloop(&["db", "list"]);
    run.expect_code(0);
    run.expect_silent_about("Moved the global store");
}

/// Both at once — which only happens if something recreated the old path after the move.
/// It is said rather than ignored, because the alternative is somebody registering
/// databases into a store nothing reads.
#[test]
fn a_store_at_the_old_path_is_never_read_alongside_the_new_one() {
    let sandbox = Sandbox::new("adopt-both");

    // **A store at the new path first.** `R19c4` put the registry in PostgreSQL, so a
    // registration alone leaves nothing in `~/.sloop` — what makes it a *store* rather than
    // just the record of where the database is, is the project index. `init` writes one.
    let project = sandbox.make_dir("here");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);
    register(&sandbox, sandbox.work(), &["db", "add", "current"]);

    // And the old directory back afterwards: the move already happened once, and this is the
    // machine where something recreated it.
    leave_a_store_at_the_old_path(&sandbox, "stale");
    let old = sandbox.legacy_dir();
    assert!(sandbox.global_dir().is_dir() && old.is_dir(), "both, now");

    let run = sandbox.sloop(&["db", "list"]);
    run.expect_code(0);
    run.expect_said("current");
    run.expect_said(&format!("{} also holds a sloop store", old.display()));
    run.expect_silent_about("stale");
    assert!(
        old.exists(),
        "the old store is left where it is, not merged"
    );
}

/// Local first, and no other project's: the rule the owner asked for, seen from outside.
#[test]
fn a_listing_puts_this_project_first_and_no_other_project_at_all() {
    let sandbox = Sandbox::new("local-first");
    let mine = sandbox.make_dir("mine");
    let theirs = sandbox.make_dir("theirs");
    sandbox.sloop_in(&mine, &["init"]).expect_code(0);
    sandbox.sloop_in(&theirs, &["init"]).expect_code(0);

    // A name that sorts last, in the project — so being printed first can only be the
    // search order and never the alphabet.
    register(&sandbox, &mine, &["db", "add", "zulu"]);
    register(&sandbox, &theirs, &["db", "add", "not-mine"]);
    register(&sandbox, &mine, &["--global", "db", "add", "alpha"]);

    let run = sandbox.sloop_in(&mine, &["db", "list"]);
    run.expect_code(0);
    run.expect_silent_about("not-mine");

    let said = run.said();
    let project = said.find("zulu").expect("the project's database is listed");
    let global = said.find("alpha").expect("the global database is listed");
    assert!(
        project < global,
        "this project's databases come first:\n{said}"
    );
}

/// The other half of "never treated as a project": the route that would make one. `~/.sloop`
/// is the global store, so `init` in the home directory would create the store and then
/// record it in itself.
#[test]
fn init_refuses_the_home_directory() {
    let sandbox = Sandbox::new("init-home");

    let run = sandbox.sloop_in(sandbox.home(), &["init"]);
    run.expect_code(2);
    run.expect_said("home directory");
    // `R19c` puts `server.toml` in `~/.sloop` on every set-up machine, so the directory
    // existing proves nothing. What `init` would have created is a project — an index entry
    // pointing at the home directory — and that is what must not be there.
    assert!(
        !sandbox.global_dir().join("projects").exists(),
        "init recorded the home directory as a project"
    );

    // And by name, from somewhere else, which is the same refusal read from the flag.
    sandbox
        .sloop(&["-C", &sandbox.home().display().to_string(), "init"])
        .expect_code(2)
        .expect_said("home directory");
}

/// A stale pointer naming the home directory — the exact entry this machine's index was
/// found holding after the `R18a` defect. It is refused rather than handing back the global
/// store dressed as a project.
#[test]
fn an_indexed_project_naming_the_home_directory_is_refused() {
    let sandbox = Sandbox::new("stale-pointer");
    register(&sandbox, sandbox.work(), &["db", "add", "orders"]);

    let pointer = sandbox.pointer("me");
    std::fs::create_dir_all(pointer.parent().expect("a file has a parent")).expect("creatable");
    std::fs::write(&pointer, sandbox.home().display().to_string()).expect("writable");

    sandbox
        .sloop(&["-C", "me", "db", "list"])
        .expect_code(2)
        .expect_said("home directory");

    sandbox
        .command(sandbox.work(), &["db", "list"])
        .env("SLOOP_PROJECT", "me")
        .run()
        .expect_code(2)
        .expect_said("home directory");
}

/// **The move survives `--json` and `--quiet`, and stdout stays the document.**
///
/// A scheduled run is exactly the run that would relocate the store, and one nobody was told
/// about is the same as one that did not happen. It goes on standard error, so a `--json`
/// consumer reading standard output is unaffected.
#[test]
fn the_move_is_reported_even_to_a_run_that_asked_for_silence() {
    for flag in ["--json", "--quiet"] {
        let sandbox = Sandbox::new("adopt-quiet");
        leave_a_store_at_the_old_path(&sandbox, "orders");

        let run = sandbox.sloop(&[flag, "db", "list"]);
        run.expect_code(0);
        assert!(
            run.stderr().contains("Moved the global store"),
            "`sloop {flag} db list` said nothing about the move\n--- stderr ---\n{}",
            run.stderr()
        );
        assert!(
            !run.stdout().contains("Moved the global store"),
            "the move landed on standard output under {flag}\n--- stdout ---\n{}",
            run.stdout()
        );
        if flag == "--json" {
            let parsed: serde_json::Value =
                serde_json::from_str(&run.stdout()).expect("stdout should be one JSON document");
            assert_eq!(parsed["ok"], serde_json::Value::Bool(true));
        }
    }
}
