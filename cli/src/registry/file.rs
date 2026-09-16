//! The registry file: what it holds, and the rules it is read under.
//!
//! **Parsed, never sourced.** It is TOML, read by a parser, and no value in it is ever
//! executed, expanded or interpreted as anything but itself. There is no line in this
//! codebase that hands a registry value to a shell.
//!
//! **No password is in it.** The `password` field holds a *route* — a sentence saying
//! where to go and ask — and a field that is not one of those sentences is refused. That
//! refusal is what makes a plaintext password impossible to write here by accident: there
//! is no spelling of one that would be accepted.

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{Engine, Target, connection_string};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::{Route, Secret};

/// The registry file's name, inside `.sloop` or inside the global store.
pub const FILE: &str = "registry.toml";

/// The encrypted password file, beside the registry it belongs to.
pub const SEALED_FILE: &str = "secrets.sealed";

/// Bumped only when the shape below changes in a way an older sloop could misread.
const VERSION: u32 = 1;

/// One registered database, exactly as the file holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDatabase {
    engine: Engine,
    host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    database: String,
    user: String,
    password: String,
}

/// The file, exactly as TOML sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    version: u32,
    #[serde(default, rename = "databases")]
    databases: BTreeMap<String, RawDatabase>,
}

/// One registered database, with its password route already understood.
///
/// `database` repeats the type's name on purpose: it is what PostgreSQL and MySQL both
/// call the field, and it is the key the user writes in the file. A struct named after
/// the thing it describes is not a reason to rename the thing.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Database {
    /// Which engine.
    pub engine: Engine,
    /// Host name or address.
    pub host: String,
    /// Port, or the engine's default.
    pub port: u16,
    /// The database name on the server.
    pub database: String,
    /// The role to connect as.
    pub user: String,
    /// Where its password comes from. Never the password.
    pub password: Route,
}

impl Database {
    /// The key this database's password is filed under, in the keyring and in the
    /// encrypted file.
    ///
    /// Derived from the connection rather than from the name the user gave it, so
    /// `db rename` moves a label and does not orphan a password. It carries no secret —
    /// it is the same thing that shows up in a connection log.
    #[must_use]
    pub fn credential_key(&self) -> String {
        connection_string(
            self.engine,
            &self.user,
            &self.host,
            self.port,
            &self.database,
        )
    }

    /// This entry as something an adapter can act on.
    ///
    /// The one place the registry hands over to `engine`, and the reason nothing in
    /// `engine` has to know what a registry file looks like.
    #[must_use]
    pub fn target<'a>(&'a self, password: &'a Secret) -> Target<'a> {
        Target {
            engine: self.engine,
            host: &self.host,
            port: self.port,
            database: &self.database,
            user: &self.user,
            password,
        }
    }
}

/// A registry file's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    databases: BTreeMap<String, Database>,
}

