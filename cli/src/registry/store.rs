//! The registry, in PostgreSQL — `R19c4`.
//!
//! **`registry.toml` and `secrets.sealed` stop being files.** A registered database becomes a
//! row in `registered_database`, the backup keypair becomes a row in `backup_key`, and the
//! sealed password store becomes one `BYTEA` in `sealed_vault`. What a *project* is becomes a
//! row in `project`, keyed by the directory holding its `.sloop`, so the walk up the tree
//! still finds the project and the project's databases are still only that project's.
//!
//! **[`Registry`] does not change, and that is the whole design.** It was already a plain
//! in-memory value with one reader and one writer; this swaps what those two talk to. Every
//! command from `R2` to `R16` goes through [`super::Registries`], which goes through here, so
//! not one of them had to learn what a table is.
//!
//! **What still cannot live in the database.** The details that *open* the database:
//! `server.toml` and, on a machine with no keyring, the global `secrets.sealed` holding
//! sloop's own two passwords. Everything else moved.
//!
//! **Rows are read as JSON and written as literals.** `psql --tuples-only` with a field
//! separator would be a delimiter somebody's database name eventually contains;
//! `json_agg` hands back one line that `serde_json` parses, and every value written goes
//! through [`literal`], which doubles the quote and refuses a NUL. There is no value a user
//! can register that steers a statement.
//!
//! **Still no client crate.** Everything here is `psql`, the same as the rest of `server`.

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::Deserialize;

use crate::crypt::PublicKey;
use crate::engine::Engine;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::{Route, Secret};
use crate::server::{self, own::Own};

use super::Scope;
use super::file::{Database, Encryption, KeyKept, Registry};

/// An open way in to sloop's own database, for the length of one run.
///
/// Opened once, in `main`, and handed to everything that needs it. Not a connection pool and
/// not a long-lived socket — every statement is its own `psql` — but the *credentials* are
/// resolved once, so a keyring that asks for permission asks once per run.
#[derive(Debug)]
pub struct Store {
    server: server::Server,
    own: Own,
    password: Secret,
}

/// Which registry a row belongs to, as the tables spell it.
///
/// **Its own type because `Scope` alone is not enough to write a row.** A project row needs
/// the directory as well, and passing those two as separate arguments is how one of them
/// eventually gets left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Which {
    /// The global store: `scope = 'global'`, `project_id IS NULL`.
    Global,
    /// One project, by the directory holding its `.sloop`.
    Project(PathBuf),
}

impl Which {
    /// Which of the two, for a scope and the project in play.
    ///
    /// `None` when the scope is `Project` and there is no project, which is the one
    /// combination that cannot name a registry.
    #[must_use]
    pub fn of(scope: Scope, project: Option<&Path>) -> Option<Self> {
        match scope {
            Scope::Global => Some(Self::Global),
            Scope::Project => project.map(|dir| Self::Project(dir.to_path_buf())),
        }
    }

    /// The word the `scope` column holds.
    const fn word(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project(_) => "project",
        }
    }

    /// The directory, for a project.
    const fn directory(&self) -> Option<&PathBuf> {
        match self {
            Self::Global => None,
            Self::Project(dir) => Some(dir),
        }
    }
}

/// One registered database, as the query hands it back.
#[derive(Debug, Deserialize)]
struct RawDatabase {
    label: String,
    engine: String,
    host: String,
    port: i64,
    database: String,
    user: String,
    /// A route. The column's own CHECK refuses anything that is not one, and
    /// [`Route::parse`] refuses it again here — the same double refusal the file had.
    password: String,
}

/// The backup keypair, as the query hands it back.
#[derive(Debug, Deserialize)]
struct RawKey {
    public_key: String,
    private_key: String,
    key_kept: Option<String>,
}

/// A whole registry, as one JSON document.
#[derive(Debug, Deserialize)]
struct RawRegistry {
    databases: Vec<RawDatabase>,
    encryption: Option<RawKey>,
}

impl Store {
    /// Open sloop's own database, or say the machine has not been set up.
    ///
    /// `None` rather than a failure, because *"there is no database yet"* is an ordinary
    /// state — it is every machine before `sloop setup` — and the caller is what decides
    /// whether that is fatal for what it is about to do.
    pub fn open(global: &Path) -> Outcome<Option<Self>> {
        let Some(ready) = server::record::reopen(global)? else {
            return Ok(None);
        };
        let Some((own, password)) = server::record::database(global)? else {
            return Ok(None);
        };

        Ok(Some(Self {
            server: ready.server,
            own,
            password,
        }))
    }

