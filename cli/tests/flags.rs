//! The global flags, through the real binary — `R16`.
//!
//! **Three claims, and each one is checked rather than described.** `--json` output parses
//! for every command; `--dry-run` writes nothing; colour disappears when standard output is
//! not a terminal. The first two are the ones worth testing hardest, because a `--json` that
//! parses for six commands out of seven is a `--json` that breaks a script at three in the
//! morning, and a `--dry-run` that writes is worse than no `--dry-run` at all.

mod support;

use support::Sandbox;

/// A sandbox with one registered database in it, pointed at a server nothing listens on.
fn with_one(label: &str) -> Sandbox {
    let sandbox = Sandbox::new(label);
    sandbox
        .sloop(&[
            "db",
            "add",
            "orders",
            "--url",
            "postgres://app@127.0.0.1:1/orders",
            "--env",
            "PW",
        ])
        .expect_code(0);
    sandbox
}

/// Parse it, or fail the test saying what would not parse.
fn parsed(output: &str, what: &str) -> serde_json::Value {
    serde_json::from_str(output)
        .unwrap_or_else(|error| panic!("{what} did not print JSON that parses: {error}\n{output}"))
}

/// **The `Done when`: `--json` output parses for every command.** Every one that can be run
/// without a server, in both the shape that works and the shape that fails.
#[test]
fn every_command_prints_json_that_parses() {
    let sandbox = with_one("json-everything");
    let project = sandbox.make_dir("demo");

    let commands: &[&[&str]] = &[
        &["--json", "init"],
        &["--json", "db", "list"],
        &["--json", "db", "test", "orders"],
        &["--json", "db", "rename", "orders", "renamed"],
        &["--json", "db", "rename", "renamed", "orders"],
        &["--json", "backups", "list"],
        &["--json", "backups", "prune", "--keep", "7", "--yes"],
        &["--json", "doctor", "--offline"],
        &["--json", "backup", "orders"],
        &["--json", "restore", "orders", "--confirm", "orders"],
        &["--json", "db", "drop", "orders", "--confirm", "orders"],
        &["--json", "db", "remove", "orders", "--yes"],
        &[
            "--json",
            "db",
            "add",
            "second",
            "--url",
            "postgres://a@h/d",
            "--env",
            "PW",
        ],
        // And a failure, which has to be a document too rather than a bare sentence.
        &["--json", "backup", "no-such-database"],
        &["--json", "mirror", "orders", "--to", "nope"],
    ];

    for command in commands {
        let run = sandbox.sloop_in(&project, command);
        let named = command.join(" ");
        let document = parsed(&run.stdout(), &named);

        assert!(
            document
                .get("ok")
                .is_some_and(serde_json::Value::is_boolean),
            "{named} has no ok: {document}"
        );
        assert!(
            document
                .get("exit")
                .is_some_and(serde_json::Value::is_number),
            "{named} has no exit: {document}"
        );
        assert_eq!(
            document["exit"].as_i64().unwrap_or(-1),
            i64::from(run.code().unwrap_or(-1)),
            "{named}: the document and the process disagree about the exit code"
        );
        assert_eq!(
            document["ok"].as_bool().unwrap_or(true),
            run.code() == Some(0),
            "{named}: ok has to mean exit 0"
        );
    }
}

/// **A `--json` run prints the document and nothing else**, or it is not parseable output.
#[test]
fn json_is_the_whole_of_standard_output() {
    let sandbox = with_one("json-alone");

    let run = sandbox.sloop(&["--json", "db", "list"]);
    let out = run.stdout();

    assert!(
        out.trim_start().starts_with('{'),
        "something was printed before the document:\n{out}"
    );
    assert!(
        out.trim_end().ends_with('}'),
        "something was printed after the document:\n{out}"
    );
    parsed(&out, "db list");
}

