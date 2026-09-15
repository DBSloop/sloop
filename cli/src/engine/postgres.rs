//! PostgreSQL, over `pg_dump`, `pg_restore` and `psql`.
//!
//! Nothing here speaks the wire protocol. The client tools are the ones PostgreSQL ships
//! and tests against, and reimplementing twenty years of their edge cases is not work this
//! project wants to own.
//!
//! Three things every child process gets, without exception:
//!
//! - **`-w`**, so it fails rather than stopping to ask for a password. A scheduled run that
//!   waits forever on a prompt nobody can see is the worst failure this tool can have, and
//!   it is exactly what happens without this flag.
//! - **standard input closed**, for the same reason, one layer down.
//! - **`PGPASSWORD` in its own environment and nowhere wider**, because `argv` is readable
//!   by every other process on the machine and an environment is not.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Instant;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use super::{
    Adapter, Capabilities, DumpSummary, Engine, ServerInfo, Table, TableCount, Target, Version,
};

/// A separator that cannot turn up inside an identifier or a number.
const FIELD: &str = "\u{1f}";

/// Long enough to cross a slow link, short enough that a scheduled run does not sit on a
/// dead host until someone notices.
const CONNECT_TIMEOUT_SECONDS: &str = "10";

/// The most connections a restore will open, however many cores the machine has. Past this
/// the server is the bottleneck and the extra connections only cost it memory.
const MAX_RESTORE_JOBS: usize = 8;

/// Where the client tools are.
///
/// Bare names by default, found on `PATH`. R6 replaces that with real discovery — and with
/// the offer to go and fetch them — but the adapter only ever needs to know where they
/// ended up, so that work can arrive without touching this file.
#[derive(Debug, Clone)]
pub struct Tools {
    /// `pg_dump`.
    pub dump: PathBuf,
    /// `pg_restore`.
    pub restore: PathBuf,
    /// `psql`.
    pub query: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            dump: PathBuf::from("pg_dump"),
            restore: PathBuf::from("pg_restore"),
            query: PathBuf::from("psql"),
        }
    }
}

/// The PostgreSQL adapter.
pub struct Postgres {
    tools: Tools,
    /// Set only by tests, to stand an old client in front of a current server without
    /// installing one.
    pretend_client_version: Option<Version>,
}

impl Default for Postgres {
    fn default() -> Self {
        Self::new(Tools::default())
    }
}

impl Postgres {
    /// An adapter that runs the tools at these paths.
    #[must_use]
    pub const fn new(tools: Tools) -> Self {
        Self {
            tools,
            pretend_client_version: None,
        }
    }

    /// Answer a version question with this instead of asking `pg_dump`.
    #[cfg(test)]
    #[must_use]
    pub const fn pretending_to_be(mut self, version: Version) -> Self {
        self.pretend_client_version = Some(version);
        self
    }

    /// What `pg_dump --version` says.
    pub fn client_version(&self) -> Outcome<Version> {
        if let Some(pretended) = self.pretend_client_version {
            return Ok(pretended);
        }

        let output = Command::new(&self.tools.dump)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| missing_tool(&self.tools.dump, &error))?;