    /// The same, but a machine that has not been set up is an error that says what to run.
    pub fn require(global: &Path) -> Outcome<Self> {
        Self::open(global)?.ok_or_else(|| {
            Failure::new(
                Exit::Usage,
                "sloop keeps its registry in its own PostgreSQL, and this machine has not \
                 been set up",
            )
            .hint("run `sloop setup` once — it is safe to run again afterwards")
        })
    }

    /// Read a registry out of the tables.
    ///
    /// An empty registry rather than an error when a project has no rows yet: that is the
    /// state of every project between `sloop init` and the first `db add`, exactly as a
    /// missing `registry.toml` was.
    pub fn read(&self, which: &Which) -> Outcome<Registry> {
        let raw: RawRegistry = self.json(&format!(
            "SELECT json_build_object(
                      'databases', coalesce((
                          SELECT json_agg(json_build_object(
                                     'label',    d.label,
                                     'engine',   e.name,
                                     'host',     d.host,
                                     'port',     d.port,
                                     'database', d.database_name,
                                     'user',     d.username,
                                     'password', d.password_route)
                                 ORDER BY d.label)
                            FROM registered_database d
                            JOIN engine e ON e.id = d.engine_id
                           WHERE {where_clause}), '[]'::json),
                      'encryption', (
                          SELECT json_build_object(
                                     'public_key',  k.public_key,
                                     'private_key', k.private_key_route,
                                     'key_kept',    k.key_kept)
                            FROM backup_key k
                           WHERE {key_where})
                    )::text;",
            where_clause = Self::belongs_to(which, "d"),
            key_where = Self::belongs_to(which, "k"),
        ))?;

        let mut registry = Registry::default();

        for row in raw.databases {
            let engine = Engine::parse(&row.engine)?;
            let port = u16::try_from(row.port).map_err(|_| {
                Failure::usage(format!("{} is registered on port {}", row.label, row.port))
            })?;
            // Parsed here, where the row is read, so a route somebody put in by hand is a
            // complaint about the registry rather than a surprise in the middle of a dump.
            // The same place the file version parsed it.
            let password =
                Route::parse(row.password.trim_end()).map_err(|why| why.prefixed(&row.label))?;

            registry.insert(
                row.label,
                Database {
                    engine,
                    host: row.host,
                    port,
                    database: row.database,
                    user: row.user,
                    password,
                },
            );
        }

        if let Some(key) = raw.encryption {
            registry.set_encryption(Encryption {
                public_key: PublicKey::parse(&key.public_key)
                    .map_err(|why| why.prefixed("encryption"))?,
                private_key: Route::parse(key.private_key.trim_end())
                    .map_err(|why| why.prefixed("encryption"))?,
                key_kept: key
                    .key_kept
                    .as_deref()
                    .map(KeyKept::parse)
                    .transpose()
                    .map_err(|why| why.prefixed("encryption"))?,
            });
        }

