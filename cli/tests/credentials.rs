//! The registry file and the password routes, through the real binary on a real disk.
//!
//! The routes themselves are proved in `src/secret/tests.rs`, where a password full of
//! shell metacharacters goes through all four. What matters here is the file: that it
//! loads after a Windows editor has been at it, that it names the route without ever
//! naming a password, and that a password written into it is refused rather than used.

mod support;

use std::path::Path;

use support::Sandbox;

const REGISTRY: &str = r#"version = 1

[databases.staging]
engine = "postgres"
host = "db.internal"
database = "app"
user = "app_rw"
password = "keyring"

[databases.nightly]
engine = "mariadb"
host = "reports.internal"
database = "warehouse"
user = "reader"
password = "${REPORTS_PASSWORD}"

[databases.team]
engine = "mysql"
host = "team.internal"
database = "shared"
user = "svc"
password = "command:op read op://vault/db/password"
"#;

/// A project with a registry in it, and the path to that registry.
fn project_with(sandbox: &Sandbox, registry: &str) -> std::path::PathBuf {
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);
    std::fs::write(project.join(".sloop").join("registry.toml"), registry).unwrap();
    project
}

#[test]
fn a_registry_is_read_and_each_route_named() {
    let sandbox = Sandbox::new("routes");
    let project = project_with(&sandbox, REGISTRY);

    sandbox
        .sloop_in(&project, &["db", "list"])
        .expect_code(0)
        .expect_said("the OS keyring")
        .expect_said("the environment variable REPORTS_PASSWORD")
        .expect_said("the command `op read op://vault/db/password`");

    // The count comes from a command that still only describes what it would read, which
    // keeps the other half of the same reader — `describe_registry` — under test. It has
    // to be a command that is still a stub, so it moves down the list as tasks land: it
    // was `backup` until R9 gave that one a body.
    sandbox
        .sloop_in(&project, &["restore"])
        .expect_said("It holds 3 databases");
}

/// A registry saved by a Windows editor, or checked out with `core.autocrlf`, has to load.
#[test]
fn a_registry_with_windows_line_endings_loads() {
    let sandbox = Sandbox::new("crlf");
    let crlf = REGISTRY.replace('\n', "\r\n");
    let project = project_with(&sandbox, &crlf);

    let written = std::fs::read(project.join(".sloop").join("registry.toml")).unwrap();
    assert!(
        written.windows(2).any(|pair| pair == b"\r\n"),
        "the fixture lost its endings"
    );

    sandbox
        .sloop_in(&project, &["db", "list"])
        .expect_code(0)
        .expect_said("staging")
        .expect_said("the OS keyring");
}

/// The rule that makes rule 3 enforceable, checked where a person would actually hit it.
#[test]
fn a_password_written_into_the_registry_is_refused_and_the_reason_given() {
    let sandbox = Sandbox::new("plaintext");
    let project = project_with(
        &sandbox,
        "version = 1\n\n[databases.oops]\nengine = \"postgres\"\nhost = \"db\"\n\
         database = \"app\"\nuser = \"app\"\npassword = \"hunter2\"\n",
    );

    let run = sandbox.sloop_in(&project, &["db", "list"]);

    run.expect_code(2)
        // The file, so it can be opened.
        .expect_said("registry.toml")
        // Which entry, so it can be found in the file.
        .expect_said("oops")
        // And what to do instead — the half that a wrapped error used to lose.
        .expect_said("never written in a registry")
        .expect_said("encrypted-file");
}

#[test]
fn a_registry_that_will_not_parse_names_the_file_and_the_line() {
    let sandbox = Sandbox::new("broken");
    let project = project_with(
        &sandbox,
        "version = 1\n[databases.x]\nengine = \"oracle\"\n",
    );

    sandbox
        .sloop_in(&project, &["db", "list"])
        .expect_code(2)
        .expect_said("registry.toml");
}

#[test]
fn a_typo_in_a_field_name_is_refused_rather_than_silently_defaulted() {
    let sandbox = Sandbox::new("typo");
    let project = project_with(
        &sandbox,
        "version = 1\n\n[databases.staging]\nengine = \"postgres\"\nhosts = \"db\"\n\
         database = \"app\"\nuser = \"app\"\npassword = \"keyring\"\n",
    );

    sandbox.sloop_in(&project, &["db", "list"]).expect_code(2);
}

#[test]
fn the_flag_overrides_every_route_in_the_file() {
    let sandbox = Sandbox::new("override");
    let project = project_with(&sandbox, REGISTRY);

    let run = sandbox.sloop_in(
        &project,
        &[
            "--password-command",
            "op read op://vault/everything",
            "db",
            "list",
        ],
    );

    // Every entry, whatever its own route said. One flag, one answer for the whole run.
    run.expect_code(0)
        .expect_said("staging")
        .expect_said("nightly")
        .expect_said("the command `op read op://vault/everything`")
        .expect_silent_about("the OS keyring")
        .expect_silent_about("REPORTS_PASSWORD");

    assert_eq!(
        run.stdout()
            .matches("op read op://vault/everything")
            .count(),
        3,
        "the override should reach all three:
{}",
        run.stdout()
    );
}

/// Everything printed about a database is a direction, never a value. Nothing sloop says
/// out loud could be pasted into a public issue and cause a problem.
#[test]
fn nothing_printed_about_a_registry_could_be_a_password() {
    let sandbox = Sandbox::new("quiet");
    let project = project_with(&sandbox, REGISTRY);

    let run = sandbox.sloop_in(&project, &["db", "list"]);

    // The one thing in the file that a careless reader might take for a secret is the
    // variable's *name*, which is not one.
    run.expect_said("REPORTS_PASSWORD");
    for leak in ["hunter2", "password =", "[databases."] {
        run.expect_silent_about(leak);
    }
}

#[test]
fn a_project_with_no_registry_file_yet_is_not_an_error() {
    let sandbox = Sandbox::new("empty");
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);

    assert!(!Path::new(&project.join(".sloop").join("registry.toml")).exists());
    sandbox
        .sloop_in(&project, &["db", "list"])
        .expect_code(0)
        .expect_said("Nothing is registered");
    // Whichever command is still a stub — see the note in `a_registry_is_read_and_each_
    // route_named`.
    sandbox
        .sloop_in(&project, &["restore"])
        .expect_code(1)
        .expect_said("no databases in it yet");
}

#[test]
fn the_global_store_has_its_own_registry() {
    let sandbox = Sandbox::new("global-registry");
    let project = project_with(&sandbox, REGISTRY);

    let global = sandbox.global_dir();
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(
        global.join("registry.toml"),
        "version = 1\n\n[databases.shared]\nengine = \"postgres\"\nhost = \"central\"\n\
         database = \"app\"\nuser = \"app\"\npassword = \"keyring\"\n",
    )
    .unwrap();

    // Standing in the project, a bare name resolves to the project first — but both
    // registries are live, so a listing shows the global one too rather than pretending
    // it is not there. That is R2's rule, and R7 is the first command that can show it.
    let inside = sandbox.sloop_in(&project, &["db", "list"]);
    inside
        .expect_code(0)
        .expect_said("staging")
        .expect_said("shared")
        .expect_said("this project, then the global store");

    assert!(
        inside.stdout().find("staging") < inside.stdout().find("shared"),
        "the project's entries should come first:
{}",
        inside.stdout()
    );

    // `--global` takes the project out of the picture entirely.
    sandbox
        .sloop_in(&project, &["--global", "db", "list"])
        .expect_code(0)
        .expect_said("shared")
        .expect_said("the OS keyring")
        .expect_silent_about("staging");
}
