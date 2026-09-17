//! The registry file, read under the rules it has to be read under.

use super::Registry;
use crate::engine::Engine;
use crate::secret::Route;

const SAMPLE: &str = r#"
version = 1

[databases.staging]
engine = "postgres"
host = "db.internal"
port = 5433
database = "app"
user = "app_rw"
password = "keyring"

[databases.analytics]
engine = "mysql"
host = "reports.internal"
database = "warehouse"
user = "reader"
password = "${REPORTS_PASSWORD}"
"#;

#[test]
fn a_registry_reads_back_what_it_says() {
    let registry = Registry::parse(SAMPLE).unwrap();

    assert_eq!(registry.len(), 2);
    assert!(!registry.is_empty());

    let staging = registry
        .entries()
        .find(|(name, _)| *name == "staging")
        .map(|(_, database)| database)
        .expect("staging");

    assert_eq!(staging.engine, Engine::Postgres);
    assert_eq!(staging.host, "db.internal");
    assert_eq!(staging.port, 5433);
    assert_eq!(staging.user, "app_rw");
    assert_eq!(staging.password, Route::Keyring);
}

#[test]
fn a_missing_port_becomes_the_engines_own() {
    let registry = Registry::parse(SAMPLE).unwrap();
    let analytics = registry
        .entries()
        .find(|(name, _)| *name == "analytics")
        .map(|(_, database)| database)
        .expect("analytics");

    assert_eq!(analytics.port, 3306);
    assert_eq!(
        analytics.password,
        Route::Environment("REPORTS_PASSWORD".to_owned())
    );
}

/// A file written by a Windows editor, or checked out with `core.autocrlf`, has to load.
/// The failure it otherwise causes points at a character nobody can see.
#[test]
fn a_file_with_windows_line_endings_loads() {
    let crlf = SAMPLE.replace('\n', "\r\n");
    assert!(crlf.contains('\r'), "the fixture has to have them");

    let from_crlf = Registry::parse(&crlf).unwrap();
    assert_eq!(from_crlf, Registry::parse(SAMPLE).unwrap());
}

#[test]
fn a_file_with_a_stray_carriage_return_still_loads() {
    // A lone `\r` — the artefact of a file that has been through two editors.
    let mangled = SAMPLE.replace("password = \"keyring\"", "password = \"keyring\"\r");

    let registry = Registry::parse(&mangled).unwrap();
    assert_eq!(registry.len(), 2);
}

#[test]
fn nothing_in_the_file_is_expanded_or_executed() {
    // Every one of these is a value, and every one of them comes back exactly as written.
    let text = r#"
version = 1

[databases.odd]
engine = "postgres"
host = '$(whoami)`id`;rm -rf /'
database = 'a"b'
user = 'back\slash'
password = "keyring"
"#;

    let registry = Registry::parse(text).unwrap();
    let (_, database) = registry.entries().next().expect("one entry");

    assert_eq!(database.host, "$(whoami)`id`;rm -rf /");
    assert_eq!(database.database, "a\"b");
    assert_eq!(database.user, "back\\slash");
}

#[test]
fn a_password_written_into_the_registry_is_refused_by_name() {
    let text = r#"
version = 1

[databases.oops]
engine = "postgres"
host = "db.internal"
database = "app"
user = "app_rw"
password = "hunter2"
"#;

    let failure = Registry::parse(text).unwrap_err();

    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure.message().contains("oops"),
        "it has to say which one"
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("never written")),
        "and why"
    );
}

#[test]
fn a_typo_in_a_field_name_is_an_error_rather_than_a_silent_default() {
    let text = r#"
version = 1

[databases.staging]
engine = "postgres"
hosts = "db.internal"
database = "app"
user = "app_rw"
password = "keyring"
"#;

    assert!(Registry::parse(text).is_err());
}

#[test]
fn a_registry_from_the_future_says_so_instead_of_guessing() {
    let failure = Registry::parse("version = 99\n").unwrap_err();

    assert!(failure.message().contains("99"));
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("newer"))
    );
}

