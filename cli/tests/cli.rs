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
///
/// **Empty since `R19c6`**, which gave `uninstall` a body — it was the last one. The list
/// stays rather than going, because the next command to be sketched before it is built goes
/// in here and the test below is what stops it shipping as a silent no-op.
const STUBS: &[&[&str]] = &[];

/// Every path in the tree, asked of the binary rather than kept in a list here.
///
/// **It used to be three hand-written lists, and by `R27a` they were four commands short.**
/// `query`, `setup`, `server` and `service` had all shipped without anybody adding them, so
/// the test that says *the whole surface documents itself* was quietly saying it about
/// two-thirds of the surface. A list of the commands, kept beside the commands, is a list
/// that goes stale — so this walks `--help` the way a user would, and a command that ships
/// is a command this covers on the same day.
fn every_command(sandbox: &Sandbox) -> Vec<Vec<String>> {
    fn walk(sandbox: &Sandbox, path: &[String], into: &mut Vec<Vec<String>>) {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let run = sandbox.sloop(&args);
        run.expect_code(0);

        for name in subcommands_in(&run.stdout()) {
            let mut under = path.to_vec();
            under.push(name);
            walk(sandbox, &under, into);
            into.push(under);
        }
    }

    let mut found = Vec::new();
    walk(sandbox, &[], &mut found);
    found.sort();
    assert!(
        found.len() > 30,
        "the walk found {} commands, which is fewer than sloop has — did `--help` change \
         shape?",
        found.len()
    );
    found
}

/// The names under `Commands:` in one `--help`, ignoring the wrapped continuation lines
/// clap indents further and the `help` command it adds itself.
fn subcommands_in(help: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in help.lines() {
        if line.starts_with("Commands:") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.trim().is_empty() {
            break;
        }
        let Some(rest) = line.strip_prefix("  ") else {
            break;
        };
        // A wrapped description is indented past the column the names sit in.
        if rest.starts_with(' ') {
            continue;
        }
        let Some((name, _)) = rest.split_once("  ") else {
            continue;
        };
        if name != "help" {
            names.push(name.to_owned());
        }
    }
    names
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
    for path in every_command(&sandbox) {
        for flag in ["-h", "--help"] {
            let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
            args.push(flag);
            let run = sandbox.sloop(&args);
            run.expect_code(0);
            assert!(!run.stdout().is_empty(), "{path:?} {flag} printed nothing");
        }
    }
}

