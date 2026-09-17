//! What `db` decides before it touches a disk or a server.
//!
//! The parts that need a database are in `engine::cluster_tests`, and the parts that need
//! a whole process are in `tests/db.rs`.

use super::{Draft, draft, route_for};
use crate::cli::{Fields, PasswordSource};
use crate::engine::Engine;
use crate::exit::Exit;
use crate::registry::file::Database;
use crate::secret::{Route, Secret};

fn no_fields() -> Fields {
    Fields {
        engine: None,
        host: None,
        port: None,
        database: None,
        user: None,
    }
}

fn no_password() -> PasswordSource {
    PasswordSource {
        keyring: false,
        encrypted_file: false,
        env: None,
        password_from: None,
        password_stdin: false,
    }
}

fn built(url: Option<&str>, fields: &Fields) -> Database {
    let (draft, _) = draft(None, url, fields).expect("a draft");
    draft.into_database(Route::Keyring).expect("a database")
}

/// R7's "Done when", as a test rather than as an intention: the two ways of registering
/// the same connection have to produce records that cannot be told apart afterwards.
#[test]
fn a_url_and_the_fields_it_stands_for_produce_the_same_record() {
    let by_url = built(Some("postgres://app@db.internal:5432/orders"), &no_fields());
    let by_field = built(
        None,
        &Fields {
            engine: Some("postgres".to_owned()),
            host: Some("db.internal".to_owned()),
            port: Some(5432),
            database: Some("orders".to_owned()),
            user: Some("app".to_owned()),
        },
    );

    assert_eq!(by_url, by_field);
    // And the derived key too, because that is what the password is filed under: two
    // records that looked equal but keyed differently would be a password that vanishes.
    assert_eq!(by_url.credential_key(), by_field.credential_key());
    assert_eq!(
        by_url.credential_key(),
        "postgres://app@db.internal:5432/orders"
    );
}

/// The same, for a URL that leaves the port out. The engine's default has to be filled in
/// at the same point in both paths, or the two records differ by a number nobody typed.
#[test]
fn a_missing_port_becomes_the_engines_default_whichever_way_it_was_given() {
    let by_url = built(Some("mysql://root@127.0.0.1/shop"), &no_fields());
    let by_field = built(
        None,
        &Fields {
            engine: Some("mysql".to_owned()),
            host: Some("127.0.0.1".to_owned()),
            port: None,
            database: Some("shop".to_owned()),
            user: Some("root".to_owned()),
        },
    );

    assert_eq!(by_url, by_field);
    assert_eq!(by_url.port, 3306);
    assert_eq!(
        built(Some("mariadb://r@h/d"), &no_fields()).port,
        3306,
        "MariaDB shares MySQL's port"
    );
    assert_eq!(built(Some("postgres://r@h/d"), &no_fields()).port, 5432);
}

/// A flag beats the URL it came with, so a pasted connection string can be corrected in
/// place rather than retyped — the same rule that makes `-C` outrank `SLOOP_PROJECT`.
#[test]
fn a_flag_outranks_the_url() {
    let record = built(
        Some("postgres://app@db.internal:5432/orders"),
        &Fields {
            host: Some("replica.internal".to_owned()),
            port: Some(6432),
            user: Some("readonly".to_owned()),
            ..no_fields()
        },
    );

    assert_eq!(record.host, "replica.internal");
    assert_eq!(record.port, 6432);
    assert_eq!(record.user, "readonly");
    // Untouched by the flags, so still whatever the URL said.
    assert_eq!(record.database, "orders");
    assert_eq!(record.engine, Engine::Postgres);
}

/// Editing keeps what was not mentioned. An edit that quietly reset the fields nobody
/// named would be a very fast way to lose a connection.
#[test]
fn an_edit_changes_only_what_was_named() {
    let before = built(Some("postgres://app@db.internal:5432/orders"), &no_fields());

    let (changed, _) = draft(
        Some(&before),
        None,
        &Fields {
            user: Some("backup".to_owned()),
            ..no_fields()
        },
    )
    .expect("a draft");
    let after = changed.into_database(Route::Keyring).expect("a database");

    assert_eq!(after.user, "backup");
    assert_eq!(after.host, before.host);
    assert_eq!(after.port, before.port);
    assert_eq!(after.database, before.database);
    assert_eq!(after.engine, before.engine);
}

