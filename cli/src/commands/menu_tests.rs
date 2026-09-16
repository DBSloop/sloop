//! Turning answers back into the arguments a flag would have carried.
//!
//! **The half that can be checked without a server.** What each command then does is its
//! own module's tests and the cluster tests; what is here is the join between the two
//! surfaces, and the join is where a menu quietly stops meaning what it says.

use super::{fields, password_source};
use crate::ui::flow::{Answers, field};

fn answered(pairs: &[(&'static str, &str)]) -> Answers {
    let mut answers = Answers::default();
    for (field, value) in pairs {
        answers.put(field, *value);
    }
    answers
}

/// The four routes, and each one names only its own extra.
#[test]
fn each_password_route_becomes_the_flag_that_names_it() {
    let keyring = password_source(&answered(&[(field::ROUTE, "keyring")]));
    assert!(keyring.keyring);
    assert!(!keyring.encrypted_file);
    assert_eq!(keyring.env, None);
    assert_eq!(keyring.password_from, None);

    let file = password_source(&answered(&[(field::ROUTE, "file")]));
    assert!(file.encrypted_file);
    assert!(!file.keyring);

    let variable = password_source(&answered(&[
        (field::ROUTE, "env"),
        (field::ENV, "CI_DB_PASSWORD"),
    ]));
    assert_eq!(variable.env.as_deref(), Some("CI_DB_PASSWORD"));
    assert!(!variable.keyring);

    let command = password_source(&answered(&[
        (field::ROUTE, "command"),
        (field::FROM_COMMAND, "op read op://vault/db/pw"),
    ]));
    assert_eq!(
        command.password_from.as_deref(),
        Some("op read op://vault/db/pw")
    );
}

/// **An answer left over from a branch nobody took must not leak into the flags.**
///
/// Going back and changing the route from `env` to `keyring` drops the variable, because
/// `← Back` drops answers. This is the belt to that braces: even if one were left behind,
/// the route decides which flag is set.
#[test]
fn an_answer_from_a_route_that_was_not_chosen_is_ignored() {
    let stale = password_source(&answered(&[
        (field::ROUTE, "keyring"),
        (field::ENV, "LEFT_OVER"),
        (field::FROM_COMMAND, "also left over"),
    ]));

    assert!(stale.keyring);
    assert_eq!(stale.env, None);
    assert_eq!(stale.password_from, None);
}

/// **A password never arrives through standard input from a menu.** That flag exists for a
/// run with no terminal, and a menu is the opposite of one: the command asks, hidden, on
/// the terminal it has just been handed.
#[test]
fn a_menu_never_says_the_password_is_on_standard_input() {
    for route in ["keyring", "file", "env", "command"] {
        let source = password_source(&answered(&[(field::ROUTE, route)]));
        assert!(
            !source.password_stdin,
            "{route} said the password was piped in"
        );
    }
}

/// Every field that was answered becomes a flag; every one left blank stays `None`, which
/// is what leaves a registered database's detail exactly as it was.
#[test]
fn a_blank_field_is_left_alone_rather_than_set_to_nothing() {
    let given = fields(
        &answered(&[
            (field::ENGINE, "postgres"),
            (field::HOST, "db.internal"),
            (field::PORT, "  "),
            (field::DATABASE, "orders"),
            (field::USER, ""),
        ]),
        true,
    );

    assert_eq!(given.engine.as_deref(), Some("postgres"));
    assert_eq!(given.host.as_deref(), Some("db.internal"));
    assert_eq!(given.port, None, "a blank port is the engine's own default");
    assert_eq!(given.database.as_deref(), Some("orders"));
    assert_eq!(given.user, None, "a blank user changed the record");
}

/// A port that is not a number is left alone rather than guessed at. The command then uses
/// the engine's own default, which is the same thing leaving the flag off does.
#[test]
fn a_port_that_is_not_a_number_is_not_a_port() {
    let given = fields(&answered(&[(field::PORT, "five thousand")]), true);
    assert_eq!(given.port, None);
}

/// **`db edit` changes one detail.** When the answers were about the password, every
/// connection field is `None` — otherwise editing a password would blank the host.
#[test]
fn editing_the_password_does_not_touch_the_connection() {
    let answers = answered(&[
        (field::NAME, "orders"),
        (field::DETAIL, "password"),
        (field::ROUTE, "keyring"),
    ]);

    let untouched = fields(&answers, false);
    assert_eq!(untouched.engine, None);
    assert_eq!(untouched.host, None);
    assert_eq!(untouched.port, None);
    assert_eq!(untouched.database, None);
    assert_eq!(untouched.user, None);
}

/// And registering by URL sends no fields either, so the URL is the whole of it.
#[test]
fn registering_by_url_sends_no_fields_alongside_it() {
    let by_url = fields(
        &answered(&[(field::HOW, "url"), (field::HOST, "leftover")]),
        false,
    );
    assert_eq!(by_url.host, None);
}
