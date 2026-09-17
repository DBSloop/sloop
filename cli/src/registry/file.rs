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
use crate::ssh::{DEFAULT_PORT as SSH_PORT, Reach, Server, Through};

/// The registry file's name, inside `.sloop` or inside the global store.
pub const FILE: &str = "registry.toml";

/// The encrypted password file, beside the registry it belongs to.
pub const SEALED_FILE: &str = "secrets.sealed";

/// Bumped only when the shape below changes in a way an older sloop could misread.
const VERSION: u32 = 1;

/// One registered database, exactly as the file holds it.
///
/// `ssh` is last because it is a table and every scalar of the same parent has to come
/// before one — the same constraint that decides the order of [`RawFile`].
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh: Option<RawSsh>,
}

/// The SSH server a database is reached through, exactly as the file holds it.
///
/// **No registry file ever written has one of these**, because `R19c4` moved the registry
/// into PostgreSQL before `R19e` existed and [`Registry::to_toml`]'s only readers are the
/// tests. It is here so the round trip keeps proving what its doc comment claims it proves:
/// that [`Registry::parse`] reads everything a registry can hold. A shape that quietly
/// dropped a field would make that test pass while saying nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawSsh {
    host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
    /// A route, and never a passphrase — the same field `password` is, for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passphrase: Option<String>,
}

/// The backup keypair, exactly as the file holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawEncryption {
    public_key: String,
    private_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_kept: Option<String>,
}

/// The file, exactly as TOML sees it.
///
/// Field order is the order TOML writes: the version, then the keypair, then the databases.
/// A table has to come after every scalar that belongs to the same parent, and `databases`
/// is a table of tables, so this is the one order that round-trips.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encryption: Option<RawEncryption>,
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
    /// Straight at that address, or through an SSH server — `R19e`.
    ///
    /// **`host` and `port` above are as the *server* sees them** once this is
    /// [`Reach::Over`], which is almost always `127.0.0.1` and the engine's default port.
    /// That is the one sentence about this feature everybody gets backwards once, so it is
    /// said here, in `--help`, and by `db add` when it writes the record.
    pub reach: Reach,
}

impl Database {
    /// The key this database's password is filed under, in the keyring and in the
    /// encrypted file.
    ///
    /// Derived from the connection rather than from the name the user gave it, so
    /// `db rename` moves a label and does not orphan a password. It carries no secret —
    /// it is the same thing that shows up in a connection log.
    ///
    /// **The SSH server is part of it, and that is not decoration.** Every database reached
    /// over a tunnel is registered at the address the *server* sees, which is almost always
    /// `127.0.0.1:5432` — so two databases behind two different bastions, each called
    /// `orders` and each reached as `app`, produce the same connection string and would
    /// otherwise be filed under the same key. One would then be opened with the other's
    /// password. A direct registration is unaffected, byte for byte, because there is
    /// nothing to append.
    #[must_use]
    pub fn credential_key(&self) -> String {
        let connection = connection_string(
            self.engine,
            &self.user,
            &self.host,
            self.port,
            &self.database,
        );

        match self.reach.through() {
            None => connection,
            Some(through) => {
                format!("{connection} through {}", through.server.credential_key())
            }
        }
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

/// Whether the private key exists anywhere but this machine.
///
/// **The one piece of state that stops a backup.** An encrypted backup whose key lives only
/// in this machine's keyring dies with the machine, so the first one refuses until somebody
/// has either taken a copy of the key or said, in as many words, that they will not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKept {
    /// `sloop key export` has printed it at least once.
    Exported,
    /// Somebody was told what it costs and chose to carry on anyway.
    Declined,
}

impl KeyKept {
    /// How it is written in the file.
    #[must_use]
    pub const fn as_field(self) -> &'static str {
        match self {
            Self::Exported => "exported",
            Self::Declined => "declined",
        }
    }

    pub(crate) fn parse(field: &str) -> Outcome<Self> {
        match field.trim() {
            "exported" => Ok(Self::Exported),
            "declined" => Ok(Self::Declined),
            other => Err(
                Failure::usage(format!("{other} is not something key-kept can say"))
                    .hint("`exported` once the key has been written out, or `declined`"),
            ),
        }
    }
}