/// A URL in an edit replaces the whole connection, and a port it does not mention becomes
/// the engine's default rather than the old host's. Carrying an old port onto a new host
/// is a connection nobody asked for.
#[test]
fn a_url_in_an_edit_does_not_leave_the_old_port_behind() {
    let before = built(Some("postgres://app@db.internal:6432/orders"), &no_fields());
    let (changed, _) = draft(
        Some(&before),
        Some("postgres://app@new.host/orders"),
        &no_fields(),
    )
    .expect("a draft");
    let after = changed.into_database(Route::Keyring).expect("a database");

    assert_eq!(after.host, "new.host");
    assert_eq!(after.port, 5432, "the old 6432 must not follow it");
}

/// Every missing field names the flag that supplies it. An error that says only "invalid"
/// is an error somebody has to guess their way out of.
#[test]
fn what_is_missing_is_said_with_the_flag_that_fixes_it() {
    let cases = [
        ("--engine", Draft::default()),
        (
            "--host",
            Draft {
                engine: Some(Engine::Postgres),
                ..Draft::default()
            },
        ),
        (
            "--database",
            Draft {
                engine: Some(Engine::Postgres),
                host: Some("h".to_owned()),
                ..Draft::default()
            },
        ),
        (
            "--user",
            Draft {
                engine: Some(Engine::Postgres),
                host: Some("h".to_owned()),
                database: Some("d".to_owned()),
                ..Draft::default()
            },
        ),
    ];

    for (flag, draft) in cases {
        let failure = draft
            .into_database(Route::Keyring)
            .expect_err("should be incomplete");
        assert_eq!(failure.exit().code(), 2, "{flag}");
        assert!(
            failure.hint_text().is_some_and(|hint| hint.contains(flag)),
            "{flag} is not named: {:?}",
            failure.hint_text()
        );
    }
}

/// The four routes, and the one that is chosen when nobody chooses.
#[test]
fn the_four_routes_and_the_default() {
    let keyring = PasswordSource {
        keyring: true,
        ..no_password()
    };
    let sealed = PasswordSource {
        encrypted_file: true,
        ..no_password()
    };
    let variable = PasswordSource {
        env: Some("PGPASSWORD".to_owned()),
        ..no_password()
    };
    let command = PasswordSource {
        password_from: Some("op read op://vault/db/pw".to_owned()),
        ..no_password()
    };

    assert_eq!(route_for(&keyring, None).unwrap(), Route::Keyring);
    assert_eq!(route_for(&sealed, None).unwrap(), Route::EncryptedFile);
    assert_eq!(
        route_for(&variable, None).unwrap(),
        Route::Environment("PGPASSWORD".to_owned())
    );
    assert_eq!(
        route_for(&command, None).unwrap(),
        Route::Command("op read op://vault/db/pw".to_owned())
    );

    // Nothing chosen on a new registration: the keyring, which needs no setting up.
    assert_eq!(route_for(&no_password(), None).unwrap(), Route::Keyring);

    // Nothing chosen on an edit: whatever the record already said, because an edit that
    // silently moved a password to a different store would be an edit that loses it.
    let existing = Route::Environment("CI_DB_PASSWORD".to_owned());
    assert_eq!(
        route_for(&no_password(), Some(&existing)).unwrap(),
        existing
    );
}

/// `--env` goes through the same reader the registry file uses, so a name that would be
/// refused in the file is refused here too rather than written and refused on the way back.
#[test]
fn a_variable_name_is_checked_the_same_way_the_file_checks_it() {
    for bad in ["not a name", "has$dollar", ""] {
        let source = PasswordSource {
            env: Some(bad.to_owned()),
            ..no_password()
        };
        assert!(route_for(&source, None).is_err(), "{bad:?} was accepted");
    }

    let empty_command = PasswordSource {
        password_from: Some("   ".to_owned()),
        ..no_password()
    };
    assert!(route_for(&empty_command, None).is_err());
}

/// Only two of the four keep anything. The other two are directions, fetched fresh every
/// run — which is what makes them the right answer for a scheduled job.
#[test]
fn only_the_two_stored_routes_have_anything_to_store() {
    assert!(Route::Keyring.is_stored());
    assert!(Route::EncryptedFile.is_stored());
    assert!(!Route::Environment("X".to_owned()).is_stored());
    assert!(!Route::Command("true".to_owned()).is_stored());
}