/// The document carries what the command actually found, not just an envelope.
#[test]
fn the_document_carries_the_result() {
    let sandbox = with_one("json-result");

    let listing = parsed(
        &sandbox.sloop(&["--json", "db", "list"]).stdout(),
        "db list",
    );
    let databases = listing["result"]["databases"]
        .as_array()
        .expect("a listing is an array");

    assert_eq!(databases.len(), 1);
    assert_eq!(databases[0]["name"], "orders");
    assert_eq!(databases[0]["engine"], "postgres");
    assert_eq!(databases[0]["database"], "orders");
    // A route, never a value — the same rule the human listing follows.
    assert_eq!(databases[0]["password"], "the environment variable PW");
}

/// **A failure is a document too.** A script that has to tell a crash from a refusal by
/// parsing standard error is a script that breaks on the day a message is reworded.
#[test]
fn a_failure_is_a_document_with_the_reason_in_it() {
    let sandbox = with_one("json-failure");

    let run = sandbox.sloop(&["--json", "backup", "no-such-database"]);
    run.expect_code(2);

    let document = parsed(&run.stdout(), "a failing backup");
    assert_eq!(document["ok"], false);
    assert_eq!(document["exit"], 2);
    assert!(
        document["error"]
            .as_str()
            .is_some_and(|said| said.contains("no-such-database")),
        "{document}"
    );
    assert!(document.get("hint").is_some(), "{document}");
}

/// **The `Done when`: `--dry-run` provably writes nothing.** Checked on the registry itself,
/// which is the thing that outlives the run — rows since `R19c4`, a file before it.
#[test]
fn a_dry_run_changes_no_file() {
    let sandbox = with_one("dry-run");
    let before = sandbox.registry_text();
    assert!(!before.is_empty(), "there should be something to change");

    for command in [
        vec!["--dry-run", "db", "rename", "orders", "something-else"],
        vec!["--dry-run", "db", "remove", "orders", "--yes"],
        vec![
            "--dry-run",
            "db",
            "add",
            "second",
            "--url",
            "postgres://a@h/d",
            "--env",
            "PW",
        ],
        vec![
            "--dry-run",
            "db",
            "edit",
            "orders",
            "--host",
            "elsewhere.internal",
        ],
    ] {
        let run = sandbox.sloop(&command);
        run.expect_code(0).expect_said("dry run");

        assert_eq!(
            sandbox.registry_text(),
            before,
            "`sloop {}` changed the registry",
            command.join(" ")
        );
    }
}

/// A rehearsal does the reading and the checking first, so it still refuses what a real run
/// would refuse — a dry run that "succeeds" on a database that does not exist is a rehearsal
/// of nothing.
#[test]
fn a_dry_run_still_refuses_what_a_real_run_refuses() {
    let sandbox = with_one("dry-run-refuses");

    sandbox
        .sloop(&["--dry-run", "db", "rename", "no-such-database", "other"])
        .expect_code(2)
        .expect_said("no database is registered");

    sandbox
        .sloop(&["--dry-run", "db", "drop", "orders"])
        .expect_code(2)
        .expect_said("needs its name typed");
}

/// **The `Done when`: colour disappears when standard output is piped.** Every run in this
/// file is piped, so the assertion is that no escape ever reaches it — with the flag and
/// without it.
#[test]
fn nothing_coloured_reaches_a_pipe() {
    let sandbox = with_one("no-colour");

    for command in [
        vec!["db", "list"],
        vec!["--no-color", "db", "list"],
        vec!["db", "list"],
    ] {
        let run = sandbox.sloop(&command);
        assert!(
            !run.stdout().contains('\u{1b}'),
            "`sloop {}` put an escape in a pipe",
            command.join(" ")
        );
    }

    // And `NO_COLOR` says the same thing from the environment.
    let said = sandbox
        .command(sandbox.work(), &["db", "list"])
        .env("NO_COLOR", "1")
        .run();
    assert!(!said.stdout().contains('\u{1b}'));
}

/// `--quiet` silences the commentary and nothing else.
#[test]
fn quiet_keeps_the_failures() {
    let sandbox = with_one("quiet");

    let listing = sandbox.sloop(&["--quiet", "db", "list"]);
    listing.expect_code(0);
    assert!(
        listing.stdout().trim().is_empty(),
        "--quiet still said: {}",
        listing.stdout()
    );

    // The failure is the thing a scheduled run must never lose.
    sandbox
        .sloop(&["--quiet", "backup", "no-such-database"])
        .expect_code(2)
        .expect_said("no database is registered");
}

