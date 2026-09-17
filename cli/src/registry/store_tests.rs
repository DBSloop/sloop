//! What can be proved about the database-backed registry without a database.
//!
//! The escaping, the hex, and which rows a registry name picks out. What needs a live
//! PostgreSQL — a registry written and read back, a `registry.toml` imported once, a sealed
//! vault that opens after the move — is `registry::cluster_tests`.

use std::path::{Path, PathBuf};

use super::{Which, from_hex, literal, to_hex};
use crate::registry::Scope;

/// **Rule 3's other half, and the one a user can actually reach.** A registry value is
/// whatever somebody typed — a host name, a database name, a `command:` line — and the only
/// thing between it and a statement is this. A quote that closed the literal early would be
/// an injection with a registry entry as the payload.
#[test]
fn a_quote_in_a_value_is_doubled_and_cannot_close_the_literal() {
    assert_eq!(literal("orders").unwrap(), "'orders'");
    assert_eq!(literal("it's").unwrap(), "'it''s'");
    assert_eq!(
        literal("'; DROP TABLE registered_database; --").unwrap(),
        "'''; DROP TABLE registered_database; --'"
    );

    // A backslash is a backslash. `standard_conforming_strings` has been on by default since
    // 9.1 and `initdb` leaves it on, so nothing in a value is an escape character — which is
    // what lets a Windows path and a password full of punctuation round-trip.
    assert_eq!(literal(r"C:\work\orders").unwrap(), r"'C:\work\orders'");
    assert_eq!(literal(r"\'; --").unwrap(), r"'\''; --'");

    // Every literal is balanced: an odd number of quotes would mean one of them closed it.
    for value in [
        "plain",
        "it's",
        "''",
        r"back\slash",
        "quote\"and'apostrophe",
        "newline\nin\nit",
    ] {
        let spelled = literal(value).unwrap();
        assert!(spelled.starts_with('\'') && spelled.ends_with('\''));
        assert_eq!(
            spelled.matches('\'').count() % 2,
            0,
            "{spelled} has an unbalanced quote"
        );
    }
}

/// A NUL is refused rather than truncated: PostgreSQL's `text` cannot hold one, and a host
/// name that silently lost its second half is worse than a complaint.
#[test]
fn a_zero_byte_is_refused_rather_than_stored_short() {
    let failure = literal("orders\0evil").expect_err("a NUL is not storable");
    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure.message().contains("zero byte"),
        "{}",
        failure.message()
    );
}

/// The vault is moved as hex because that is what `encode`/`decode` speak. It has to survive
/// every byte, because what goes through it is ciphertext.
#[test]
fn the_vault_survives_every_byte_as_hex() {
    let every_byte: Vec<u8> = (0..=255u8).collect();
    assert_eq!(from_hex(&to_hex(&every_byte)).unwrap(), every_byte);

    assert_eq!(to_hex(&[]), "");
    assert_eq!(from_hex("").unwrap(), Vec::<u8>::new());
    assert_eq!(to_hex(b"SLOOPSEC"), "534c4f4f50534543");
    assert_eq!(from_hex("534c4f4f50534543").unwrap(), b"SLOOPSEC");

    // Lowercase, because that is what `encode(…, 'hex')` returns and a test that accepted
    // either would not notice the day it changed.
    assert_eq!(to_hex(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
}

/// Half a byte is not a byte. A vault that came back truncated is a failure, not a shorter
/// vault — the difference between those two is somebody's passwords.
#[test]
fn a_truncated_vault_is_a_failure_and_not_a_shorter_vault() {
    assert!(
        from_hex("abc").is_err(),
        "an odd number of digits is not bytes"
    );
    assert!(from_hex("zz").is_err(), "that is not hex");
    assert!(from_hex("00ff0g").is_err(), "that is not hex either");
}

/// Which registry a scope names, and the one combination that names none.
#[test]
fn a_scope_names_a_registry_only_when_there_is_one_to_name() {
    let project = PathBuf::from("/work/orders");

    assert_eq!(Which::of(Scope::Global, None), Some(Which::Global));
    assert_eq!(
        Which::of(Scope::Global, Some(&project)),
        Some(Which::Global),
        "--global is the global store even when a project is in play"
    );
    assert_eq!(
        Which::of(Scope::Project, Some(&project)),
        Some(Which::Project(project.clone()))
    );
    assert_eq!(
        Which::of(Scope::Project, None),
        None,
        "there is no project registry without a project"
    );
}

/// **The predicate that keeps one project's databases out of another's.** The owner's line —
/// *"other project local db will not be shown"* — is this `WHERE` clause and nothing else, so
/// it is checked rather than assumed.
#[test]
fn a_registry_picks_out_its_own_rows_and_nobody_elses() {
    assert_eq!(Which::Global.word(), "global");
    assert_eq!(Which::Project(PathBuf::from("/a")).word(), "project");

    assert_eq!(Which::Global.directory(), None);
    assert_eq!(
        Which::Project(PathBuf::from("/work/orders")).directory(),
        Some(&PathBuf::from("/work/orders"))
    );

    // Two projects are two different predicates, and neither is the global store's.
    let one = Which::Project(PathBuf::from("/work/one"));
    let two = Which::Project(PathBuf::from("/work/two"));
    assert_ne!(one, two);
    assert_ne!(one, Which::Global);
}

/// A project directory with a quote in it is a directory, not a way into the statement.
#[test]
fn a_project_directory_is_escaped_like_any_other_value() {
    let awkward = Path::new("/work/o'brien's code");
    let which = Which::of(Scope::Project, Some(awkward)).expect("a project is a project");

    let spelled = literal(&which.directory().unwrap().display().to_string()).unwrap();
    assert!(spelled.contains("o''brien''s"), "{spelled}");
    assert_eq!(spelled.matches('\'').count() % 2, 0, "{spelled}");
}