        let said = String::from_utf8_lossy(&output.stdout);
        Version::from_tool_output(&said).ok_or_else(|| {
            Failure::new(
                Exit::Usage,
                format!(
                    "could not tell what version {} is: {}",
                    self.tools.dump.display(),
                    said.trim()
                ),
            )
        })
    }

    /// Refuse a dump that PostgreSQL itself would refuse.
    ///
    /// `pg_dump` will not read a server newer than itself — the catalogue it is reading has
    /// changed shape underneath it. Catching that here turns a confusing tool error late in
    /// a backup into a sentence before anything starts.
    ///
    /// Exit `2` rather than `4`: no dump was attempted, and nothing about retrying it will
    /// help. This is a machine that needs different tools installed.
    pub(super) fn refuse_an_old_client(&self, server: Version) -> Outcome<()> {
        let client = self.client_version()?;
        if client.major >= server.major {
            return Ok(());
        }

        Err(Failure::new(
            Exit::Usage,
            format!(
                "pg_dump is {client} and the server is {server}. PostgreSQL will not let an \
                 older pg_dump read a newer server"
            ),
        )
        .hint(format!(
            "install PostgreSQL client tools {} or newer; `sloop doctor` reports what this \
             machine has",
            server.major
        )))
    }

    /// The flags that say which server, in every invocation.
    fn connection_args(target: &Target<'_>) -> Vec<String> {
        vec![
            "--host".to_owned(),
            target.host.to_owned(),
            "--port".to_owned(),
            target.port.to_string(),
            "--username".to_owned(),
            target.user.to_owned(),
            "--dbname".to_owned(),
            target.database.to_owned(),
            // Never stop to ask for a password. See the module comment.
            "--no-password".to_owned(),
        ]
    }

    /// A child process with the password in its environment and nowhere else.
    fn spawn(program: &Path, target: &Target<'_>) -> Command {
        let mut command = Command::new(program);

        command
            .stdin(Stdio::null())
            .env("PGPASSWORD", target.password.expose())
            // TLS when the server offers it, plain when it does not. Set rather than left
            // to libpq's default so a `PGSSLMODE` exported in someone's shell cannot turn
            // a working backup into a refusal.
            .env("PGSSLMODE", "prefer")
            .env("PGCONNECT_TIMEOUT", CONNECT_TIMEOUT_SECONDS)
            .env("PGCLIENTENCODING", "UTF8")
            // So a DBA looking at `pg_stat_activity` can see who this is.
            .env("PGAPPNAME", "sloop");

        // Anything that could quietly redirect the connection somewhere else. The flags
        // above already win over most of these; a service file does not.
        for redirect in [
            "PGSERVICE",
            "PGSERVICEFILE",
            "PGPASSFILE",
            "PGOPTIONS",
            "PGDATABASE",
            "PGHOST",
            "PGHOSTADDR",
            "PGPORT",
            "PGUSER",
        ] {
            command.env_remove(redirect);
        }

        command
    }

    fn spawn_dump(&self, target: &Target<'_>) -> Command {
        Self::spawn(&self.tools.dump, target)
    }

    fn spawn_restore(&self, target: &Target<'_>) -> Command {
        Self::spawn(&self.tools.restore, target)
    }

    /// Run a read-only statement and hand back its rows.
    fn query(&self, target: &Target<'_>, sql: &str) -> Outcome<Vec<Vec<String>>> {
        let mut command = Self::spawn(&self.tools.query, target);
        command
            .args(Self::connection_args(target))
            .arg("--no-psqlrc")
            .arg("--quiet")
            .arg("--tuples-only")
            .arg("--no-align")
            .arg("--field-separator")
            .arg(FIELD)
            .arg("--variable")
            .arg("ON_ERROR_STOP=1")
            .arg("--command")
            .arg(sql);

        let output = command
            .output()
            .map_err(|error| missing_tool(&self.tools.query, &error))?;

        if !output.status.success() {
            return Err(from_tool(&self.tools.query, &output, Exit::Connect, target));
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(|line| line.split(FIELD).map(str::to_owned).collect())
            .collect())
    }
}

impl Adapter for Postgres {
    fn engine(&self) -> Engine {
        Engine::Postgres
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            custom_format: true,
            parallel_restore: true,
        }
    }

    fn probe(&self, target: &Target<'_>) -> Outcome<ServerInfo> {
        let rows = self.query(
            target,
            "SELECT current_setting('server_version_num'), \
             coalesce((SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()), false)",
        )?;

        let row = rows.first().ok_or_else(|| {
            Failure::new(
                Exit::Connect,
                format!(
                    "{} answered nothing when asked its version",
                    target.describe()
                ),
            )
        })?;

        let number: u32 = row
            .first()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| {
                Failure::new(
                    Exit::Connect,
                    format!(
                        "{} gave a version this sloop cannot read",
                        target.describe()
                    ),
                )
            })?;

        Ok(ServerInfo {
            engine: Engine::Postgres,
            version: Version::from_server_version_num(number),
            tls: matches!(row.get(1).map(String::as_str), Some("t" | "true" | "on")),
        })
    }

    fn tables(&self, target: &Target<'_>) -> Outcome<Vec<Table>> {
        let rows = self.query(target, TABLES_SQL)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(Table {
                    schema: row.first()?.clone(),
                    name: row.get(1)?.clone(),
                })
            })
            .collect())
    }

    fn row_counts(&self, target: &Target<'_>) -> Outcome<Vec<TableCount>> {
        let rows = self.query(target, ROW_COUNTS_SQL)?;
        rows.into_iter()
            .map(|row| {
                let table = Table {
                    schema: row.first().cloned().unwrap_or_default(),
                    name: row.get(1).cloned().unwrap_or_default(),
                };
                let rows: u64 =
                    row.get(2)
                        .and_then(|value| value.parse().ok())
                        .ok_or_else(|| {
                            Failure::new(
                                Exit::Mismatch,
                                format!("could not read a row count for {table}"),
                            )
                        })?;
                Ok(TableCount { table, rows })
            })
            .collect()
    }

    fn dump(&self, target: &Target<'_>, to: &Path) -> Outcome<DumpSummary> {
        // Before anything is written: is this pg_dump even allowed to read that server?
        let server = self.probe(target)?;
        self.refuse_an_old_client(server.version)?;

        if let Some(parent) = to.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| {
                Failure::new(
                    Exit::Dump,
                    format!("could not create {}: {error}", parent.display()),
                )
            })?;
        }

        let started = Instant::now();
        let output = self
            .spawn_dump(target)
            .args(Self::connection_args(target))
            // Custom format: compressed, and the only format pg_restore can read
            // selectively and in parallel.
            .arg("--format=custom")
            // The destination has its own roles and its own grants. Carrying the source's
            // across is how a restore fails on a machine that never had them.
            .arg("--no-owner")
            .arg("--no-privileges")
            .arg("--file")
            .arg(to)
            .output()
            .map_err(|error| missing_tool(&self.tools.dump, &error))?;

        if !output.status.success() {
            return Err(from_tool(&self.tools.dump, &output, Exit::Dump, target));
        }

        let bytes = std::fs::metadata(to)
            .map(|meta| meta.len())
            .map_err(|error| {
                Failure::new(
                    Exit::Dump,
                    format!(
                        "pg_dump reported success but {} is not there: {error}",
                        to.display()
                    ),
                )
            })?;

        if bytes == 0 {
            return Err(Failure::new(
                Exit::Dump,
                format!("pg_dump wrote nothing to {}", to.display()),
            ));
        }

        Ok(DumpSummary {
            path: to.to_path_buf(),
            bytes,
            took: started.elapsed(),
        })
    }

    fn restore(&self, target: &Target<'_>, from: &Path) -> Outcome<()> {
        if !from.is_file() {
            return Err(Failure::new(
                Exit::Restore,
                format!("there is no dump at {}", from.display()),
            ));
        }

        let output = self
            .spawn_restore(target)
            .args(Self::connection_args(target))
            .arg("--no-owner")
            .arg("--no-privileges")
            // Without this pg_restore prints warnings, carries on, and exits 0 with a
            // half-restored database. A restore that partly worked is a failure.
            .arg("--exit-on-error")
            .arg(format!("--jobs={}", restore_jobs()))
            .arg(from)
            .output()
            .map_err(|error| missing_tool(&self.tools.restore, &error))?;

        if !output.status.success() {
            return Err(from_tool(
                &self.tools.restore,
                &output,
                Exit::Restore,
                target,
            ));
        }

        Ok(())
    }
}

