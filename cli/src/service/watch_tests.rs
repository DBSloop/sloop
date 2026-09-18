//! What can be proved about the attachment list without a PostgreSQL to run it against.
//!
//! The statements, the escaping, and the state the daemon's round keeps between turns. What
//! needs a live server — a row written, read back by a round, and history surviving a detach —
//! is `commands::service_cluster_tests`, which does all of it against a real migrated database.

use super::{
    Attached, Attachment, Detached, Round, attach_sql, attached, detach_sql, service_name,
    standing_sql, unknown,
};
use crate::backup::stamp::Stamp;
use crate::service::unit::SERVICE_NAME;

/// **Rule 3's neighbour, and the one a user can actually reach.** A label is whatever somebody
/// typed at `db add`, and the only thing between it and a statement is [`literal`]. A quote
/// that closed the literal early would be an injection with a registry entry as the payload —
/// so every statement this module builds is checked with the label from hell in it.
#[test]
fn a_quote_in_a_label_cannot_close_the_literal_in_any_of_the_three() {
    let nasty = "'; DROP TABLE monitored_database; --";

    for sql in [
        standing_sql(nasty).unwrap(),
        attach_sql(nasty).unwrap(),
        detach_sql(nasty).unwrap(),
    ] {
        assert!(
            sql.contains("'''; DROP TABLE monitored_database; --'"),
            "the label was not doubled: {sql}"
        );
        assert!(
            !sql.contains("; DROP TABLE monitored_database; --'\n"),
            "the label escaped its literal: {sql}"
        );
        // Every quote in the statement is one of a pair. An odd count means one of them
        // closed something it should not have.
        assert_eq!(
            sql.matches('\'').count() % 2,
            0,
            "an unbalanced quote: {sql}"
        );
    }
}

/// A NUL is refused rather than stored short, which is `literal`'s rule reaching this module.
#[test]
fn a_zero_byte_in_a_label_is_refused_by_every_statement() {
    for built in [
        standing_sql("orders\0evil"),
        attach_sql("orders\0evil"),
        detach_sql("orders\0evil"),
    ] {
        let failure = built.expect_err("a NUL is not storable");
        assert_eq!(failure.exit().code(), 2);
    }
}

/// **The one rule the whole entry rests on**: detaching touches the attachment and nothing
/// else. `bandwidth_day` hangs off the database rather than off the attachment, so a delete
/// that names only `monitored_database` cannot reach the history — and this is what says the
/// statement still names only that.
#[test]
fn detaching_names_the_attachment_and_never_the_history() {
    let sql = detach_sql("orders").unwrap();

    assert!(sql.starts_with("DELETE FROM monitored_database"), "{sql}");
    assert!(
        !sql.contains("bandwidth_day"),
        "detach reaches the history: {sql}"
    );
    assert!(
        !sql.contains("registered_database d\n")
            || !sql.contains("DELETE FROM registered_database"),
        "detach deletes the registration: {sql}"
    );
    // It is scoped to one service and to the global registry, not to a label alone.
    assert!(sql.contains("s.name = 'sloop'"), "{sql}");
    assert!(sql.contains("d.project_id IS NULL"), "{sql}");
}

/// **The global registry, and only it.** A service has no working directory, so a project's
/// databases are invisible to it — every one of the three statements says so in SQL rather
/// than relying on the caller to have resolved the right registry.
#[test]
fn every_statement_looks_in_the_global_registry_only() {
    for sql in [
        standing_sql("orders").unwrap(),
        attach_sql("orders").unwrap(),
        detach_sql("orders").unwrap(),
        attached(),
    ] {
        if sql.contains("registered_database") {
            assert!(
                sql.contains("project_id IS NULL"),
                "a statement reads a project's databases: {sql}"
            );
        }
    }
}

/// The predicate honours `enabled`, which nothing sets false yet. A reader that ignored it
/// would have to be found on the day a pause arrives, and this is the reader.
#[test]
fn the_reader_honours_the_column_a_pause_would_use() {
    assert!(attached().contains("m.enabled"), "{}", attached());
}

/// The name in the predicate comes from the constant the three service managers are given,
/// rather than from a literal that a rename would leave behind.
#[test]
fn the_predicate_names_the_service_this_machine_installs() {
    assert_eq!(service_name(), format!("'{SERVICE_NAME}'"));
    assert!(attached().contains(&service_name()), "{}", attached());

    for sql in [
        standing_sql("orders").unwrap(),
        attach_sql("orders").unwrap(),
        detach_sql("orders").unwrap(),
    ] {
        assert!(sql.contains(&service_name()), "{sql}");
    }
}

