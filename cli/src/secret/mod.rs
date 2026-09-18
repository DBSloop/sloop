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

    /// Does sloop have to keep the password itself for this route?
    ///
    /// The split that decides what `db add` asks for. Two of the four routes are places
    /// sloop puts a value; the other two are *directions* — an environment variable and a
    /// command — where the answer is fetched fresh on every run and there is nothing here
    /// to store, rotate or lose.
    #[must_use]
    pub const fn is_stored(&self) -> bool {
        matches!(self, Self::Keyring | Self::EncryptedFile)
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
    /// Where the encrypted store lives — a row in sloop's own database for every registry,
    /// a file for the two passwords that open that database.
    pub vault: &'a sealed::Vault<'a>,
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
        Route::EncryptedFile => sealed::get(lookup.vault, lookup.key)?,
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

/// Where this machine keeps a secret sloop generated, in the order to try.
///
/// **`SLOOP_PASSPHRASE` decides, where it is set.** That variable exists for one purpose —
/// the Argon2id file — so a machine that has it has already said where it keeps secrets, and
/// putting one in a keyring it deliberately does not use would be ignoring the answer.
/// Everywhere else the keyring comes first, because it needs no configuration and is there
/// on every desktop.
#[must_use]
pub fn preferred_routes() -> [Route; 2] {
    if sealed::is_the_machines_choice() {
        [Route::EncryptedFile, Route::Keyring]
    } else {
        [Route::Keyring, Route::EncryptedFile]
    }
}

/// Put a secret sloop generated wherever this machine can keep one, and say where that was.
///
/// The two routes that *store* something, never the two that fetch what somebody else keeps:
/// `${VAR}` and `command:` point at a secret that already exists somewhere, and this is for
/// one that has just been invented.
pub fn keep_somewhere(
    key: impl AsRef<str>,
    secret: &Secret,
    vault: &sealed::Vault<'_>,
) -> Outcome<Route> {
    let key = key.as_ref();
    let mut first = None;

    for route in preferred_routes() {
        let kept = match route {
            Route::Keyring => os_keyring::set(key, secret),
            Route::EncryptedFile => sealed::put(vault, key, secret),
            // Unreachable by construction — `preferred_routes` returns only these two — and
            // written as a refusal rather than a panic, because rule 8 says a user can never
            // reach an `unwrap`.
            ref other => Err(Failure::usage(format!(
                "a password sloop generated cannot live in {} — that route fetches a secret \
                 something else keeps",
                other.describe()
            ))),
        };

        match kept {
            Ok(()) => return Ok(route),
            Err(failure) => first = first.or(Some(failure)),
        }
    }

    Err(first.unwrap_or_else(|| {
        Failure::usage("this machine has nowhere to keep a password sloop generated")
    }))
}

/// A password for something that is about to exist.
///
/// **Letters and digits only, and that is a decision rather than a shortcut.** 28 of them is
/// about 166 bits, which is past anything that matters; what the character set buys is a
/// password that pastes into a URL, a YAML file, a `docker-compose` environment and a shell
/// command without one escaping rule between them. `R3` proved sloop itself round-trips a
/// password full of punctuation — this is about every other tool it will be pasted into.
pub fn generated_password() -> Outcome<String> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    const LENGTH: usize = 28;

    let mut bytes = [0_u8; LENGTH];
    getrandom::fill(&mut bytes).map_err(|error| {
        Failure::new(
            crate::exit::Exit::Failure,
            format!("the operating system would not provide random bytes: {error}"),
        )
    })?;

    // **Rejection sampling, not `%`.** 62 does not divide 256, so taking the remainder would
    // make the first few letters of the alphabet slightly likelier than the last few. One
    // extra draw per unlucky byte costs nothing and removes the bias entirely.
    let mut password = String::with_capacity(LENGTH);
    let mut spare = [0_u8; 8];
    let mut at = 0;
    for byte in bytes {
        let mut value = byte;
        while value >= 248 {
            if at == 0 {
                getrandom::fill(&mut spare).map_err(|error| {
                    Failure::new(
                        crate::exit::Exit::Failure,
                        format!("the operating system would not provide random bytes: {error}"),
                    )
                })?;
            }
            value = spare[at];
            at = (at + 1) % spare.len();
        }
        password.push(char::from(ALPHABET[usize::from(value) % ALPHABET.len()]));
    }

    Ok(password)
}

/// Ask for a password, hidden, and again to be sure it is the one that was meant.
///
/// `None` when nothing was typed, which means *"generate one"* — the default, and what
/// every run did before this question existed.
///
/// **Asked twice because it is typed blind and cannot be read back.** Whatever is typed is
/// both set on the server and filed on this machine, so the two always agree with each
/// other — what a typo breaks is the thing somebody was about to paste it into, and a second
/// line catches that before a database exists rather than after.
///
/// **Here rather than in one command, because two of them ask it.** `db create` asks for the
/// password of a role it is about to make, and `R19f` asks for `sloop_db_admin`'s at Setup;
/// they are the same question about the same kind of value, and a second copy of it is a
/// second place for the two prompts to drift apart.
pub fn typed_twice(what: &str) -> Outcome<Option<Secret>> {
    let typed = rpassword::prompt_password(format!(
        "? Password for {what} [press Enter to have one generated]: "
    ))
    .map_err(|error| Failure::usage(format!("could not read the password: {error}")))?;

    if typed.is_empty() {
        return Ok(None);
    }

    let again = rpassword::prompt_password("? And again, to be sure: ")
        .map_err(|error| Failure::usage(format!("could not read the password: {error}")))?;

    if again != typed {
        return Err(Failure::usage("those two passwords are not the same").hint(
            "nothing was created. Run it again, or press Enter at the prompt to have one \
             generated",
        ));
    }

    Ok(Some(Secret::new(typed)))
}
