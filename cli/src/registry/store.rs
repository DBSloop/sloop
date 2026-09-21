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
use crate::ssh::{Reach, Server, Through};

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
    /// The SSH server this one goes through, or `None` for the direct connection every
    /// database had before `R19e`.
    ssh: Option<RawSsh>,
}

/// The SSH server a row names, as the query hands it back.
#[derive(Debug, Deserialize)]
struct RawSsh {
    host: String,
    port: i64,
    user: Option<String>,
    identity: Option<String>,
    /// A route, and never a passphrase. Two CHECKs and [`Route::parse`] all refuse
    /// anything else.
    passphrase: Option<String>,
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

/// The failure for a store that is there and shut to this account.
///
/// **"This machine has not been set up" is a lie on a shared store**, and the expensive kind:
/// it sends somebody to run `sloop setup` on a machine that is already set up, which is how
/// `R31` started in the first place — a true sentence about the wrong thing. What is actually
/// wrong is one group membership, so that is what it says.
///
/// `None` when the record is genuinely absent, which is every machine before Setup.
fn locked_out(global: &Path) -> Option<Failure> {
    let record = global.join(crate::server::record::FILE);
    if !record.exists() {
        return None;
    }
    if std::fs::File::open(&record)
        .err()
        .is_none_or(|why| why.kind() != std::io::ErrorKind::PermissionDenied)
    {
        return None;
    }

    let group = crate::account::group_of(global);
    Some(
        Failure::new(
            Exit::Usage,
            format!(
                "{} is set up, and this account is not allowed to read it",
                global.display()
            ),
        )
        .hint(match group {
            Some(group) => format!(
                "this store belongs to the {group} group: `sudo usermod -aG {group} $(whoami)`, \
                 then log in again"
            ),
            None => "somebody with root on this machine has to give this account access to it"
                .to_owned(),
        }),
    )
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
            locked_out(global).unwrap_or_else(|| {
                Failure::new(
                    Exit::Usage,
                    "sloop keeps its registry in its own PostgreSQL, and this machine has not \
                     been set up",
                )
                .hint("run `sloop setup` once — it is safe to run again afterwards")
            })
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
                                     'password', d.password_route,
                                     -- One object or NULL, rather than five nullable
                                     -- fields: `ssh_host IS NULL` is the whole question of
                                     -- whether this database goes over SSH, and answering
                                     -- it once in SQL is what lets Rust match on an Option
                                     -- instead of re-deriving it from four columns.
                                     'ssh', CASE WHEN d.ssh_host IS NULL THEN NULL ELSE
                                         json_build_object(
                                             'host',       d.ssh_host,
                                             'port',       d.ssh_port,
                                             'user',       d.ssh_user,
                                             'identity',   d.ssh_identity,
                                             'passphrase', d.ssh_secret_route)
                                     END)
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
            let port = port_of(&row.label, row.port, "registered on")?;
            // Parsed here, where the row is read, so a route somebody put in by hand is a
            // complaint about the registry rather than a surprise in the middle of a dump.
            // The same place the file version parsed it.
            let password =
                Route::parse(row.password.trim_end()).map_err(|why| why.prefixed(&row.label))?;

            let reach = match row.ssh {
                None => Reach::Direct,
                Some(ssh) => {
                    let ssh_port = port_of(&row.label, ssh.port, "reached over SSH on")?;
                    Reach::Over(Box::new(Through {
                        server: Server {
                            host: ssh.host,
                            port: ssh_port,
                            user: ssh.user,
                            identity: ssh.identity.map(PathBuf::from),
                        },
                        secret: ssh
                            .passphrase
                            .as_deref()
                            .map(|field| Route::parse(field.trim_end()))
                            .transpose()
                            .map_err(|why| why.prefixed(format!("{} ssh", row.label)))?,
                    }))
                }
            };