/// The backup keypair this registry encrypts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encryption {
    /// The public half. This is all an unattended backup needs.
    pub public_key: crate::crypt::PublicKey,
    /// Where the private half is kept: the keyring, or the encrypted file.
    pub private_key: Route,
    /// Whether a copy of the private key exists off this machine. See [`KeyKept`].
    pub key_kept: Option<KeyKept>,
}

/// A registry file's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    encryption: Option<Encryption>,
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

    /// The keypair this registry's backups are encrypted to, if it has one.
    #[must_use]
    pub const fn encryption(&self) -> Option<&Encryption> {
        self.encryption.as_ref()
    }

    /// Set, or replace, the keypair.
    ///
    /// Replacing one is how `key import` moves a registry onto a different key, and it is
    /// deliberately not something that happens quietly: whatever refuses to overwrite a key
    /// does so before calling this.
    pub fn set_encryption(&mut self, encryption: Encryption) {
        self.encryption = Some(encryption);
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

        // Parsed here, when the file is read, so a key somebody mistyped is a complaint
        // about the registry rather than a surprise at the end of a two-hour dump.
        let encryption = match raw.encryption {
            None => None,
            Some(block) => Some(Encryption {
                public_key: crate::crypt::PublicKey::parse(&block.public_key)
                    .map_err(|failure| failure.prefixed("encryption"))?,
                private_key: Route::parse(block.private_key.trim_end())
                    .map_err(|failure| failure.prefixed("encryption"))?,
                key_kept: block
                    .key_kept
                    .as_deref()
                    .map(KeyKept::parse)
                    .transpose()
                    .map_err(|failure| failure.prefixed("encryption"))?,
            }),
        };

        let mut databases = BTreeMap::new();
        for (name, entry) in raw.databases {
            // The route is parsed here, when the file is read, so a bad one is a complaint
            // about the file rather than a surprise in the middle of a backup.
            let password = Route::parse(entry.password.trim_end())
                .map_err(|failure| failure.prefixed(&name))?;

            let reach = match entry.ssh {
                None => Reach::Direct,
                Some(ssh) => Reach::Over(Box::new(Through {
                    server: Server {
                        host: ssh.host,
                        port: ssh.port.unwrap_or(SSH_PORT),
                        user: ssh.user,
                        identity: ssh.identity.map(std::path::PathBuf::from),
                    },
                    secret: ssh
                        .passphrase
                        .as_deref()
                        .map(|field| Route::parse(field.trim_end()))
                        .transpose()
                        .map_err(|failure| failure.prefixed(format!("{name} ssh")))?,
                })),
            };

            databases.insert(
                name,
                Database {
                    port: entry.port.unwrap_or_else(|| entry.engine.default_port()),
                    engine: entry.engine,
                    host: entry.host,
                    database: entry.database,
                    user: entry.user,
                    password,
                    reach,
                },
            );
        }

        Ok(Self {
            encryption,
            databases,
        })
    }

    /// Write a registry back out as TOML.
    ///
    /// **Nothing in the binary writes TOML any more** — `R19c4` moved the registry into
    /// PostgreSQL, and `Registry::save` went with it so that there is exactly one writer.
    /// This is kept because it is the inverse of [`Registry::parse`], which `import_once`
    /// still needs, and because the round-trip is what proves that parse reads everything
    /// a registry can hold. Its readers are the tests.
    #[allow(dead_code)]
    pub fn to_toml(&self) -> Outcome<String> {
        let raw = RawFile {
            version: VERSION,
            encryption: self.encryption.as_ref().map(|encryption| RawEncryption {
                public_key: encryption.public_key.to_string(),
                private_key: encryption.private_key.as_field(),
                key_kept: encryption.key_kept.map(|kept| kept.as_field().to_owned()),
            }),
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
                            ssh: database.reach.through().map(|through| RawSsh {
                                host: through.server.host.clone(),
                                port: Some(through.server.port),
                                user: through.server.user.clone(),
                                identity: through
                                    .server
                                    .identity
                                    .as_ref()
                                    .map(|path| path.display().to_string()),
                                passphrase: through
                                    .secret
                                    .as_ref()
                                    .map(crate::secret::Route::as_field),
                            }),
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