impl Registry {
    /// How many databases are registered in this one.
    ///
    /// Read by the tests. A command asks [`super::Registries`] instead, because a command
    /// cares about the project and the global store together and one of these is only
    /// ever half the answer.
    #[allow(dead_code)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.databases.len()
    }

    /// Is this one empty? See [`Registry::len`] for why a command does not ask this.
    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.databases.is_empty()
    }

    /// Every database, by name, in order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Database)> {
        self.databases
            .iter()
            .map(|(name, database)| (name.as_str(), database))
    }

    /// The database registered under `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Database> {
        self.databases.get(name)
    }

    /// Register one, or replace the entry already under that name.
    ///
    /// Returns what was there before, which is what `db add` needs in order to refuse
    /// rather than overwrite, and what `db edit` needs in order to know the old
    /// [`Database::credential_key`] and move the stored password off it.
    pub fn insert(&mut self, name: String, database: Database) -> Option<Database> {
        self.databases.insert(name, database)
    }

    /// Forget one. The server is never touched; that is `db drop`, and R8's problem.
    ///
    /// So is this: R8 is `db remove`. It lives here because taking an entry out is the
    /// other half of putting one in, and the two belong side by side.
    #[allow(dead_code)]
    pub fn remove(&mut self, name: &str) -> Option<Database> {
        self.databases.remove(name)
    }

    /// Move an entry to a different name, keeping everything else about it.
    ///
    /// Only a label moves. [`Database::credential_key`] is derived from the connection
    /// and not from the name, precisely so that this cannot orphan a password.
    pub fn rename(&mut self, from: &str, to: String) -> Outcome<()> {
        let Some(database) = self.databases.remove(from) else {
            return Err(unknown(from));
        };
        self.databases.insert(to, database);
        Ok(())
    }

    /// Write the registry back, atomically.
    ///
    /// **Through a temporary file and a rename**, because the alternative is a process
    /// that dies mid-write and leaves a half-written registry — which parses as a syntax
    /// error and takes every *other* registered database down with it. A rename over an
    /// existing file is atomic on every platform this ships to.
    pub fn save(&self, path: &std::path::Path) -> Outcome<()> {
        let text = self.to_toml()?;

        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        if let Some(parent) = parent {
            std::fs::create_dir_all(parent).map_err(|error| {
                Failure::usage(format!("could not create {}: {error}", parent.display()))
            })?;
        }

        // Beside the real file rather than in the system temp directory: a rename across
        // filesystems is not atomic, and on Linux `/tmp` is very often a different one.
        let staged = path.with_extension("toml.writing");
        std::fs::write(&staged, text.as_bytes()).map_err(|error| {
            Failure::usage(format!("could not write {}: {error}", staged.display()))
        })?;

        std::fs::rename(&staged, path).map_err(|error| {
            // The staged file is no use to anybody if the rename failed, and leaving it
            // behind makes the next run look like it crashed.
            let _ = std::fs::remove_file(&staged);
            Failure::usage(format!("could not replace {}: {error}", path.display()))
        })
    }

    /// Read a registry from the text of a file.
    ///
    /// Carriage returns are taken off the ends of lines first. TOML treats `\r\n` as a
    /// line ending already, but a file that has been through a Windows editor and a
    /// `git` checkout and back can end up with a lone `\r` somewhere, and the parse error
    /// that causes points at a character nobody can see.
    pub fn parse(text: &str) -> Outcome<Self> {
        let cleaned = strip_carriage_returns(text);

        let raw: RawFile = toml::from_str(&cleaned).map_err(|error| {
            Failure::new(Exit::Usage, format!("the registry could not be read: {error}"))
                .hint("it is TOML, and sloop wrote it — if it has been edited by hand, that is where to look")
        })?;

        if raw.version > VERSION {
            return Err(Failure::new(
                Exit::Usage,
                format!(
                    "the registry says version {} and this sloop understands version {VERSION}",
                    raw.version
                ),
            )
            .hint("a newer sloop wrote it"));
        }

        let mut databases = BTreeMap::new();
        for (name, entry) in raw.databases {
            // The route is parsed here, when the file is read, so a bad one is a complaint
            // about the file rather than a surprise in the middle of a backup.
            let password = Route::parse(entry.password.trim_end())
                .map_err(|failure| failure.prefixed(&name))?;

            databases.insert(
                name,
                Database {
                    port: entry.port.unwrap_or_else(|| entry.engine.default_port()),
                    engine: entry.engine,
                    host: entry.host,
                    database: entry.database,
                    user: entry.user,
                    password,
                },
            );
        }

        Ok(Self { databases })
    }

    /// Write a registry back out.
    pub fn to_toml(&self) -> Outcome<String> {
        let raw = RawFile {
            version: VERSION,
            databases: self
                .databases
                .iter()
                .map(|(name, database)| {
                    (
                        name.clone(),
                        RawDatabase {
                            engine: database.engine,
                            host: database.host.clone(),
                            port: Some(database.port),
                            database: database.database.clone(),
                            user: database.user.clone(),
                            password: database.password.as_field(),
                        },
                    )
                })
                .collect(),
        };

        toml::to_string_pretty(&raw).map_err(|error| {
            Failure::new(
                Exit::Failure,
                format!("could not write the registry: {error}"),
            )
        })
    }

    /// Read the registry at `path`, or an empty one when there is no file there yet.
    pub fn load(path: &std::path::Path) -> Outcome<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|failure| failure.prefixed(path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(Failure::new(
                Exit::Usage,
                format!("could not read {}: {error}", path.display()),
            )),
        }
    }
}

/// A name nothing is registered under.
///
/// One sentence, in one place, because six commands say it and six slightly different
/// wordings is how a user starts wondering whether they mean different things.
pub fn unknown(name: &str) -> Failure {
    Failure::new(Exit::Usage, format!("no database is registered as {name}"))
        .hint("`sloop db list` shows what is, and `sloop db add` registers one")
}

/// Check that a database name is one a person can type back.
///
/// Looser than a project name — this one never becomes a filename — but not a free-for-all:
/// a leading or trailing space is invisible in every listing, and a colon is the qualifier
/// that tells `global:staging` from a database actually called `global:staging`.
pub fn check_name(name: &str) -> Outcome<&str> {
    let refuse = |why: &str| {
        Failure::usage(format!("{name} cannot be a database name: {why}"))
            .hint("letters, digits and the usual punctuation, with no colon and no stray spaces")
    };

    if name.is_empty() {
        return Err(refuse("it is empty"));
    }
    if name.trim() != name {
        return Err(refuse("it starts or ends with whitespace"));
    }
    if name.contains(':') {
        return Err(refuse("a colon is the `global:` qualifier"));
    }
    if let Some(bad) = name.chars().find(|letter| letter.is_control()) {
        return Err(refuse(&format!(
            "{} is not something you could type back",
            bad.escape_debug()
        )));
    }

    Ok(name)
}

/// Take a carriage return off the end of every line.
///
/// Done on the text rather than on values, because a lone `\r` is a line-ending artefact
/// and never something a person typed on purpose.
fn strip_carriage_returns(text: &str) -> String {
    text.lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect::<Vec<_>>()
        .join("\n")
}