            registry.insert(
                row.label,
                Database {
                    engine,
                    host: row.host,
                    port,
                    database: row.database,
                    user: row.user,
                    password,
                    reach,
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
    /// truth about that scope — `db remove` is an entry that is no longer in it — so what is
    /// not in it goes and what is in it is written, together. A run that dies halfway leaves
    /// the registry exactly as it was, which is the property [`Registry::save`]'s
    /// write-and-rename bought on a file.
    ///
    /// **A surviving row keeps its `id`, and `R25` is what made that matter.** This used to
    /// delete every row for the registry and insert them all again, which is the same registry
    /// and a different set of primary keys — and `monitored_database` and `bandwidth_day` both
    /// hang off `registered_database.id` with `ON DELETE CASCADE`. So `sloop db add` on the
    /// global store silently detached every database from the service and took the whole
    /// history with it. Running `R25` against a real cluster is what found it; nothing could
    /// have noticed before, because nothing had ever put a row in either table.
    ///
    /// The upsert keys on `one_label_per_registry`, which `0002` already declared. **A label
    /// that changed is still a different row** — a rename is a delete and an insert here, and
    /// what that costs an attachment is noted in `docs/OWNER-DECISIONS.md` rather than fixed
    /// under an entry that is about something else.
    pub fn write(&self, which: &Which, registry: &Registry) -> Outcome<()> {
        self.run(&Self::script_for(which, registry)?)
    }

    /// Rename an entry, keeping the row it is.
    ///
    /// **An `UPDATE` in front of the ordinary write, in one transaction.** [`write`] is handed
    /// the registry *after* the change and cannot tell a rename from a remove-and-add — so it
    /// would delete the old label's row and insert a new one, and `monitored_database` and
    /// `bandwidth_day` would cascade away with it. This relabels first, so by the time the
    /// write runs the row already carries the new label and the upsert merely updates it.
    ///
    /// `--single-transaction`, which [`run`] passes, is what makes the two one thing: a rename
    /// that dies between them would otherwise leave a relabelled row and a registry that still
    /// says the old name.
    ///
    /// [`write`]: Self::write
    /// [`run`]: Self::run
    pub fn rename(&self, which: &Which, from: &str, to: &str, registry: &Registry) -> Outcome<()> {
        let mut script = format!(
            "UPDATE registered_database
                SET label = {to}, updated_at = now()
              WHERE {belongs} AND label = {from};
",
            to = literal(to)?,
            from = literal(from)?,
            belongs = Self::belongs_to(which, "registered_database"),
        );
        script.push_str(&Self::script_for(which, registry)?);

        self.run(&script)
    }

    /// The statements [`Self::write`] runs, without running them.
    fn script_for(which: &Which, registry: &Registry) -> Outcome<String> {
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

        script.push_str(&Self::databases(which, registry, &project)?);

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

        Ok(script)
    }

    /// The `registered_database` half of a write: what is gone goes, what is here is upserted.
    ///
    /// **Its own function because the two halves answer different questions.** Above is which
    /// rows belong to this registry at all; this is which of them survived, and the difference
    /// between deleting the survivors and keeping them is a table two other tables cascade off.
    fn databases(which: &Which, registry: &Registry, project: &str) -> Outcome<String> {
        let mut script = String::new();

        // **What is gone, rather than everything.** A registry with nothing in it deletes the
        // lot, which is what `db remove` of the last entry means.
        let surviving = registry
            .entries()
            .map(|(label, _)| literal(label))
            .collect::<Outcome<Vec<String>>>()?;
        let _ = writeln!(
            script,
            "DELETE FROM registered_database WHERE {}{};",
            Self::belongs_to(which, "registered_database"),
            if surviving.is_empty() {
                String::new()
            } else {
                format!(" AND label NOT IN ({})", surviving.join(", "))
            }
        );

        for (label, database) in registry.entries() {
            // Five values or five NULLs. `Reach::Direct` is not an absence of settings to
            // be filled in later — it is the ordinary state, and the constraints on those
            // columns say so.
            let through = database.reach.through();
            let optional = |value: Option<String>| match value {
                Some(value) => literal(&value),
                None => Ok("NULL".to_owned()),
            };

            let _ = write!(
                script,
                "INSERT INTO registered_database
                     (label, engine_id, host, port, database_name, username,
                      password_route, scope, project_id,
                      ssh_host, ssh_port, ssh_user, ssh_identity, ssh_secret_route)
                 VALUES ({label}, (SELECT id FROM engine WHERE name = {engine}),
                         {host}, {port}, {database}, {user}, {route}, {scope}, {project},
                         {ssh_host}, {ssh_port}, {ssh_user}, {ssh_identity}, {ssh_route})
                 ON CONFLICT ON CONSTRAINT one_label_per_registry DO UPDATE
                     SET engine_id        = EXCLUDED.engine_id,
                         host             = EXCLUDED.host,
                         port             = EXCLUDED.port,
                         database_name    = EXCLUDED.database_name,
                         username         = EXCLUDED.username,
                         password_route   = EXCLUDED.password_route,
                         scope            = EXCLUDED.scope,
                         project_id       = EXCLUDED.project_id,
                         ssh_host         = EXCLUDED.ssh_host,
                         ssh_port         = EXCLUDED.ssh_port,
                         ssh_user         = EXCLUDED.ssh_user,
                         ssh_identity     = EXCLUDED.ssh_identity,
                         ssh_secret_route = EXCLUDED.ssh_secret_route,
                         updated_at       = now();\n",
                label = literal(label)?,
                engine = literal(database.engine.scheme())?,
                host = literal(&database.host)?,
                port = database.port,
                database = literal(&database.database)?,
                user = literal(&database.user)?,
                route = literal(&database.password.as_field())?,
                scope = literal(which.word())?,
                project = project,
                ssh_host = optional(through.map(|one| one.server.host.clone()))?,
                ssh_port = match through {
                    Some(one) => one.server.port.to_string(),
                    None => "NULL".to_owned(),
                },
                ssh_user = optional(through.and_then(|one| one.server.user.clone()))?,
                ssh_identity = optional(through.and_then(|one| {
                    one.server
                        .identity
                        .as_ref()
                        .map(|path| path.display().to_string())
                }))?,
                ssh_route =
                    optional(through.and_then(|one| one.secret.as_ref().map(Route::as_field)))?,
            );
        }

        Ok(script)
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
    ///
    /// **`pub(crate)` for the one other module that reads these tables.** `R25`'s service
    /// attachments live in `monitored_database`, beside the registry and behind the same
    /// credentials — so `service::watch` goes through this rather than opening a second way
    /// in to the same database. It follows the same two rules this module does: read as JSON,
    /// write through [`literal`].
    pub(crate) fn ask(&self, sql: &str) -> Outcome<String> {
        server::make::ask(&self.server, &self.own.as_who(), Some(&self.password), sql)
    }

    /// Ask for one JSON document and parse it.
    pub(crate) fn json<T: serde::de::DeserializeOwned>(&self, sql: &str) -> Outcome<T> {
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
    pub(crate) fn run(&self, sql: &str) -> Outcome<()> {
        server::make::script(&self.server, &self.own.as_who(), Some(&self.password), sql)
    }
}

/// A port as PostgreSQL stored it, back as a port.
///
/// **A column wide enough to hold a number a port cannot be**, so the narrowing is a real
/// question rather than a formality: `bigint` is what comes back, and a row edited by
/// hand can hold 70000 in it. Both the direct port and the SSH one come through here, so the
/// complaint reads the same either way and names the command that fixes it.
fn port_of(label: &str, stored: i64, how: &str) -> Outcome<u16> {
    u16::try_from(stored).map_err(|_| {
        Failure::usage(format!("{label} is {how} port {stored}"))
            .hint(format!("a port is 1 to 65535: `sloop db edit {label}`"))
    })
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
        )
        .hint("take it out of whatever was typed — `sloop db edit <name>` shows every field"));
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
        )
        .report_a_bug());
    }

    (0..hex.len())
        .step_by(2)
        .map(|at| {
            u8::from_str_radix(&hex[at..at + 2], 16).map_err(|_| {
                Failure::new(
                    Exit::Failure,
                    "the sealed vault came back as something not hex",
                )
                .report_a_bug()
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