/// **`R28`.** Every command reachable from `--help` says what it does in the summary its
/// parent lists it by — the same rule `cli::tests::every_command_says_what_it_does` holds
/// the clap tree to, checked here against what the binary actually prints.
#[test]
fn every_command_in_the_help_says_what_it_does() {
    let sandbox = Sandbox::new("surface-summaries");
    for path in every_command(&sandbox) {
        let parent: Vec<&str> = path[..path.len() - 1].iter().map(String::as_str).collect();
        let mut args = parent.clone();
        args.push("--help");
        let run = sandbox.sloop(&args);
        let help = run.stdout();

        let name = path.last().expect("a path has a last element");
        let summary = help
            .lines()
            .find_map(|line| {
                let rest = line.strip_prefix("  ")?;
                let (found, said) = rest.split_once("  ")?;
                (found == name).then(|| said.trim().to_owned())
            })
            .unwrap_or_else(|| panic!("`sloop {}` is listed with no summary", path.join(" ")));

        assert!(
            !summary.is_empty(),
            "`sloop {}` is listed with an empty summary",
            path.join(" ")
        );
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

/// **`sloop` on its own is the menu, and the menu is nothing but prompts.**
///
/// So with nothing to draw on it refuses at the door: the frozen `2`, and a line saying
/// what to do instead. Rule 4 — a run with no terminal must never be left waiting for an
/// answer nobody is there to give.
#[test]
fn no_command_at_all_is_the_menu_and_the_menu_needs_a_terminal() {
    let sandbox = Sandbox::new("menu");
    let run = sandbox.sloop(&[]);

    run.expect_code(2)
        .expect_said("needs a terminal")
        .expect_said("sloop --help");
    assert!(
        run.stdout().is_empty(),
        "the menu wrote to stdout; there is nothing there to pipe"
    );
}

/// **Nothing in the scrollback.** The menu owns the alternate screen buffer, and the only
/// thing worse than not opening it is opening it into something that is not a terminal —
/// a pipe or a log file full of `?1049` is a pipe nobody can read.
#[test]
fn the_menu_never_writes_a_screen_switch_into_a_pipe() {
    let sandbox = Sandbox::new("menu-pipe");
    let run = sandbox.sloop(&[]);

    for stream in [run.stdout(), run.stderr()] {
        assert!(
            !stream.contains("1049"),
            "the alternate screen was entered with nowhere to draw: {stream:?}"
        );
        assert!(
            !stream.contains('\r'),
            "the menu wrote a carriage return into a pipe: {stream:?}"
        );
    }
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

/// A command that only holds other commands, run on its own. Derived like the rest, so a
/// group added later is covered without anybody remembering to add it here.
#[test]
fn a_group_with_no_command_under_it_is_a_usage_error() {
    let sandbox = Sandbox::new("group");
    let all = every_command(&sandbox);
    let groups: Vec<&Vec<String>> = all
        .iter()
        .filter(|path| {
            all.iter()
                .any(|other| other.len() > path.len() && other.starts_with(path))
        })
        .collect();

    assert!(!groups.is_empty(), "no group commands were found to check");
    for group in groups {
        let args: Vec<&str> = group.iter().map(String::as_str).collect();
        sandbox.sloop(&args).expect_code(2);
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
        "8 doctor found a problem",
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

/// `doctor` is a report, and a report belongs on stdout where it can be piped.
///
/// What it finds depends on the machine, so the assertions are about the shape of the
/// answer rather than its content — every engine accounted for, the fetched directory named,
/// and an exit code that agrees with what the report just said.
#[test]
fn doctor_reports_on_every_engine_and_exits_on_what_it_found() {
    let sandbox = Sandbox::new("doctor");
    let run = sandbox.sloop(&["doctor"]);

    let report = run.stdout();
    for engine in ["postgres", "mysql", "mariadb"] {
        assert!(
            report.contains(engine),
            "`sloop doctor` said nothing about {engine}:
{report}"
        );
    }
    for tool in ["pg_dump", "pg_restore", "psql", "mysqldump", "mariadb-dump"] {
        assert!(
            report.contains(tool),
            "`sloop doctor` said nothing about {tool}:
{report}"
        );
    }

    // It looks inside the sandbox and not inside the real machine's store, which is what
    // makes this test safe to run at all.
    assert!(
        report.contains(&sandbox.global_dir().display().to_string()),
        "the fetched directory is not the sandbox's:
{report}"
    );

    // 0 when something is usable, 2 when nothing is. Derived from the report rather than
    // from an assumption about what this machine happens to have installed.
    let anything_ready = report.contains("— ready");
    run.expect_code(if anything_ready { 0 } else { 2 });
}

/// There is no terminal here — which is exactly a scheduled run's situation — so nothing is
/// offered, nothing is downloaded, and nothing waits for an answer.
#[test]
fn doctor_never_asks_to_install_anything_without_a_terminal() {
    let sandbox = Sandbox::new("doctor-quiet");
    let run = sandbox.sloop(&["doctor"]);

    for asking in ["[y/N]", "Download and install", "Run that"] {
        run.expect_silent_about(asking);
    }

    // And it fetched nothing: the directory it would install into is still not there.
    assert!(
        !sandbox.global_dir().join("tools").join("bin").is_dir(),
        "something was installed without anybody being asked"
    );
}