/// A password in a URL is taken rather than refused — see the module comment — and it
/// never reaches the record, which holds a route and nothing else.
#[test]
fn a_password_in_a_url_is_taken_out_of_it_and_never_written_back() {
    let (draft, password) = draft(
        None,
        Some("postgres://app:s3cr%40t@db.internal/orders"),
        &no_fields(),
    )
    .expect("a draft");

    assert_eq!(password.as_ref().map(Secret::expose), Some("s3cr@t"));

    let record = draft.into_database(Route::Keyring).expect("a database");
    let written = crate::registry::file::Registry::default();
    let mut written = written;
    written.insert("orders".to_owned(), record);
    let toml = written.to_toml().expect("serialising");

    assert!(
        !toml.contains("s3cr@t"),
        "the password reached the file:\n{toml}"
    );
    assert!(
        !toml.contains("s3cr%40t"),
        "the encoded form reached it:\n{toml}"
    );
    assert!(
        toml.contains("keyring"),
        "the route is what is written:\n{toml}"
    );
}

/// What an edit leaves behind, and through which door it has to be cleared.
///
/// **The route is the half that was a bug.** Clearing the old key through the *new* route
/// reads correctly and is wrong: migrating a keyring record to `--encrypted-file` wrote
/// the password into the file and then asked the file to forget a key it had never held,
/// while the keyring kept the password for a connection that no longer existed. Caught by
/// reading the real Credential Manager after a run, which is why it is pinned here.
#[test]
fn what_an_edit_orphans_is_cleared_through_the_route_that_held_it() {
    use super::Retiring;

    let record = |url: &str, route: Route| {
        let (draft, _) = draft(None, Some(url), &no_fields()).expect("a draft");
        draft.into_database(route).expect("a database")
    };
    let here = "postgres://app@db.internal:5432/orders";
    let moved = "postgres://app@db.internal:5432/replica";

    // Nothing changed: nothing to clear.
    assert!(
        Retiring::between(&record(here, Route::Keyring), &record(here, Route::Keyring)).is_none()
    );

    // The connection moved. Same route, old key.
    let by_key = Retiring::between(
        &record(here, Route::Keyring),
        &record(moved, Route::Keyring),
    )
    .expect("the old key is orphaned");
    assert_eq!(by_key.route, Route::Keyring);
    assert_eq!(by_key.key, "postgres://app@db.internal:5432/orders");

    // The route moved and the key did not — the case that was silently leaking. It has to
    // be cleared from the keyring, which is where it is, and not from the file it is
    // going to.
    let by_route = Retiring::between(
        &record(here, Route::Keyring),
        &record(here, Route::EncryptedFile),
    )
    .expect("the keyring entry is orphaned");
    assert_eq!(by_route.route, Route::Keyring);
    assert_eq!(by_route.key, "postgres://app@db.internal:5432/orders");

    // Moving to a route that stores nothing still has to clear what the old one held.
    let to_a_variable = Retiring::between(
        &record(here, Route::Keyring),
        &record(here, Route::Environment("PW".to_owned())),
    )
    .expect("the keyring entry is orphaned");
    assert_eq!(to_a_variable.route, Route::Keyring);

    // And a record that never stored anything leaves nothing behind, however it changes.
    for after in [
        record(moved, Route::Keyring),
        record(here, Route::EncryptedFile),
    ] {
        assert!(
            Retiring::between(&record(here, Route::Environment("PW".to_owned())), &after).is_none(),
            "a ${{VAR}} record has nothing for sloop to clear"
        );
    }
}

// ---------------------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------------------

/// The flags, with nothing set but the two that have no default.
fn creating() -> super::Building<'static> {
    super::Building {
        name: "orders",
        engine: crate::engine::Engine::Postgres,
        host: "127.0.0.1",
        port: None,
        superuser: None,
        superuser_password_stdin: false,
        superuser_password_command: None,
        database: None,
        role: None,
        role_password_stdin: false,
        role_password_command: None,
    }
}