/// **`--log-file` gets the lines, without the colour and without the secrets.**
#[test]
fn a_log_file_keeps_the_lines_and_not_the_credentials() {
    let sandbox = Sandbox::new("log-file");
    let log = sandbox.work().join("sloop.log");
    let path = log.display().to_string();

    // A URL with a password in it is the paste people really do.
    sandbox
        .sloop(&[
            "--log-file",
            &path,
            "db",
            "add",
            "orders",
            "--url",
            "postgres://app:hunter2@127.0.0.1:1/orders",
        ])
        .expect_code(0);

    sandbox
        .sloop(&["--log-file", &path, "db", "list"])
        .expect_code(0);

    let written = std::fs::read_to_string(&log).expect("a log file");

    assert!(
        written.contains("orders"),
        "the log says nothing:\n{written}"
    );
    assert!(
        !written.contains('\u{1b}'),
        "the log kept its colour:\n{written}"
    );
    assert!(
        !written.contains("hunter2"),
        "**the log kept a password**:\n{written}"
    );
}

/// A `--log-file` that cannot be opened is a usage error before the command runs, not a
/// surprise halfway through one.
#[test]
fn a_log_file_that_cannot_be_opened_is_refused_up_front() {
    let sandbox = with_one("log-file-bad");
    let nowhere = sandbox
        .work()
        .join("no-such-directory")
        .join("sloop.log")
        .display()
        .to_string();

    sandbox
        .sloop(&["--log-file", &nowhere, "db", "list"])
        .expect_code(2)
        .expect_said("to log to");
}

/// `--json` and `--quiet` are two answers to one question.
#[test]
fn json_and_quiet_are_not_both() {
    let sandbox = with_one("json-quiet");

    sandbox
        .sloop(&["--json", "--quiet", "db", "list"])
        .expect_code(2)
        .expect_said("cannot be used with");
}

/// **`doctor --json` carries the whole report, not just a verdict.** A script watching a
/// fleet needs to know *which* engine is short of a tool and *which* role cannot dump, not
/// that something somewhere is wrong.
#[test]
fn doctor_reports_its_whole_findings_as_json() {
    let sandbox = with_one("json-doctor");

    let document = parsed(
        &sandbox.sloop(&["--json", "doctor", "--offline"]).stdout(),
        "doctor",
    );
    let result = &document["result"];

    // Every engine sloop knows, each with the three facts the printed report leads with.
    let engines = result["tools"]["engines"]
        .as_array()
        .expect("an engine per adapter");
    assert_eq!(engines.len(), 3, "{result}");
    for engine in engines {
        assert!(engine["engine"].is_string(), "{engine}");
        assert!(engine["ready"].is_boolean(), "{engine}");
        assert!(engine["missing"].is_array(), "{engine}");
        assert!(engine["using"].is_array(), "{engine}");
    }

    assert!(result["tools"]["fetched_into"].is_string(), "{result}");
    assert!(result["every_engine_ready"].is_boolean(), "{result}");
    assert!(result["every_role_can_dump"].is_boolean(), "{result}");
    // `--offline` asks no server, so there is nothing checked to report.
    assert_eq!(
        result["roles"].as_array().map(Vec::len),
        Some(0),
        "{result}"
    );
}

/// A role that could not be checked is in the document too, saying so — a run that silently
/// left it out would read as a clean bill of health.
#[test]
fn a_role_that_cannot_be_reached_is_reported_rather_than_dropped() {
    let sandbox = with_one("json-doctor-unreachable");

    let document = parsed(&sandbox.sloop(&["--json", "doctor"]).stdout(), "doctor");
    let roles = document["result"]["roles"]
        .as_array()
        .expect("one registered database");

    assert_eq!(roles.len(), 1, "{document}");
    assert_eq!(roles[0]["name"], "orders");
    assert_eq!(roles[0]["checked"], false);
    assert!(roles[0]["error"].is_string(), "{document}");
}
