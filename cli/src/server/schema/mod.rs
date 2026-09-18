//! The tables inside `sloop_database`, and the migrations that put them there.
//!
//! **An upgrade is a migration, not a surprise.** Every change to this schema is a numbered
//! file that runs exactly once against a given database, recorded in `schema_migration` with
//! the SHA-256 of its own SQL. A machine a release behind runs the ones it has not had and
//! nothing else; a machine that is up to date runs nothing at all and says so; and a file
//! edited after it was applied somewhere is a failure that names the migration rather than a
//! schema that quietly disagrees with the build reading it.
//!
//! **Eleven tables, and every one of them is on the owner's list:**
//!
//! ```text
//! schema_migration      which migrations have run here                     0001
//! engine                the engines this build speaks                      0002
//! project               a directory with a .sloop in it                    0002
//! registered_database   host, port, user, route, and whose registry        0002
//! backup_key            the age keypair, one per registry                  0002
//! backup_run            backup history                                     0003
//! restore_run           restore history                                    0003
//! service               whether the service is set up                      0004
//! monitored_database    which databases are attached to it                 0004
//! bandwidth_day         bytes in and out, per database, per UTC day        0004
//! sealed_vault          `secrets.sealed`, as ciphertext in a column        0005
//! ```
//!
//! **`0006` and `0007` add no table.** `0006` puts five columns on `registered_database` —
//! the SSH server a database is reached through, when it is reached through one — and `0007`
//! puts one on `monitored_database`, the moment the running service last read that
//! attachment. Which is why the count above is still eleven.
//!
//! **No user data, ever.** What is stored is about *databases* — their addresses, their
//! sizes, their row counts, how long a dump took. Not one column holds anything that was
//! inside one of the user's tables, and [`tests::no_column_could_hold_a_secret`] is what
//! keeps a later one from creeping in.
//!
//! **And no plaintext password.** Every credential column holds a *route*; the one column
//! that holds secret material at all is `sealed_vault.blob`, which is `R3`'s ciphertext and
//! cannot be read without a passphrase this database has never seen.
//!
//! **Every table is reachable from a documented query.** [`READINGS`] is that list, one entry
//! per table, and a unit test proves it covers exactly the tables the migrations create — so
//! adding a table without documenting the way into it does not compile past `cargo test`.
//!
//! **Still no client crate.** Every statement here goes through `psql`, the same way the rest
//! of `server` does. `cargo tree` is unchanged by this module.

#[cfg(test)]
mod tests;

use sha2::{Digest as _, Sha256};

use crate::engine::Engine;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;

use super::Server;
use super::make::{self, As};
use super::own::Own;

/// One numbered step of the schema.
///
/// The SQL is `include_str!`d rather than written in Rust so that it reads as SQL, can be run
/// by hand against a copy of the database, and hashes to something stable.
pub struct Migration {
    /// Its number. Dense from 1, which [`tests::the_migrations_are_numbered_from_one`]
    /// enforces — a gap would make "a release older" ambiguous.
    pub version: i64,
    /// A word for it, for the ledger and for the line Setup prints.
    pub name: &'static str,
    /// The statements, exactly as the file holds them.
    pub sql: &'static str,
}

impl Migration {
    /// SHA-256 of the SQL, lowercase hex.
    ///
    /// Recorded when the migration runs, and checked on every run after it. The point is not
    /// tamper-proofing — anybody who can reach this database can rewrite the ledger too — it
    /// is catching the honest mistake of editing a migration that has already shipped.
    #[must_use]
    pub fn checksum(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.sql.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// The schema, in order.
///
/// **Append only.** A migration that has shipped is never edited and never renumbered; a
/// change to the schema is the next number. That is what makes the checksum check meaningful
/// and what makes a machine a release behind a knowable state rather than a guess.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "ledger",
        sql: include_str!("migrations/0001_ledger.sql"),
    },
    Migration {
        version: 2,
        name: "registry",
        sql: include_str!("migrations/0002_registry.sql"),
    },
    Migration {
        version: 3,
        name: "history",
        sql: include_str!("migrations/0003_history.sql"),
    },
    Migration {
        version: 4,
        name: "service",
        sql: include_str!("migrations/0004_service.sql"),
    },
    Migration {
        version: 5,
        name: "vault",
        sql: include_str!("migrations/0005_vault.sql"),
    },
    Migration {
        version: 6,
        name: "ssh",
        sql: include_str!("migrations/0006_ssh.sql"),
    },
    Migration {
        version: 7,
        name: "attachment",
        sql: include_str!("migrations/0007_attachment.sql"),
    },
];