/// **Everything missing, in one message.** Two secrets are needed and only one can come
/// down a pipe, so an unattended run has one shape — and being told half of it twice is
/// exactly the unfriendliness this project is trying to avoid.
#[test]
fn an_unattended_create_names_every_flag_it_needs_at_once() {
    let failure = super::unattended_needs(&creating()).expect_err("there is no terminal here");

    assert_eq!(failure.exit(), Exit::Usage);
    let said = failure.message();
    assert!(said.contains("--role-password-stdin"), "{said}");
    assert!(said.contains("--superuser-password-command"), "{said}");
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("--role-password-stdin")),
        "the hint has to show the shape: {:?}",
        failure.hint_text()
    );
}

#[test]
fn an_unattended_create_with_both_sources_is_accepted() {
    super::unattended_needs(&super::Building {
        superuser_password_command: Some("echo secret"),
        role_password_stdin: true,
        ..creating()
    })
    .expect("both secrets have a source");
}

/// One of the two is not enough, and the message says which one is still missing.
#[test]
fn half_the_flags_is_still_refused() {
    let only_role = super::unattended_needs(&super::Building {
        role_password_stdin: true,
        ..creating()
    })
    .expect_err("the superuser's password has no source");
    assert!(
        only_role.message().contains("--superuser-password-command"),
        "{}",
        only_role.message()
    );
    assert!(
        !only_role.message().contains("--role-password-stdin"),
        "it must not ask for a flag that was given: {}",
        only_role.message()
    );

    let only_admin = super::unattended_needs(&super::Building {
        superuser_password_stdin: true,
        ..creating()
    })
    .expect_err("the new role's password has no source");
    assert!(
        only_admin.message().contains("--role-password-stdin"),
        "{}",
        only_admin.message()
    );
}

/// **Paste-safe on purpose.** The length is the strength; the alphabet is so that the
/// password survives a URL, a YAML file and a shell without one escaping rule between them.
#[test]
fn a_generated_password_is_long_and_needs_no_escaping() {
    let mut seen: Vec<String> = Vec::new();

    for _ in 0..50 {
        let password = crate::secret::generated_password().expect("the OS has randomness");
        assert_eq!(password.chars().count(), 28, "{password}");
        assert!(
            password
                .chars()
                .all(|letter| letter.is_ascii_alphanumeric()),
            "{password} would need escaping somewhere"
        );
        assert!(!seen.contains(&password), "{password} came back twice");
        seen.push(password);
    }
}

/// The engine decides which account administers it, and nothing else does.
#[test]
fn each_engine_has_its_usual_superuser() {
    assert_eq!(
        super::usual_superuser(crate::engine::Engine::Postgres),
        "postgres"
    );
    assert_eq!(super::usual_superuser(crate::engine::Engine::Mysql), "root");
    assert_eq!(
        super::usual_superuser(crate::engine::Engine::Mariadb),
        "root"
    );
}

// ---------------------------------------------------------------------------------------
// the two names on the server
// ---------------------------------------------------------------------------------------

/// **With no terminal, the defaults hold and nothing is asked.** Rule 4: a scheduled run
/// cannot answer a question, so the behaviour it had before the prompt arrived is the
/// behaviour it keeps.
#[test]
fn unattended_the_label_names_the_database_and_the_database_names_its_user() {
    let named = super::name_it("orders", None, None).expect("nothing is asked without a terminal");

    assert_eq!(named.database, "orders");
    assert_eq!(named.role, "orders");
}

/// A flag is an answer, and an answered question is not asked.
#[test]
fn what_was_given_is_what_is_used() {
    let both =
        super::name_it("orders", Some("orders_live"), Some("orders_app")).expect("both were given");
    assert_eq!(both.database, "orders_live");
    assert_eq!(both.role, "orders_app");

    // The user follows the database's name, not the label — which is the whole reason the
    // two are settled in this order.
    let half = super::name_it("orders", Some("orders_live"), None).expect("one was given");
    assert_eq!(half.database, "orders_live");
    assert_eq!(half.role, "orders_live");
}

/// **A name on the server is checked, not just quoted.** Quoting makes anything legal SQL;
/// what is refused here is what would be invisible afterwards.
#[test]
fn a_name_that_could_not_be_typed_back_is_refused() {
    for bad in ["", " orders", "orders ", "or\nders"] {
        let failure =
            super::check_server_name(bad).expect_err("that is not a name anyone could read back");
        assert_eq!(failure.exit(), Exit::Usage);
    }

    // A colon is fine here and not in a label: this one never has to be typed at a command
    // line, where `global:` is the qualifier.
    super::check_server_name("odd:name").expect("a server may call a database that");
    super::check_server_name("orders_live").expect("the ordinary case");
}