/// Attaching makes the row it hangs off, because a fresh machine has no `service` row and a
/// foreign key with nothing on the other end is a failure rather than an attachment.
#[test]
fn attaching_makes_the_service_row_it_hangs_off() {
    let sql = attach_sql("orders").unwrap();

    assert!(sql.contains("INSERT INTO service (name)"), "{sql}");
    assert!(sql.contains("ON CONFLICT (name) DO NOTHING"), "{sql}");
    assert!(
        sql.contains("ON CONFLICT ON CONSTRAINT one_attachment_per_database DO NOTHING"),
        "attaching twice would be an error rather than a no-op: {sql}"
    );

    // **It claims nothing about the machine.** `R24` settled that only the service manager can
    // say whether the service is installed, so the row carries `0004`'s defaults and this
    // statement never writes either column.
    assert!(!sql.contains("installed"), "{sql}");
    assert!(!sql.contains("mechanism"), "{sql}");
}

/// A name that is not in the global registry names the registry it looked in, because the
/// likeliest reason is a database registered into a project.
#[test]
fn an_unknown_name_says_which_registry_was_searched() {
    let failure = unknown("orders");

    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure.message().contains("orders"),
        "{}",
        failure.message()
    );
    assert!(
        failure.message().contains("global registry"),
        "{}",
        failure.message()
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("sloop db list --global")),
        "the hint does not name the command that would have answered it"
    );
}

/// **The states `attach` reports are different sentences, so they are different values.**
/// A provisioning script that runs twice wants the second run to succeed; a person wants to
/// know the second run changed nothing; and `--dry-run` has already said its own piece, so a
/// rehearsal must not be mistaken for either.
#[test]
fn attaching_and_detaching_report_what_they_found_rather_than_only_succeeding() {
    assert_ne!(Attached::Now, Attached::Already);
    assert_ne!(Attached::Now, Attached::Rehearsed);
    assert_ne!(Attached::Already, Attached::Rehearsed);

    let detached = |was_attached, rehearsed| Detached {
        was_attached,
        days_of_history: 0,
        rehearsed,
    };
    assert_ne!(detached(true, false), detached(false, false));
    assert_ne!(detached(true, false), detached(true, true));
}

/// An attachment nothing has read yet has no moment, and `None` is how that is said. A zero
/// would be 1970, which is a lie about a moment that never happened.
#[test]
fn an_attachment_nothing_has_read_has_no_moment() {
    let fresh = Attachment {
        label: String::from("orders"),
        enabled: true,
        attached_at: Stamp::from_unix_seconds(1_789_000_000),
        seen_at: None,
        days_of_history: 0,
    };

    assert!(fresh.seen_at.is_none());
    assert_ne!(fresh.seen_at, Some(Stamp::from_unix_seconds(0)));
}

/// **The journal stays readable, which is what the state in [`Round`] is for.** A round every
/// sixty seconds for a year is half a million turns; the same line half a million times is a
/// log nobody reads, and a list that changed is the one thing worth printing.
#[test]
fn a_round_speaks_when_the_list_changes_and_stays_quiet_when_it_does_not() {
    let mut round = Round::at(std::path::Path::new("nowhere"));

    round.settled(vec![String::from("orders")]);
    assert_eq!(
        round.watching.as_deref(),
        Some(&[String::from("orders")][..])
    );

    // The same list again leaves the remembered one alone — and, more to the point, took the
    // early return rather than printing.
    round.settled(vec![String::from("orders")]);
    assert_eq!(
        round.watching.as_deref(),
        Some(&[String::from("orders")][..])
    );

    // Attaching a second database changes it, which is `R25`'s promise seen from the journal.
    round.settled(vec![String::from("analytics"), String::from("orders")]);
    assert_eq!(
        round.watching.as_deref(),
        Some(&[String::from("analytics"), String::from("orders")][..])
    );
}

/// The same for a round that cannot run: said once, and not again until it changes.
#[test]
fn a_failing_round_complains_once_and_then_stops() {
    let mut round = Round::at(std::path::Path::new("nowhere"));

    round.grumble("the server is not listening");
    assert_eq!(
        round.complaint.as_deref(),
        Some("the server is not listening")
    );

    round.grumble("the server is not listening");
    assert_eq!(
        round.complaint.as_deref(),
        Some("the server is not listening")
    );

    round.grumble("the password was rejected");
    assert_eq!(
        round.complaint.as_deref(),
        Some("the password was rejected")
    );

    // And a round that works again clears it, so the *next* failure is said rather than
    // swallowed as a repeat of one that has been fixed since.
    round.settled(Vec::new());
    assert!(round.complaint.is_none());
}

/// A round that failed read nothing, so the next one that works has something to say even if
/// the list is exactly what it was. Otherwise a database is restarted, the round recovers, and
/// the journal never says it recovered.
#[test]
fn a_recovered_round_says_what_it_is_watching_again() {
    let mut round = Round::at(std::path::Path::new("nowhere"));

    round.settled(vec![String::from("orders")]);
    round.grumble("the server is not listening");
    assert!(round.watching.is_none());
}