/// The documented way in to one table.
///
/// **The `Done when` of `R19c3`, as data rather than as prose.** Every table has an entry,
/// a unit test proves the list and the migrations agree, and a cluster test runs every one of
/// these against a real migrated database — so a query that stops being valid fails a test
/// instead of sitting in a comment being wrong.
///
/// Nothing in the binary reads it yet — `R19c4` is what moves the registry onto these queries.
/// Until then its reader is `server::cluster_tests`, which runs every one of them against a
/// real migrated database.
#[allow(dead_code)]
pub struct Reading {
    /// The table it reads.
    pub table: &'static str,
    /// What somebody is asking when they run it.
    pub purpose: &'static str,
    /// The query. Valid against an empty database as well as a full one, which is what lets
    /// the cluster test run all of them straight after migrating.
    pub sql: &'static str,
}

/// One documented query per table.
#[allow(dead_code)]
pub const READINGS: &[Reading] = &[
    Reading {
        table: "schema_migration",
        purpose: "which migrations this database has had, oldest first",
        sql: "SELECT version, name, applied_by, applied_at
                FROM schema_migration
               ORDER BY version;",
    },
    Reading {
        table: "engine",
        purpose: "the engines this build speaks",
        sql: "SELECT name, display_name, default_port
                FROM engine
               WHERE supported
               ORDER BY name;",
    },
    Reading {
        table: "project",
        purpose: "every project this machine has registered a database from",
        sql: "SELECT directory, first_seen, last_seen
                FROM project
               ORDER BY directory;",
    },
    Reading {
        table: "registered_database",
        // The project's own first, then the global store's: `scope` DESC puts 'project'
        // above 'global' because that is the order the two words sort in, which is a
        // coincidence worth naming rather than relying on silently.
        purpose: "every database this registry can reach, the project's own listed first, \
                  and the SSH server each one goes through when it goes through one",
        sql: "SELECT d.label, e.name AS engine, d.host, d.port, d.database_name,
                     d.username, d.password_route, d.scope, p.directory AS project,
                     d.ssh_host, d.ssh_port, d.ssh_user, d.ssh_identity, d.ssh_secret_route
                FROM registered_database d
                JOIN engine  e ON e.id = d.engine_id
                LEFT JOIN project p ON p.id = d.project_id
               ORDER BY d.scope DESC, d.label;",
    },
    Reading {
        table: "backup_key",
        purpose: "the backup keypair of each registry, and whether the private half has been \
                  taken off this machine",
        sql: "SELECT k.scope, p.directory AS project, k.public_key,
                     k.private_key_route, k.key_kept
                FROM backup_key k
                LEFT JOIN project p ON p.id = k.project_id
               ORDER BY k.scope DESC;",
    },
    Reading {
        table: "backup_run",
        purpose: "the last twenty backups, newest first",
        sql: "SELECT label, started_at, outcome, exit_code, dump_bytes, rows_counted,
                     encrypted, took_seconds
                FROM backup_run
               ORDER BY started_at DESC
               LIMIT 20;",
    },
    Reading {
        table: "restore_run",
        purpose: "the last twenty restores, and which backup each one came from",
        sql: "SELECT r.label, r.started_at, r.outcome, r.exit_code, r.rows_restored,
                     r.verified, b.directory AS from_backup
                FROM restore_run r
                LEFT JOIN backup_run b ON b.id = r.backup_run_id
               ORDER BY r.started_at DESC
               LIMIT 20;",
    },
    Reading {
        table: "service",
        purpose: "whether the service is set up on this machine, and what is holding it",
        sql: "SELECT name, installed, mechanism, installed_at, last_seen_at
                FROM service
               ORDER BY name;",
    },
    Reading {
        table: "monitored_database",
        purpose: "which databases are attached to the service, and when a running service \
                  last read each attachment — NULL until one has picked it up",
        sql: "SELECT s.name AS service, d.label, m.enabled, m.attached_at, m.seen_at
                FROM monitored_database m
                JOIN service s ON s.id = m.service_id
                JOIN registered_database d ON d.id = m.registered_database_id
               ORDER BY s.name, d.label;",
    },
    Reading {
        table: "sealed_vault",
        // Never `blob` itself. A documented query is something somebody runs at a prompt,
        // and dumping ciphertext across a terminal helps nobody.
        purpose: "which registries have a sealed vault, and how big each one is — never its                   contents",
        sql: "SELECT v.scope, p.directory AS project,
                     octet_length(v.blob) AS bytes, v.updated_at
                FROM sealed_vault v
                LEFT JOIN project p ON p.id = v.project_id
               ORDER BY v.scope DESC;",
    },
    Reading {
        table: "bandwidth_day",
        // The point of the table, as a query: four windows, one scan, no rollups.
        purpose: "bytes in and out per database over a day, a week, a month and a year — all \
                  from the one table",
        sql: "SELECT d.label,
                     sum(b.bytes_in)  FILTER (WHERE b.day  = current_date)       AS in_today,
                     sum(b.bytes_in)  FILTER (WHERE b.day >  current_date - 7)   AS in_week,
                     sum(b.bytes_in)  FILTER (WHERE b.day >  current_date - 30)  AS in_month,
                     sum(b.bytes_in)  FILTER (WHERE b.day >  current_date - 365) AS in_year,
                     sum(b.bytes_out) FILTER (WHERE b.day >  current_date - 365) AS out_year
                FROM bandwidth_day b
                JOIN registered_database d ON d.id = b.registered_database_id
               GROUP BY d.label
               ORDER BY d.label;",
    },
];