/// **What a typed line means.** The prompt needs a terminal; this is the part where being
/// wrong would be expensive, so it is tested on its own.
#[test]
fn an_empty_answer_takes_the_default_and_a_typed_one_replaces_it() {
    let answered = |given: &str| super::answer_or_default(given, "orders");

    // Enter, and Enter after a stray space or two, both mean "the one you showed me".
    assert_eq!(answered("\n").unwrap(), "orders");
    assert_eq!(answered("\r\n").unwrap(), "orders");
    assert_eq!(answered("   \n").unwrap(), "orders");
    assert_eq!(answered("").unwrap(), "orders");

    // A name is taken, and the line ending and any padding around it are not part of it.
    assert_eq!(answered("orders_live\n").unwrap(), "orders_live");
    assert_eq!(answered("  orders_live  \r\n").unwrap(), "orders_live");

    // And a name that could not be read back is refused rather than quoted into existence.
    let failure = answered("or\u{7}ders\n").expect_err("a bell is not a database name");
    assert_eq!(failure.exit(), Exit::Usage);
}

/// **A password manager is as good as a pipe, and only one thing can use standard input.**
/// An unattended run with `--superuser-password-stdin` has to take the new user's password
/// from a command — and that shape is accepted rather than told it is missing a flag.
#[test]
fn a_role_password_command_answers_for_the_pipe() {
    super::unattended_needs(&super::Building {
        superuser_password_stdin: true,
        role_password_command: Some("op read op://vault/app/pw"),
        ..creating()
    })
    .expect("both secrets have a source");

    super::unattended_needs(&super::Building {
        superuser_password_command: Some("op read op://vault/pg/root"),
        role_password_command: Some("op read op://vault/app/pw"),
        ..creating()
    })
    .expect("neither needs standard input at all");

    // And the hint says why only one of them can have the pipe.
    let failure = super::unattended_needs(&creating()).expect_err("there is no terminal here");
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("password-command")),
        "{:?}",
        failure.hint_text()
    );
}

/// The password prompt names the user it is for, by whichever name that user ends up with.
#[test]
fn the_password_is_asked_for_by_the_name_the_user_will_have() {
    assert_eq!(super::role_for(&creating()), "orders");

    assert_eq!(
        super::role_for(&super::Building {
            database: Some("orders_live"),
            ..creating()
        }),
        "orders_live",
        "with no --role the database's name is the user's"
    );

    assert_eq!(
        super::role_for(&super::Building {
            database: Some("orders_live"),
            role: Some("orders_app"),
            ..creating()
        }),
        "orders_app"
    );
}

/// **A new record goes wherever this machine keeps secrets, not to the keyring regardless.**
///
/// It went to the keyring regardless, and that was a defect with a shape: `db create` asks
/// [`super::kept_where`] and the backup key asks `crypt::keep_somewhere`, and both have always
/// read `SLOOP_PASSPHRASE` as *"this machine keeps secrets in the encrypted file"*. `db add`
/// was the one of the three that did not — so a headless Linux box which had said exactly
/// that was still sent to a keyring it does not run, and `db add` failed on the machine the
/// encrypted file exists for. CI was that machine, and this is what that bug looks like from
/// the inside.
///
/// Asserted against `kept_where` rather than against a route by name, because the point is
/// that the three agree — naming one here would just be a fourth opinion.
#[test]
fn a_new_record_is_kept_where_this_machine_keeps_secrets() {
    let chosen = route_for(&no_password(), None).expect("nothing chosen is not an error");

    assert_eq!(chosen, super::kept_where());
}

/// And an existing record keeps the route it already had, whatever this machine prefers —
/// changing a field must never quietly move somebody's password to another store.
#[test]
fn editing_a_record_leaves_its_route_alone() {
    for existing in [
        Route::Keyring,
        Route::EncryptedFile,
        Route::Environment("PGPASSWORD".to_owned()),
        Route::Command("op read op://vault/db/pw".to_owned()),
    ] {
        let kept = route_for(&no_password(), Some(&existing)).expect("nothing chosen");
        assert_eq!(kept, existing);
    }
}
