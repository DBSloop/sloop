//! What must never reach a log file.
//!
//! **The negative assertions are the ones that matter.** A redactor that scrubs too much is
//! an annoyance; one that scrubs too little is rule 3 broken, so every test here checks that
//! the secret is *gone* rather than that the line looks right.

use super::secrets;

/// The line has to keep meaning something, or a log is no use for reading.
#[test]
fn an_ordinary_line_is_left_alone() {
    for plain in [
        "backing up orders",
        "postgres://app@db.internal:5432/orders",
        "2 of 2 tables matched",
        "the nearest .sloop at or above the working directory",
        "password from the OS keyring",
    ] {
        assert_eq!(secrets(plain), plain, "nothing here is a secret");
    }
}

/// **The paste people really do.** `db add --url postgres://app:hunter2@db/orders`.
#[test]
fn a_password_inside_a_url_is_taken_out() {
    let said = secrets("registered postgres://app:hunter2@db.internal:5432/orders");

    assert!(!said.contains("hunter2"), "{said}");
    assert!(said.contains("app:***@db.internal:5432/orders"), "{said}");
}

/// A URL without one keeps its shape, port and all — the colon before a port is not this.
#[test]
fn a_url_with_no_password_is_untouched() {
    let plain = "postgres://app@db.internal:5432/orders";
    assert_eq!(secrets(plain), plain);
}

/// `key=value`, however the key is spelled and whatever is around it.
#[test]
fn an_assignment_whose_key_says_what_it_is_loses_its_value() {
    for (line, gone) in [
        ("connecting with password=hunter2 now", "hunter2"),
        ("PGPASSWORD=s3cr3t", "s3cr3t"),
        ("?sslmode=require&password=hunter2", "hunter2"),
        ("api_key=abc123", "abc123"),
        ("API-KEY=abc123", "abc123"),
        ("auth=letmein", "letmein"),
        ("token=xyz;", "xyz"),
    ] {
        let said = secrets(line);
        assert!(!said.contains(gone), "{line} kept its secret: {said}");
        assert!(
            said.contains("***"),
            "{line} lost the value silently: {said}"
        );
    }
}

/// A key that merely contains the word is not one that announces a secret.
#[test]
fn a_key_that_is_not_about_a_secret_keeps_its_value() {
    for plain in ["sslmode=require", "port=5432", "engine=postgres", "keep=7"] {
        assert_eq!(secrets(plain), plain);
    }
}

/// **Somebody else's token, inside their own command.** `--password-command` runs whatever
/// they wrote, and what they wrote is not sloop's to see coming.
#[test]
fn the_value_after_a_password_flag_goes() {
    let said = secrets("running --password-command `op read op://vault/db/pw` now");
    assert!(!said.contains("op://vault/db/pw"), "{said}");

    let bearer = secrets("curl -H 'Authorization: Bearer sk-abc123' https://vault");
    assert!(!bearer.contains("sk-abc123"), "{bearer}");
}

/// A quoted run belongs to the flag the whole way to its closing quote, not one word of it.
#[test]
fn a_quoted_command_is_hidden_the_whole_way_to_its_quote() {
    let said = secrets("--password-command \"vault read -field=pw secret/db\" --json");

    assert!(!said.contains("vault"), "{said}");
    assert!(!said.contains("secret/db"), "{said}");
    // What came after the closing quote is not part of it.
    assert!(said.contains("--json"), "{said}");
}

/// Every flag that carries one is covered, not just the first.
#[test]
fn every_password_flag_is_covered() {
    for flag in [
        "--password-command",
        "--superuser-password-command",
        "--role-password-command",
        "--password-from",
    ] {
        let said = secrets(&format!("with {flag} echo-the-secret and then"));
        assert!(
            !said.contains("echo-the-secret"),
            "{flag} let it through: {said}"
        );
    }
}

/// Two secrets in one line is two secrets gone.
#[test]
fn more_than_one_in_a_line_is_all_of_them() {
    let said = secrets("postgres://a:one@h/d and postgres://b:two@h/e with password=three");

    for gone in ["one", "two", "three"] {
        assert!(!said.contains(gone), "{gone} survived: {said}");
    }
}

/// An empty line, a line of only a marker, and a line ending in a flag are all handled
/// rather than panicking — a redactor that can crash is a redactor that takes a backup with
/// it.
#[test]
fn the_awkward_shapes_do_not_panic() {
    for odd in [
        "",
        "=",
        "://",
        "password=",
        "--password-command",
        "a:@b",
        "Bearer",
    ] {
        let _ = secrets(odd);
    }
}