#[test]
fn an_empty_registry_is_a_registry() {
    let registry = Registry::parse("version = 1\n").unwrap();

    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn a_registry_survives_being_written_out_and_read_back() {
    let original = Registry::parse(SAMPLE).unwrap();
    let written = original.to_toml().unwrap();
    let again = Registry::parse(&written).unwrap();

    assert_eq!(original, again);
    // And nothing that looks like a password went into the text.
    assert!(!written.contains("hunter2"));
    assert!(written.contains("keyring"));
    assert!(written.contains("${REPORTS_PASSWORD}"));
}

#[test]
fn a_credential_key_identifies_the_connection_and_not_the_label() {
    let registry = Registry::parse(SAMPLE).unwrap();
    let (_, staging) = registry
        .entries()
        .find(|(name, _)| *name == "staging")
        .expect("staging");

    assert_eq!(
        staging.credential_key(),
        "postgres://app_rw@db.internal:5433/app"
    );
    // No password in it — it is the same thing a connection log would show.
    assert!(!staging.credential_key().contains("keyring"));
}

#[test]
fn a_file_that_is_missing_is_an_empty_registry_and_not_an_error() {
    let nowhere = std::env::temp_dir().join("sloop-no-such-registry-file.toml");
    assert!(Registry::load(&nowhere).unwrap().is_empty());
}

#[test]
fn a_file_that_will_not_parse_names_itself() {
    let path = std::env::temp_dir().join(format!("sloop-bad-registry-{}.toml", std::process::id()));
    std::fs::write(&path, "this is not toml at all [[[").unwrap();

    let failure = Registry::load(&path).unwrap_err();
    let _ = std::fs::remove_file(&path);

    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure.message().contains("sloop-bad-registry"),
        "the path has to be in the message: {}",
        failure.message()
    );
}

/// **The round trip is what proves `parse` reads everything a registry can hold**, so a
/// field that arrives has to arrive here too — and a `Reach` that quietly did not survive
/// being written and read would make the test above pass while checking nothing about it.
#[test]
fn a_registry_that_goes_over_ssh_survives_the_same_round_trip() {
    let text = r#"
version = 1

[databases.prod]
engine = "postgres"
host = "127.0.0.1"
port = 5432
database = "orders"
user = "app"
password = "keyring"

[databases.prod.ssh]
host = "bastion.internal"
port = 2222
user = "deploy"
identity = "/home/me/.ssh/id_ed25519"
passphrase = "${SSH_KEY_PW}"

[databases.plain]
engine = "mysql"
host = "db.internal"
port = 3306
database = "app"
user = "app"
password = "keyring"
"#;

    let original = Registry::parse(text).unwrap();
    let written = original.to_toml().unwrap();
    let again = Registry::parse(&written).unwrap();
    assert_eq!(original, again);

    let prod = original.get("prod").expect("prod");
    let through = prod.reach.through().expect("it goes over SSH");
    assert_eq!(through.server.host, "bastion.internal");
    assert_eq!(through.server.port, 2222);
    assert_eq!(through.server.user.as_deref(), Some("deploy"));
    assert_eq!(
        through.secret,
        Some(crate::secret::Route::Environment("SSH_KEY_PW".to_owned()))
    );

    // A database that says nothing about SSH has no block written for it, because
    // `Reach::Direct` is an ordinary state and not an empty setting.
    assert!(original.get("plain").unwrap().reach.through().is_none());
    assert!(
        !written.contains("[databases.plain.ssh]"),
        "a direct registration writes no ssh block:\n{written}"
    );
}

/// A passphrase written into the file where a route belongs is refused, exactly as a
/// password in the `password` field is. There is no spelling of a plaintext one that parses.
#[test]
fn a_passphrase_in_the_ssh_block_is_refused_like_any_other_secret() {
    let text = r#"
version = 1

[databases.prod]
engine = "postgres"
host = "127.0.0.1"
port = 5432
database = "orders"
user = "app"
password = "keyring"

[databases.prod.ssh]
host = "bastion.internal"
passphrase = "hunter2"
"#;

    let failure = Registry::parse(text).unwrap_err();
    assert!(
        failure.message().contains("prod ssh"),
        "it has to say which entry: {}",
        failure.message()
    );
}