        Ok(registry)
    }

    /// Write a registry back.
    ///
    /// **Replace, not merge, and in one transaction.** The in-memory [`Registry`] is the whole
    /// truth about that scope — `db remove` is an entry that is no longer in it — so the rows
    /// for that registry are deleted and rewritten together. A run that dies halfway leaves
    /// the registry exactly as it was, which is the property [`Registry::save`]'s
    /// write-and-rename bought on a file.
    pub fn write(&self, which: &Which, registry: &Registry) -> Outcome<()> {
        let mut script = String::new();

        // The project row first: everything below points at it, and a project that has just
        // had its first database registered has no row yet.
        let project = match which.directory() {
            Some(dir) => {
                let _ = write!(
                    script,
                    "INSERT INTO project (directory) VALUES ({})
                     ON CONFLICT (directory) DO UPDATE SET last_seen = now();\n",
                    literal(&dir.display().to_string())?
                );
                format!(
                    "(SELECT id FROM project WHERE directory = {})",
                    literal(&dir.display().to_string())?
                )
            }
            None => "NULL".to_owned(),
        };

        let _ = writeln!(
            script,
            "DELETE FROM registered_database WHERE {};",
            Self::belongs_to(which, "registered_database")
        );

        for (label, database) in registry.entries() {
            let _ = write!(
                script,
                "INSERT INTO registered_database
                     (label, engine_id, host, port, database_name, username,
                      password_route, scope, project_id)
                 VALUES ({label}, (SELECT id FROM engine WHERE name = {engine}),
                         {host}, {port}, {database}, {user}, {route}, {scope}, {project});\n",
                label = literal(label)?,
                engine = literal(database.engine.scheme())?,
                host = literal(&database.host)?,
                port = database.port,
                database = literal(&database.database)?,
                user = literal(&database.user)?,
                route = literal(&database.password.as_field())?,
                scope = literal(which.word())?,
                project = project,
            );
        }

        let _ = writeln!(
            script,
            "DELETE FROM backup_key WHERE {};",
            Self::belongs_to(which, "backup_key")
        );

        if let Some(encryption) = registry.encryption() {
            let _ = write!(
                script,
                "INSERT INTO backup_key
                     (scope, project_id, public_key, private_key_route, key_kept)
                 VALUES ({scope}, {project}, {public}, {private}, {kept});\n",
                scope = literal(which.word())?,
                project = project,
                public = literal(&encryption.public_key.to_string())?,
                private = literal(&encryption.private_key.as_field())?,
                kept = match encryption.key_kept {
                    Some(kept) => literal(kept.as_field())?,
                    None => "NULL".to_owned(),
                },
            );
        }

        self.run(&script)
    }

    /// The sealed password store for a registry, or empty on one that has never had a
    /// password put in it.
    pub fn vault(&self, which: &Which) -> Outcome<Vec<u8>> {
        let hex = self.ask(&format!(
            "SELECT coalesce(encode(blob, 'hex'), '')
               FROM sealed_vault WHERE {};",
            Self::belongs_to(which, "sealed_vault")
        ))?;

        from_hex(hex.trim())
    }

    /// Put the sealed password store back.
    ///
    /// The bytes are `R3`'s own format, unchanged — one Argon2id-derived key over one
    /// XChaCha20-Poly1305 ciphertext. What moved is where they sit, not what they are.
    pub fn put_vault(&self, which: &Which, blob: &[u8]) -> Outcome<()> {
        if blob.is_empty() {
            return self.run(&format!(
                "DELETE FROM sealed_vault WHERE {};",
                Self::belongs_to(which, "sealed_vault")
            ));
        }

        let mut script = String::new();
        let project = match which.directory() {
            Some(dir) => {
                let _ = write!(
                    script,
                    "INSERT INTO project (directory) VALUES ({})
                     ON CONFLICT (directory) DO UPDATE SET last_seen = now();\n",
                    literal(&dir.display().to_string())?
                );
                format!(
                    "(SELECT id FROM project WHERE directory = {})",
                    literal(&dir.display().to_string())?
                )
            }
            None => "NULL".to_owned(),
        };

        let _ = write!(
            script,
            "INSERT INTO sealed_vault (scope, project_id, blob)
                  VALUES ({scope}, {project}, decode({hex}, 'hex'))
             ON CONFLICT (project_id) DO UPDATE
                    SET blob = EXCLUDED.blob, updated_at = now();\n",
            scope = literal(which.word())?,
            project = project,
            hex = literal(&to_hex(blob))?,
        );

        self.run(&script)
    }

    /// This registry's sealed password store, as something `secret` can read and write.
    ///
    /// **It borrows nothing.** A vault that held a reference to the [`super::Registries`] it
    /// came from would lock that value for as long as the vault lived — and every command
    /// that stores a password goes on to *write* the registry it just read the vault out of.
    /// So the store is shared rather than borrowed, and the vault outlives the call.
    #[must_use]
    pub fn vault_at(self: &Rc<Self>, which: Which) -> crate::secret::sealed::Vault<'static> {
        crate::secret::sealed::Vault::Rows(Box::new(Shelf {
            store: Rc::clone(self),
            which,
        }))
    }

    /// Bring a `registry.toml` from the previous release in, once.
    ///
    /// **Once, and the old file is kept.** The entry says so in as many words, and the reason
    /// is that a migration nobody has looked at is not one anybody should trust: the file
    /// stays exactly where it was, so a machine that migrated wrongly can be compared against
    /// what it had. `sloop db migrated` is what retires it, and that is the owner's to run.
    ///
    /// **It runs only into an empty registry.** A scope that already has rows has been
    /// through this, or has been used since, and importing over it would resurrect entries
    /// somebody removed.
    pub fn import_once(&self, which: &Which, dir: &Path) -> Outcome<()> {
        let file = dir.join(super::file::FILE);
        if !file.is_file() {
            return Ok(());
        }
        if self.has_rows(which)? {
            return Ok(());
        }

        let registry = Registry::load(&file)?;
        let sealed = dir.join(super::file::SEALED_FILE);
        let carried = std::fs::read(&sealed).unwrap_or_default();

        if registry.is_empty() && registry.encryption().is_none() && carried.is_empty() {
            return Ok(());
        }

        self.write(which, &registry)?;
        if !carried.is_empty() {
            self.put_vault(which, &carried)?;
        }

        crate::note!(
            "{}",
            crate::style::dim(&format!(
                "moved {} into sloop's own database — {} is kept until you say it went well",
                file.display(),
                file.display()
            ))
        );
        Ok(())
    }

    /// Does this registry hold anything at all?
    ///
    /// Rows *or* a vault: a registry whose every database used `${VAR}` has no sealed store,
    /// and one that only ever held a backup key has no databases. Either is "been through
    /// this already".
    fn has_rows(&self, which: &Which) -> Outcome<bool> {
        let said = self.ask(&format!(
            "SELECT (SELECT count(*) FROM registered_database WHERE {databases})
                  + (SELECT count(*) FROM backup_key          WHERE {keys})
                  + (SELECT count(*) FROM sealed_vault        WHERE {vaults});",
            databases = Self::belongs_to(which, "registered_database"),
            keys = Self::belongs_to(which, "backup_key"),
            vaults = Self::belongs_to(which, "sealed_vault"),
        ))?;

        Ok(said.trim() != "0")
    }

    /// The `WHERE` that picks one registry's rows out of a table.
    ///
    /// **One place, because getting it wrong is silent.** A predicate that forgot the project
    /// would read every project's databases into one listing, which is the single thing the
    /// owner said must never happen: *"other project local db will not be shown"*.
    fn belongs_to(which: &Which, table: &str) -> String {
        match which.directory() {
            None => format!("{table}.project_id IS NULL"),
            Some(dir) => format!(
                "{table}.project_id = (SELECT id FROM project WHERE directory = {})",
                // Infallible in practice — a path that will not go in a literal cannot have
                // been walked to — and a predicate that matches nothing is the safe way to
                // be wrong: it reads an empty registry rather than somebody else's.
                literal(&dir.display().to_string()).unwrap_or_else(|_| "NULL".to_owned())
            ),
        }
    }

    /// Ask for one value.
    fn ask(&self, sql: &str) -> Outcome<String> {
        server::make::ask(&self.server, &self.own.as_who(), Some(&self.password), sql)
    }

    /// Ask for one JSON document and parse it.
    fn json<T: serde::de::DeserializeOwned>(&self, sql: &str) -> Outcome<T> {
        let said = self.ask(sql)?;
        serde_json::from_str(said.trim()).map_err(|error| {
            Failure::new(
                Exit::Failure,
                format!("sloop's own database answered something it cannot read: {error}"),
            )
            .hint("`sloop setup` re-runs the migrations, which is where to start")
        })
    }

    /// Run a script, all of it or none of it.
    fn run(&self, sql: &str) -> Outcome<()> {
        server::make::script(&self.server, &self.own.as_who(), Some(&self.password), sql)
    }
}

