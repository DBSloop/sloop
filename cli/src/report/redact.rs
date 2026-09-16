//! Scrubbing a line before it reaches a log file.
//!
//! **Rule 3: nothing is written to a log that could not be pasted into a public issue.** The
//! first half of keeping that true is structural — the one line carrying a real password goes
//! through [`super::secret`], which has no logging in it at all. This is the second half, for
//! the credentials that arrive inside something else.
//!
//! Three shapes, and they are the three that actually occur here:
//!
//! - **A URL with a password in it.** `db add --url postgres://app:hunter2@db/orders` is a
//!   paste people really do; sloop takes it, complains, and files it under a real route — and
//!   the connection string it complains *with* is a string that has the password in it.
//! - **A `key=value` pair whose key says what it is.** `password=`, `token=`, `api_key=` and
//!   their spellings, which turn up inside connection strings and inside the command a
//!   `--password-command` runs.
//! - **A word after something that announces a secret is coming.** `Bearer abc123`, and the
//!   value after any of the password flags.
//!
//! **What this is not is a guarantee.** A redactor is a net under a design, never the design:
//! what keeps passwords out of sloop's logs is that sloop does not print them. This catches
//! the case where somebody's own `--password-command` carries a token in its arguments, which
//! is theirs to write and not sloop's to see coming.

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;

/// What replaces anything this finds.
const HIDDEN: &str = "***";

/// The keys that announce a secret when one is assigned to them.
const TELLING: [&str; 10] = [
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "api-key",
    "auth",
    "credential",
];

/// The flags whose *next word* is a secret, or is a command that may hold one.
const CARRYING: [&str; 5] = [
    "--password-command",
    "--superuser-password-command",
    "--role-password-command",
    "--password-from",
    "--password",
];

/// Scrub a line of anything that reads like a credential.
#[must_use]
pub fn secrets(line: &str) -> String {
    let scrubbed = a_password_in_a_url(line);
    let scrubbed = assignments(&scrubbed);
    after_a_flag(&scrubbed)
}

/// `scheme://user:password@host` becomes `scheme://user:***@host`.
///
/// **The colon inside the authority is the whole signal**, and it cannot be confused with the
/// one before a port: the port's colon comes after the `@`, and this only ever looks between
/// `//` and the first `@` of a run.
fn a_password_in_a_url(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while let Some(at) = rest.find("://") {
        let (before, from_scheme) = rest.split_at(at + 3);
        out.push_str(before);

        // The authority ends at the first character that cannot be in one. An `@` after a
        // colon inside it is a password; anything else is not.
        let authority_ends = from_scheme
            .find(|letter: char| letter.is_whitespace() || matches!(letter, '/' | '"' | '`'))
            .unwrap_or(from_scheme.len());
        let (authority, after) = from_scheme.split_at(authority_ends);

        match authority.find('@').and_then(|at| {
            let (userinfo, host) = authority.split_at(at);
            userinfo.find(':').map(|colon| (&userinfo[..colon], host))
        }) {
            Some((user, host)) => {
                out.push_str(user);
                out.push(':');
                out.push_str(HIDDEN);
                out.push_str(host);
            }
            None => out.push_str(authority),
        }
        rest = after;
    }

    out.push_str(rest);
    out
}

/// `password=hunter2` becomes `password=***`, however the key was spelled.
fn assignments(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while let Some(equals) = rest.find('=') {
        let (before, from_equals) = rest.split_at(equals);

        // The key is the run of name-ish characters immediately before the `=`.
        let key_starts = before
            .rfind(|letter: char| !(letter.is_alphanumeric() || matches!(letter, '_' | '-')))
            .map_or(0, |at| {
                at + before[at..].chars().next().map_or(1, char::len_utf8)
            });
        let key = &before[key_starts..];

        out.push_str(before);
        out.push('=');

        if telling(key) {
            let value = &from_equals[1..];
            let value_ends = value
                .find(|letter: char| {
                    letter.is_whitespace() || matches!(letter, '&' | '"' | '\'' | '`' | ';')
                })
                .unwrap_or(value.len());
            if value_ends > 0 {
                out.push_str(HIDDEN);
            }
            rest = &value[value_ends..];
        } else {
            rest = &from_equals[1..];
        }
    }

    out.push_str(rest);
    out
}

/// The word after `Bearer`, or after any flag that carries a secret, goes.
fn after_a_flag(line: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut hide_the_rest = false;

    for word in line.split_inclusive(char::is_whitespace) {
        let bare = word.trim();
        let trimmed = bare.trim_matches(|letter| matches!(letter, '`' | '"' | '\'' | ',' | '.'));

        if hide_the_rest {
            // A quoted run belongs to the flag the whole way to its closing quote; an
            // unquoted one is a single word.
            hide_the_rest = !bare.ends_with(['`', '"', '\'']);
            if !bare.is_empty() {
                words.push(format!("{HIDDEN}{}", trailing(word)));
                continue;
            }
        }

        if trimmed.eq_ignore_ascii_case("bearer") || CARRYING.contains(&trimmed) {
            hide_the_rest = true;
        }
        words.push(word.to_owned());
    }

    // Two `***` in a row say nothing the first did not.
    let mut out = String::with_capacity(line.len());
    let mut last_was_hidden = false;
    for word in words {
        let hidden = word.trim_end().ends_with(HIDDEN) || word.trim_end() == HIDDEN;
        if !(hidden && last_was_hidden) {
            out.push_str(&word);
        }
        last_was_hidden = hidden;
    }
    out
}

/// Whatever whitespace a word ended with, so a line keeps its shape.
fn trailing(word: &str) -> &str {
    &word[word.trim_end().len()..]
}

/// Does this key announce that a secret follows it?
fn telling(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    TELLING.iter().any(|known| lower.ends_with(known))
}
