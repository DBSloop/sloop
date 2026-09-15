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

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Route;

/// The registry file's name, inside `.sloop` or inside the global store.
pub const FILE: &str = "registry.toml";

/// The encrypted password file, beside the registry it belongs to. Waits for R7.
#[allow(dead_code)]
pub const SEALED_FILE: &str = "secrets.sealed";

/// Bumped only when the shape below changes in a way an older sloop could misread.
const VERSION: u32 = 1;

/// The engines. Nothing else in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// PostgreSQL.
    Postgres,
    /// MySQL.
    Mysql,
    /// MariaDB — a separate engine, not an alias, because it ships `mariadb-dump`.
    Mariadb,
}

impl Engine {
    /// The port this engine listens on when nobody says otherwise.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Postgres => 5432,
            Self::Mysql | Self::Mariadb => 3306,
        }
    }

    /// The scheme this engine uses in a URL. Used by `Database::credential_key`, which
    /// waits for R7.
    #[allow(dead_code)]
    #[must_use]
    pub const fn scheme(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
        }
    }
}

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

// Waits for R7 to file the first password under it.
#[allow(dead_code)]
impl Database {
    /// The key this database's password is filed under, in the keyring and in the
    /// encrypted file.
    ///
    /// Derived from the connection rather than from the name the user gave it, so
    /// `db rename` moves a label and does not orphan a password. It carries no secret —
    /// it is the same thing that shows up in a connection log.
    #[must_use]
    pub fn credential_key(&self) -> String {
        format!(
            "{}://{}@{}:{}/{}",
            self.engine.scheme(),
            self.user,
            self.host,
            self.port,
            self.database
        )
    }
}

/// A registry file's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    databases: BTreeMap<String, Database>,
}

impl Registry {
    /// How many databases are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.databases.len()
    }

    /// Is it empty?
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

    /// Write a registry back out. Waits for R7's `db add`.
    #[allow(dead_code)]
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
