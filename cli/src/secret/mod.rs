//! Passwords: where they come from, and how they are handled once they are here.
//!
//! Four routes and no fifth. A password is never written in a registry file, never passed
//! in `argv`, never printed and never logged, so the only thing a registry holds is a
//! *route* — a sentence saying where to go and ask.
//!
//! ```text
//! keyring                  the OS keyring, and the default
//! encrypted-file           Argon2id + XChaCha20-Poly1305, for a Linux box with no keyring
//! ${VAR}                   an environment variable, for automation
//! command:<command line>   run something and read its output, for a team password manager
//! ```
//!
//! Nothing here has a caller yet: R4 opens the first connection and R7 stores the first
//! password. Where a password comes from is R3's decision and not the connection code's,
//! so it is settled, built and tested now — which is why the whole module is allowed to
//! sit unused rather than each item carrying its own excuse.
#![allow(dead_code)]

pub mod command;
pub mod os_keyring;
pub mod sealed;

#[cfg(test)]
mod tests;

use std::fmt;
use std::path::Path;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::failure::{Failure, Outcome};

/// A password, in memory, wiped when it goes out of scope.
///
/// There is no `Display` and `Debug` says nothing, on purpose. A password that can be
/// formatted is a password that reaches a log eventually, and reading one has to be an
/// act you can grep for — which is what [`Secret::expose`] is named for.
#[derive(Clone, ZeroizeOnDrop)]
pub struct Secret(String);

impl Secret {
    /// Take ownership of a password.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Read it. Every call site of this is a place a password could escape, so it is
    /// spelled loudly enough to find with one search.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Is it empty? The one question worth answering without exposing anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// What is worth telling the user about this value, without saying what it is.
    ///
    /// Trailing whitespace is almost always an accident — a newline a password manager
    /// added, a space a copy-and-paste brought along — and the authentication failure it
    /// causes says nothing about whitespace. It is never stripped, because a password is
    /// allowed to end in a space and guessing would be worse.
    #[must_use]
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();

        if self.0.is_empty() {
            notes.push("the password came back empty".to_owned());
        } else if self.0.trim_end() != self.0 {
            notes.push(
                "the password ends in whitespace, which is kept as-is — if that was not \
                 deliberate it will look like a wrong password"
                    .to_owned(),
            );
        }

        notes
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Where to go and ask for a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The OS keyring: Credential Manager, Keychain, Secret Service.
    Keyring,
    /// The Argon2id-encrypted file, for a Linux machine with no keyring running.
    EncryptedFile,
    /// An environment variable, read literally.
    Environment(String),
    /// A command to run, whose output is the password.
    Command(String),
}

impl Route {
    /// Read a route out of a registry field.
    ///
    /// Anything unrecognised is refused rather than treated as a password. That refusal is
    /// the point: it is what makes it impossible to put a plaintext password in a registry
    /// file by accident, because there is no spelling of one that would be accepted.
    pub fn parse(field: &str) -> Outcome<Self> {
        let refuse = || {
            Failure::usage(format!("{field} is not a password route")).hint(
                "a password is never written in a registry. Use keyring, encrypted-file, \
                 ${SOME_VARIABLE}, or command:<command line>",
            )
        };

        match field {
            "keyring" => return Ok(Self::Keyring),
            "encrypted-file" => return Ok(Self::EncryptedFile),
            _ => {}
        }

        if let Some(rest) = field.strip_prefix("command:") {
            let command = rest.trim();
            if command.is_empty() {
                return Err(Failure::usage("command: needs a command after it")
                    .hint("for example command:op read op://vault/db/password"));
            }
            return Ok(Self::Command(command.to_owned()));
        }

        if let Some(inner) = field.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
            if inner.is_empty() || inner.contains(|c: char| c.is_whitespace() || c == '$') {
                return Err(Failure::usage(format!("{field} is not a variable name"))
                    .hint("it should look like ${PGPASSWORD}"));
            }
            return Ok(Self::Environment(inner.to_owned()));
        }

        Err(refuse())
    }

    /// How this route is written in a registry file. The inverse of [`Route::parse`].
    #[must_use]
    pub fn as_field(&self) -> String {
        match self {
            Self::Keyring => "keyring".to_owned(),
            Self::EncryptedFile => "encrypted-file".to_owned(),
            Self::Environment(name) => format!("${{{name}}}"),
            Self::Command(command) => format!("command:{command}"),
        }
    }

    /// The route actually used this run, once `--password-command` has had its say.
    ///
    /// The flag outranks the file because it is the more immediate instruction, the same
    /// way `-C` outranks `SLOOP_PROJECT`.
    #[must_use]
    pub fn overridden_by(&self, flag: Option<&str>) -> Self {
        match flag.filter(|command| !command.trim().is_empty()) {
            Some(command) => Self::Command(command.trim().to_owned()),
            None => self.clone(),
        }
    }

    /// A short phrase for a message, naming the route without revealing anything. None of
    /// these can contain a password: they are all directions, not values.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Keyring => "the OS keyring".to_owned(),
            Self::EncryptedFile => "the encrypted file".to_owned(),
            Self::Environment(name) => format!("the environment variable {name}"),
            Self::Command(command) => format!("the command `{command}`"),
        }
    }
}

impl fmt::Display for Route {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_field())
    }
}

/// Everything a route needs to find the password it points at.
pub struct Lookup<'a> {
    /// What this database is called, in a form that survives being a keyring account name
    /// and a key inside the encrypted file.
    pub key: &'a str,
    /// Where the encrypted file lives.
    pub sealed_file: &'a Path,
}

/// A password, with anything the user ought to know about how it arrived.
///
/// Safe to print: the password inside redacts itself, and the notes never quote it.
#[derive(Debug)]
pub struct Resolved {
    /// The password.
    pub secret: Secret,
    /// Notes worth printing. None of them contains the password.
    pub notes: Vec<String>,
}

/// Go and get the password.
pub fn resolve(route: &Route, lookup: &Lookup<'_>) -> Outcome<Resolved> {
    let secret = match route {
        Route::Keyring => os_keyring::get(lookup.key)?,
        Route::EncryptedFile => sealed::get(lookup.sealed_file, lookup.key)?,
        Route::Environment(name) => from_environment(name)?,
        Route::Command(command) => command::run(command)?,
    };

    let notes = secret.notes();
    Ok(Resolved { secret, notes })
}

/// Read a variable, literally.
///
/// Nothing in the value is interpreted: no second round of interpolation, no shell, no
/// unescaping. A password is a string of bytes the user chose, and every byte of it —
/// backslashes, dollars, quotes, semicolons — has to arrive unchanged.
fn from_environment(name: &str) -> Outcome<Secret> {
    let mut value = std::env::var(name).map_err(|_| {
        Failure::new(
            crate::exit::Exit::Usage,
            format!("{name} is not set, and the registry says the password is in it"),
        )
        .hint(format!(
            "export {name} before running this, or change the password route"
        ))
    })?;

    let secret = Secret::new(value.clone());
    value.zeroize();
    Ok(secret)
}