/// Base tables, in every schema that is not the server's own.
const TABLES_SQL: &str = "\
SELECT n.nspname, c.relname \
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
WHERE c.relkind = 'r' AND n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%' \
ORDER BY 1, 2";

/// Every table's exact `count(*)`, in one statement.
///
/// `query_to_xml` runs a query per table inside the one round trip, so this is one
/// statement against one snapshot rather than a count and then another count and then a
/// third. `n_live_tup` would be faster and is a planner estimate — it has been seen
/// reporting double the truth, which is why nothing here goes near it.
///
/// `relkind = 'r'` only: a partitioned table's rows live in its partitions, which are
/// ordinary tables, so counting the parent as well would count them twice.
const ROW_COUNTS_SQL: &str = "\
SELECT n.nspname, c.relname, \
(xpath('/row/cnt/text()', query_to_xml(format('select count(*) as cnt from %I.%I', n.nspname, c.relname), false, true, '')))[1]::text::bigint \
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
WHERE c.relkind = 'r' AND n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%' \
ORDER BY 1, 2";

fn restore_jobs() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .clamp(1, MAX_RESTORE_JOBS)
}

/// The tool is not on this machine, or not where we were told it was.
fn missing_tool(tool: &Path, error: &std::io::Error) -> Failure {
    Failure::new(
        Exit::Usage,
        format!("could not run {}: {error}", tool.display()),
    )
    .hint("PostgreSQL's client tools are not bundled with sloop. `sloop doctor` reports what this machine has")
}

/// Turn a tool's own complaint into a failure with the right code on it.
///
/// A connection problem is `3` whatever was being attempted when it happened, because that
/// is the distinction a scheduler acts on: a server that is down will be up later, and a
/// dump that failed on its own terms will not fix itself.
fn from_tool(tool: &Path, output: &Output, otherwise: Exit, target: &Target<'_>) -> Failure {
    let said = String::from_utf8_lossy(&output.stderr);
    let first = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it said nothing");

    let exit = if looks_like_a_connection_problem(&said) {
        Exit::Connect
    } else {
        otherwise
    };

    Failure::new(
        exit,
        format!(
            "{} failed against {}: {first}",
            tool.file_name()
                .unwrap_or(tool.as_os_str())
                .to_string_lossy(),
            target.describe()
        ),
    )
}

/// Does this read like the server was unreachable rather than unhappy?
fn looks_like_a_connection_problem(stderr: &str) -> bool {
    let said = stderr.to_ascii_lowercase();
    [
        "could not connect",
        "connection refused",
        "connection to server",
        "authentication failed",
        "no pg_hba.conf entry",
        "could not translate host name",
        "timeout expired",
        "the database system is starting up",
        "server closed the connection unexpectedly",
        "role \"",
        "database \"",
    ]
    .iter()
    .any(|marker| said.contains(marker))
}