/// What a run of [`migrate`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The version this database was at before. `0` on one that had never been migrated.
    pub from: i64,
    /// The version it is at now.
    pub to: i64,
    /// The migrations this run applied, in order. Empty on a database that was up to date.
    pub ran: Vec<&'static str>,
}

impl Applied {
    /// Whether this run changed anything.
    #[must_use]
    pub fn changed_anything(&self) -> bool {
        !self.ran.is_empty()
    }
}

/// Bring `sloop_database` up to the schema this build expects.
///
/// Idempotent, because Setup is re-runnable: a second call reads the ledger, finds nothing to
/// do and returns `from == to` with an empty `ran`.
pub fn migrate(server: &Server, own: &Own, password: &Secret) -> Outcome<Applied> {
    migrate_through(server, own, password, MIGRATIONS)
}

/// The same, through a given set of migrations.
///
/// **Split off so a test can build a database a release older** — apply the first three, then
/// run the real [`migrate`] against it and prove the fourth arrives. There is no other way to
/// get such a database on a machine where this release is the only one that exists.
pub(super) fn migrate_through(
    server: &Server,
    own: &Own,
    password: &Secret,
    migrations: &'static [Migration],
) -> Outcome<Applied> {
    let as_who = own.as_who();

    // The ledger before anything else, because it is what says whether anything else has to
    // run. Its own migration is `IF NOT EXISTS`, so this is a no-op on a database that has
    // one — and it is recorded below like any other migration rather than being special.
    let ledger = migrations
        .first()
        .ok_or_else(|| Failure::usage("there is no migration to run"))?;
    if !ledger_exists(server, &as_who, password)? {
        make::script(server, &as_who, Some(password), ledger.sql)
            .map_err(|failure| failure.hint("sloop's own database could not be given a ledger"))?;
    }

    let already = recorded(server, &as_who, password)?;
    let from = already
        .iter()
        .map(|(version, _)| *version)
        .max()
        .unwrap_or(0);

    // A version in the ledger that this build has never heard of. The database has been
    // opened by a newer sloop, and running an older one's migrations over it would be
    // rearranging a schema somebody else wrote.
    let newest_known = migrations.iter().map(|one| one.version).max().unwrap_or(0);
    if from > newest_known {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "{} is at schema version {from}, and this sloop only knows {newest_known}",
                own.database
            ),
        )
        .hint("upgrade sloop — an older one never migrates a newer schema"));
    }

    let mut ran = Vec::new();
    for migration in migrations {
        // Already run here. Its SQL has to still hash to what it hashed to then, or the file
        // has been edited since it shipped and what is in this database is not what the code
        // reading it believes is in it.
        if let Some((_, checksum)) = already
            .iter()
            .find(|(version, _)| *version == migration.version)
        {
            if checksum != &migration.checksum() {
                return Err(Failure::new(
                    Exit::Failure,
                    format!(
                        "migration {} ({}) has changed since it was applied to {}",
                        migration.version, migration.name, own.database
                    ),
                )
                .hint(
                    "a migration that has shipped is never edited — the change belongs in the \
                     next one",
                ));
            }
            continue;
        }

        // Not run here. The statements and the ledger row go in together, in one transaction,
        // so a migration that fails halfway leaves neither.
        let script = format!("{}\n{}", migration.sql, record_of(migration));
        make::script(server, &as_who, Some(password), &script).map_err(|failure| {
            failure.hint(format!(
                "migration {} ({}) did not apply; nothing from it was kept",
                migration.version, migration.name
            ))
        })?;
        ran.push(migration.name);
    }

    Ok(Applied {
        from,
        to: newest_known,
        ran,
    })
}

