//! What sloop prints when it is asked where its own database is — and what it never prints.

use super::Connection;
use crate::secret::Route;

fn a_connection(route: Route) -> Connection {
    Connection {
        host: super::LOOPBACK,
        port: 5433,
        database: "sloop_database".to_owned(),
        role: "sloop_db_admin".to_owned(),
        route,
        answering: true,
    }
}

/// The URL is the one somebody pastes into a client, with no password anywhere in it.
#[test]
fn the_url_names_the_role_and_never_a_password() {
    assert_eq!(
        a_connection(Route::Keyring).url(),
        "postgres://sloop_db_admin@127.0.0.1:5433/sloop_database"
    );
}

/// **The `--json` half of `R19f`'s `Done when`.** A document carries the route — a word like
/// `keyring` — and there is no field in it a password could be put in.
#[test]
fn the_document_carries_a_route_and_never_a_value() {
    let document = a_connection(Route::Environment("SLOOP_PW".to_owned())).document();

    assert_eq!(document["password_route"], "${SLOOP_PW}");
    assert_eq!(
        document["url"],
        "postgres://sloop_db_admin@127.0.0.1:5433/sloop_database"
    );

    let mut keys: Vec<&String> = document
        .as_object()
        .expect("a document is an object")
        .keys()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "answering",
            "database",
            "host",
            "password_route",
            "port",
            "role",
            "url"
        ],
        "a field was added to the document — check it cannot carry a password"
    );
}

/// Every route describes itself without revealing anything, which is what the `Password` row
/// prints. A `command:` route is the one that could carry a secret in its text, and it does
/// not: it is a command line somebody wrote, not its output.
#[test]
fn every_route_can_be_named_on_the_screen() {
    for route in [
        Route::Keyring,
        Route::EncryptedFile,
        Route::Environment("SLOOP_PW".to_owned()),
        Route::Command("op read op://vault/sloop/pw".to_owned()),
    ] {
        let said = a_connection(route.clone()).document()["password_route"].to_string();
        assert!(
            said.contains(&route.as_field()),
            "{route:?} was not written as itself: {said}"
        );
    }
}
