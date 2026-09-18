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

    // **The count used to come from whichever command was still a stub** — `backup` until
    // R9, `restore` until R13, `mirror` until R14, `sync` until R15 — because a stub printed
    // the other half of the same reader. There is no registry-reading stub left, so the
    // count is read where a user reads it: `db list` prints one line per database.
    assert_eq!(
        sandbox
            .sloop_in(&project, &["db", "list"])
            .stdout()
            .lines()
            .filter(|line| line.contains("://"))
            .count(),
        3,
        "three databases were registered and the listing has to show all three"
    );
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

/// **`SLOOP_PASSPHRASE_FILE`, which `R24` wrote into three unit files and nothing read.**
///
/// A Windows service runs as `LocalSystem` and cannot see the keyring, so the encrypted store
/// is the only route it has — and the passphrase reaches it through a file, because a unit
/// file is world-readable on all three platforms and a variable is inherited by every child a
/// process starts. `R25` is the first entry in which the daemon opens the store at all, which
/// is why it is the first that could notice the file was being written and never read.
///
/// **The file is written with CRLF on purpose.** An editor on Windows ends a line that way,
/// and a passphrase with a carriage return on the end is an authentication failure nobody can
/// see — which `docs/OWNER-DECISIONS.md` already records happening once.
#[test]
fn the_passphrase_can_come_from_a_file_and_its_line_ending_is_not_part_of_it() {
    let sandbox = Sandbox::new("passphrase-file");
    if !sandbox.has_a_registry() {
        support::skipping("the passphrase file: this machine has no PostgreSQL server");
        return;
    }

    // One entry, sealed the ordinary way, so there is a vault for the file to open.
    sandbox
        .command(
            sandbox.work(),
            &[
                "db",
                "add",
                "first",
                "--global",
                "--url",
                "postgres://app@db.internal/orders",
                "--encrypted-file",
                "--password-stdin",
            ],
        )
        .env("SLOOP_PASSPHRASE", "a test passphrase")
        .stdin(b"one\n")
        .run()
        .expect_code(0);

    let right = sandbox.home().join("passphrase.txt");
    std::fs::write(&right, "a test passphrase\r\n").expect("the passphrase file is writable");

    // A second entry, with the variable gone and only the file to go on. It has to *open* the
    // vault the first one wrote, so a passphrase read back wrong fails here rather than later.
    sandbox
        .command(
            sandbox.work(),
            &[
                "db",
                "add",
                "second",
                "--global",
                "--url",
                "postgres://app@db.internal/reports",
                "--encrypted-file",
                "--password-stdin",
            ],
        )
        .env_remove("SLOOP_PASSPHRASE")
        .env("SLOOP_PASSPHRASE_FILE", &right.display().to_string())
        .stdin(b"two\n")
        .run()
        .expect_code(0);

    let written = sandbox.registry_text();
    assert!(written.contains("first"), "{written}");
    assert!(written.contains("second"), "{written}");
}

/// A passphrase file that is wrong, missing or empty is a failure that names the variable —
/// never a fall-through to a prompt, because the process this route exists for is a service
/// with no terminal at all.
#[test]
fn a_passphrase_file_that_cannot_be_used_is_refused_rather_than_prompted_around() {
    let sandbox = Sandbox::new("passphrase-file-bad");
    if !sandbox.has_a_registry() {
        support::skipping("the passphrase file: this machine has no PostgreSQL server");
        return;
    }

    sandbox
        .command(
            sandbox.work(),
            &[
                "db",
                "add",
                "first",
                "--global",
                "--url",
                "postgres://app@db.internal/orders",
                "--encrypted-file",
                "--password-stdin",
            ],
        )
        .env("SLOOP_PASSPHRASE", "a test passphrase")
        .stdin(b"one\n")
        .run()
        .expect_code(0);

    let missing = sandbox.home().join("not-there.txt");
    let empty = sandbox.home().join("empty.txt");
    let wrong = sandbox.home().join("wrong.txt");
    std::fs::write(&empty, "\r\n").expect("writable");
    std::fs::write(&wrong, "not the passphrase").expect("writable");

    for (file, expected) in [
        (&missing, "SLOOP_PASSPHRASE_FILE"),
        (&empty, "SLOOP_PASSPHRASE_FILE"),
        (&wrong, "passphrase"),
    ] {
        let run = sandbox
            .command(
                sandbox.work(),
                &[
                    "db",
                    "add",
                    "another",
                    "--global",
                    "--url",
                    "postgres://app@db.internal/reports",
                    "--encrypted-file",
                    "--password-stdin",
                ],
            )
            .env_remove("SLOOP_PASSPHRASE")
            .env("SLOOP_PASSPHRASE_FILE", &file.display().to_string())
            .stdin(b"two\n")
            .run();

        run.expect_not_code(0);
        run.expect_said(expected);
    }
}

/// **The gap `R25` found, closed and driven.** *(owner, 2026-09-19: "go with 1")*
///
/// A service runs as another account and cannot read the keyring the interactive session
/// used, so `sloop service install` seals a copy of sloop's own passwords into the encrypted
/// store under the key file's passphrase. `secret::resolve` reaches that copy only when a
/// passphrase *file* is set, which nothing but a unit file does.
///
/// What is checked here is that seam: a password sealed under a key file's passphrase is
/// readable by a process given only `SLOOP_PASSPHRASE_FILE`, and unreadable without it.
#[test]
fn a_password_sealed_under_a_key_file_is_readable_by_a_process_given_only_that_file() {
    let sandbox = Sandbox::new("service-copy");
    if !sandbox.has_a_registry() {
        support::skipping("the service's copy: this machine has no PostgreSQL server");
        return;
    }

    // Sealed with one passphrase, exactly as `service install` seals its copy.
    sandbox
        .command(
            sandbox.work(),
            &[
                "db",
                "add",
                "copied",
                "--global",
                "--url",
                "postgres://app@db.internal/orders",
                "--encrypted-file",
                "--password-stdin",
            ],
        )
        .env("SLOOP_PASSPHRASE", "what the key file holds")
        .stdin(b"the password\n")
        .run()
        .expect_code(0);

    let key_file = sandbox.home().join("service.key");
    std::fs::write(&key_file, "what the key file holds\r\n").expect("the key file is writable");

    // A process with the file and no variable — which is what the unit gives a daemon.
    sandbox
        .command(sandbox.work(), &["db", "test", "copied"])
        .env_remove("SLOOP_PASSPHRASE")
        .env("SLOOP_PASSPHRASE_FILE", &key_file.display().to_string())
        .run()
        // It cannot reach db.internal, so it fails to *connect* — which is proof the password
        // came out: a passphrase that had not opened the store would have failed before that,
        // with exit 2 and a complaint about the passphrase rather than about the host.
        .expect_code(3);

    // And with neither, it cannot get at the password at all.
    let blind = sandbox
        .command(sandbox.work(), &["db", "test", "copied"])
        .env_remove("SLOOP_PASSPHRASE")
        .run();
    blind.expect_not_code(3);
    blind.expect_said("passphrase");
}