/// A string as an SQL literal, or a refusal.
///
/// **Doubling the quote is the whole of it** while `standard_conforming_strings` is on, which
/// it has been by default since 9.1 and which `initdb` leaves on — so a backslash in a value
/// is a backslash and not an escape. A NUL is refused rather than truncated, because
/// PostgreSQL's `text` cannot hold one and a silently shortened host name is worse than a
/// complaint.
pub fn literal(text: &str) -> Outcome<String> {
    if text.contains('\0') {
        return Err(Failure::usage(
            "a registry value contains a zero byte, which PostgreSQL cannot store",
        ));
    }
    Ok(format!("'{}'", text.replace('\'', "''")))
}

/// Bytes as lowercase hex, for `decode(…, 'hex')`.
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        // Infallible: writing into a `String` cannot fail.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// The inverse, for what `encode(…, 'hex')` hands back.
fn from_hex(hex: &str) -> Outcome<Vec<u8>> {
    if hex.is_empty() {
        return Ok(Vec::new());
    }
    if !hex.len().is_multiple_of(2) {
        return Err(Failure::new(
            Exit::Failure,
            "the sealed vault came back as an odd number of hex digits",
        ));
    }

    (0..hex.len())
        .step_by(2)
        .map(|at| {
            u8::from_str_radix(&hex[at..at + 2], 16).map_err(|_| {
                Failure::new(
                    Exit::Failure,
                    "the sealed vault came back as something not hex",
                )
            })
        })
        .collect()
}

/// One registry's sealed password store, as [`crate::secret::sealed::Shelf`] sees it.
///
/// **The whole `SLOOPSEC` blob, read and written as one value.** `R3`'s format is one
/// Argon2id-derived key over one XChaCha20-Poly1305 ciphertext covering every pair at once;
/// splitting it per row would mean redesigning the crypto, so what changed is where the bytes
/// sit and nothing else.
struct Shelf {
    store: Rc<Store>,
    which: Which,
}

impl crate::secret::sealed::Shelf for Shelf {
    fn read(&self) -> Outcome<Vec<u8>> {
        self.store.vault(&self.which)
    }

    fn write(&self, blob: &[u8]) -> Outcome<()> {
        self.store.put_vault(&self.which, blob)
    }

    fn describe(&self) -> String {
        match self.which.directory() {
            None => format!("{} (global)", self.store.own.database),
            Some(dir) => format!("{} ({})", self.store.own.database, dir.display()),
        }
    }
}