/// Fill in the `engine` table from the engines this build actually speaks.
///
/// **Reconciled rather than seeded in a migration**, so adding an engine to [`Engine::ALL`] is
/// a Rust change and not an SQL one — and so a build that stopped speaking one marks the row
/// unsupported instead of deleting something a year of history points at.
pub fn reconcile_engines(server: &Server, own: &Own, password: &Secret) -> Outcome<()> {
    use std::fmt::Write as _;

    let mut script = String::from("UPDATE engine SET supported = FALSE;\n");

    for engine in Engine::ALL {
        // Infallible: writing into a `String` cannot fail, and `write!` is only here because
        // it is the allocation-free spelling of the `push_str(&format!(…))` above it.
        let _ = write!(
            script,
            "INSERT INTO engine (name, display_name, default_port, supported)
                  VALUES ({}, {}, {}, TRUE)
             ON CONFLICT (name) DO UPDATE
                    SET display_name = EXCLUDED.display_name,
                        default_port = EXCLUDED.default_port,
                        supported    = TRUE;\n",
            literal(engine.scheme()),
            literal(engine.proper_name()),
            engine.default_port()
        );
    }

    make::script(server, &own.as_who(), Some(password), &script)
}

/// Does the ledger table exist?
fn ledger_exists(server: &Server, as_who: &As<'_>, password: &Secret) -> Outcome<bool> {
    let said = make::ask(
        server,
        as_who,
        Some(password),
        "SELECT to_regclass('public.schema_migration') IS NOT NULL;",
    )?;
    Ok(said.trim() == "t")
}

/// Every migration this database has had, as `(version, checksum)`.
fn recorded(server: &Server, as_who: &As<'_>, password: &Secret) -> Outcome<Vec<(i64, String)>> {
    let said = make::ask(
        server,
        as_who,
        Some(password),
        "SELECT version || ' ' || checksum FROM schema_migration ORDER BY version;",
    )?;

    said.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (version, checksum) = line.split_once(' ').ok_or_else(|| {
                Failure::new(
                    Exit::Failure,
                    format!("the migration ledger holds a row sloop cannot read: {line}"),
                )
            })?;
            let version: i64 = version.parse().map_err(|_| {
                Failure::new(
                    Exit::Failure,
                    format!("the migration ledger holds {version} where a number belongs"),
                )
            })?;
            Ok((version, checksum.to_owned()))
        })
        .collect()
}

/// The ledger row that goes in with a migration, in the same transaction as it.
fn record_of(migration: &Migration) -> String {
    format!(
        "INSERT INTO schema_migration (version, name, checksum, applied_by)
              VALUES ({}, {}, {}, {});",
        migration.version,
        literal(migration.name),
        literal(&migration.checksum()),
        literal(env!("CARGO_PKG_VERSION"))
    )
}

/// A string as an SQL literal.
///
/// **Nothing user-supplied reaches this yet, and this is what keeps that true when `R19c4`
/// does.** Doubling the quote is the whole of the escaping PostgreSQL needs while
/// `standard_conforming_strings` is on, which it has been by default since 9.1 and which
/// `initdb` leaves on — so the backslash in a real password is a backslash and not an escape.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Say what a run of [`migrate`] did, in the one line a report wants.
pub fn announce(applied: &Applied, own: &Own) {
    if applied.changed_anything() {
        crate::say!(
            "  {} {} — {}",
            crate::style::label("Schema"),
            crate::style::paint(&format!("version {}", applied.to)),
            crate::style::dim(&format!(
                "applied {} from version {}",
                applied.ran.join(", "),
                applied.from
            ))
        );
    } else {
        crate::say!(
            "  {} {}",
            crate::style::label("Schema"),
            crate::style::dim(&format!(
                "already at version {} in {}",
                applied.to, own.database
            ))
        );
    }
}
