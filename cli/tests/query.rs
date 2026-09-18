//! `sloop query`, through the real binary, against a real server.
//!
//! **This one needs a server on the other end, unlike most of these files.** What `R19a`
//! promises is that *the engine* refuses a write, not that sloop reads the statement and
//! decides — so the proof has to be a statement arriving at a PostgreSQL and being turned
//! down by it. The sandbox already has one: its own `sloop_database`, on the cluster the
//! test binary built, with the tables `R19c3` migrated into it.
//!
//! Skipped out loud on a machine with no PostgreSQL server, which is the rule every cluster
//! test here follows.

mod support;

use support::Sandbox;

/// A sandbox with one of its own databases registered under `own`, or `None` to skip.
fn ready(label: &str) -> Option<Sandbox> {
    let sandbox = Sandbox::new(label);
    if !sandbox.has_a_registry() {
        eprintln!("skipped: no PostgreSQL server on this machine");
        return None;
    }
    sandbox.register_its_own_database("own");
    Some(sandbox)
}

/// A read arrives and comes back with what is there.
#[test]
fn a_select_comes_back_with_its_columns_and_rows() {
    let Some(sandbox) = ready("query-select") else {
        return;
    };

    let run = sandbox.sloop(&[
        "query",
        "own",
        "--sql",
        "select label, host, port from registered_database order by label",
    ]);
    run.expect_code(0);
    // The sandbox registered `own` a moment ago, so that row is in there.
    run.expect_said("own");
    run.expect_said("Rows:");
}

/// **The `Done when`'s second clause: an `UPDATE` pushed through `--sql` is refused by the
/// server.** Not by a keyword list here — the statement is sent, and PostgreSQL turns it
/// down because the transaction it is in was opened read-only.
#[test]
fn an_update_is_refused_by_the_server() {
    let Some(sandbox) = ready("query-update") else {
        return;
    };

    let run = sandbox.sloop(&[
        "query",
        "own",
        "--sql",
        "update registered_database set label = 'moved'",
    ]);
    run.expect_code(2);
    run.expect_said("read-only transaction");

    // And nothing moved: the row is still called what it was called.
    sandbox
        .sloop(&[
            "query",
            "own",
            "--sql",
            "select label from registered_database order by label",
        ])
        .expect_code(0)
        .expect_said("own");
}

/// **A data-modifying CTE is the case a blacklist misses**, because its first word is
/// `WITH`. The server refuses it for the same reason it refuses the `UPDATE`.
#[test]
fn a_data_modifying_cte_is_refused_too() {
    let Some(sandbox) = ready("query-cte") else {
        return;
    };

    sandbox
        .sloop(&[
            "query",
            "own",
            "--sql",
            "WITH gone AS (DELETE FROM registered_database RETURNING *) SELECT * FROM gone",
        ])
        .expect_code(2)
        .expect_said("read-only transaction");

    sandbox
        .sloop(&[
            "query",
            "own",
            "--sql",
            "select count(*) from registered_database",
        ])
        .expect_code(0)
        .expect_said("1");
}

/// `\!` is psql's language, not SQL, and `--sql` promises SQL.
#[test]
fn a_psql_meta_command_is_not_a_statement() {
    let Some(sandbox) = ready("query-meta") else {
        return;
    };

    sandbox
        .sloop(&["query", "own", "--sql", "\\! echo hello"])
        .expect_code(2)
        .expect_said("psql command rather than SQL");
}

/// **Rule 4.** The builder is nothing but questions, so a run with nobody to ask is refused
/// at the door — with the flag that would have answered it.
#[test]
fn the_builder_refuses_where_there_is_no_terminal() {
    let Some(sandbox) = ready("query-headless") else {
        return;
    };

    sandbox
        .sloop(&["query", "own"])
        .expect_code(2)
        .expect_said("no terminal")
        .expect_said("--sql");
}

/// A `--json` run prints one document, and `NULL` in it is `null` rather than a word.
#[test]
fn the_document_tells_a_null_from_a_string() {
    let Some(sandbox) = ready("query-json") else {
        return;
    };

    let run = sandbox.sloop(&[
        "--json",
        "query",
        "own",
        "--sql",
        "select 'here' as a, null::text as b, '' as c",
    ]);
    run.expect_code(0);

    let document: serde_json::Value =
        serde_json::from_str(&run.stdout()).expect("a --json run prints one document");
    let row = &document["result"]["rows"][0];

    assert_eq!(row[0], "here");
    assert!(row[1].is_null(), "a NULL came back as {:?}", row[1]);
    assert_eq!(row[2], "", "an empty string is not a NULL");
}

/// A name that is not registered is a usage error, before anything is dialled.
#[test]
fn a_database_nobody_registered_is_a_usage_error() {
    let Some(sandbox) = ready("query-unknown") else {
        return;
    };

    sandbox
        .sloop(&["query", "nope", "--sql", "select 1"])
        .expect_code(2);
}

/// Nothing registered at all says so, and says what to do about it.
#[test]
fn an_empty_registry_says_what_to_register() {
    let sandbox = Sandbox::new("query-empty");
    if !sandbox.has_a_registry() {
        eprintln!("skipped: no PostgreSQL server on this machine");
        return;
    }

    sandbox
        .sloop(&["query"])
        .expect_code(2)
        .expect_said("nothing registered")
        .expect_said("sloop db add");
}

/// **The `Done when`'s first clause, arriving at a real server**: a two-table join with a
/// condition on each side, written exactly as the builder writes it.
///
/// `registered_database.engine_id` really points at `engine.id` — it is the foreign key
/// `R19c3` created — so this is the join the builder would offer, run against the schema
/// that has it.
#[test]
fn a_two_table_join_with_a_condition_on_each_side_returns_the_right_rows() {
    let Some(sandbox) = ready("query-join") else {
        return;
    };

    let run = sandbox.sloop(&[
        "query",
        "own",
        "--sql",
        "SELECT \"public\".\"registered_database\".\"label\" AS \"registered_database.label\", \
         \"public\".\"engine\".\"name\" AS \"engine.name\" \
         FROM \"public\".\"registered_database\" \
         LEFT JOIN \"public\".\"engine\" \
         ON \"public\".\"registered_database\".\"engine_id\" = \"public\".\"engine\".\"id\" \
         WHERE \"public\".\"registered_database\".\"label\" = 'own' \
         AND \"public\".\"engine\".\"name\" = 'postgres' \
         LIMIT 201 OFFSET 0",
    ]);

    run.expect_code(0);
    run.expect_said("registered_database.label");
    run.expect_said("engine.name");
    run.expect_said("own");
    run.expect_said("Rows: 1");

    // And the same join with a condition that matches nothing comes back with no rows,
    // rather than with everything — which is what a `WHERE` that was dropped would look like.
    sandbox
        .sloop(&[
            "query",
            "own",
            "--sql",
            "SELECT \"public\".\"registered_database\".\"label\" \
             FROM \"public\".\"registered_database\" \
             LEFT JOIN \"public\".\"engine\" \
             ON \"public\".\"registered_database\".\"engine_id\" = \"public\".\"engine\".\"id\" \
             WHERE \"public\".\"engine\".\"name\" = 'mysql' \
             LIMIT 201 OFFSET 0",
        ])
        .expect_code(0)
        .expect_said("Rows: 0");
}
