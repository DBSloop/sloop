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

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use super::privileges::{self, Finding, Report, Verdict, verdict_from};
use super::{Adapter, Capabilities, Engine, ServerInfo, Table, TableCount, Target, Version};

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

    fn estimated_row_counts(&self, target: &Target<'_>) -> Outcome<Vec<TableCount>> {
        let rows = self.query(target, ESTIMATED_ROW_COUNTS_SQL)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(TableCount {
                    table: Table {
                        schema: row.first()?.clone(),
                        name: row.get(1)?.clone(),
                    },
                    // A table the statistics collector has never seen reports nothing
                    // useful, and zero is the honest reading of that. It is an estimate
                    // either way, and `verify` never fails a run on one.
                    rows: row.get(2).and_then(|value| value.parse().ok()).unwrap_or(0),
                })
            })
            .collect())
    }

    fn dump_into(&self, target: &Target<'_>, sink: &mut dyn std::io::Write) -> Outcome<()> {
        // Before anything is written: is this pg_dump even allowed to read that server?
        let server = self.probe(target)?;
        self.refuse_an_old_client(server.version)?;

        // **No `--file`, so the archive comes back on standard output.** Custom format is a
        // stream either way, and `pg_dump -Fc` to a pipe is what every `pg_dump | gzip`
        // does. Reading it here is what lets the bytes be encrypted on the way past without
        // a plaintext copy ever reaching the disk.
        let mut child = self
            .spawn_dump(target)
            .args(Self::connection_args(target))
            // Custom format: compressed, and the only format pg_restore can read
            // selectively and in parallel.
            .arg("--format=custom")
            // The destination has its own roles and its own grants. Carrying the source's
            // across is how a restore fails on a machine that never had them.
            .arg("--no-owner")
            .arg("--no-privileges")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| missing_tool(&self.tools.dump, &error))?;

        // Drained on a thread of its own: a full stderr pipe stops the child writing to
        // stdout, which would be a backup that hangs rather than one that fails.
        let mut complaining = child.stderr.take().expect("stderr was piped");
        let draining = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut complaining, &mut said);
            said
        });

        let copied = {
            let mut archive = child.stdout.take().expect("stdout was piped");
            std::io::copy(&mut archive, sink)
        };

        let status = child.wait().map_err(|error| {
            Failure::new(Exit::Dump, format!("pg_dump would not finish: {error}"))
        })?;
        let said = String::from_utf8_lossy(&draining.join().unwrap_or_default()).into_owned();

        if !status.success() {
            // The tool's own complaint outranks whatever went wrong writing the bytes: if
            // the dump failed, a short stream is the symptom and not the cause.
            return Err(from_stderr(&self.tools.dump, &said, Exit::Dump, target));
        }

        copied
            .map(|_| ())
            .map_err(|error| Failure::new(Exit::Dump, format!("could not write the dump: {error}")))
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

    fn terminate_connections(&self, target: &Target<'_>) -> Outcome<u64> {
        // From the maintenance database, so this session is not one of the ones being
        // counted — and `pid <> pg_backend_pid()` as well, in case somebody points the
        // maintenance connection at the same place.
        let maintenance = maintenance_target(target);
        let rows = self.query(
            &maintenance,
            &format!(
                "SELECT count(*) FROM (\
                   SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                    WHERE datname = {} AND pid <> pg_backend_pid()\
                 ) ended",
                sql_literal(target.database)
            ),
        )?;

        Ok(rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0))
    }

    fn drop_database(&self, target: &Target<'_>) -> Outcome<()> {
        // `IF EXISTS` is deliberately **not** used. `db drop` has already established that
        // this database is there, so a drop that finds nothing means something else has
        // changed underneath it — and silently succeeding at destroying nothing is how a
        // person ends up believing the wrong database is gone.
        let maintenance = maintenance_target(target);
        self.query(
            &maintenance,
            &format!("DROP DATABASE {}", quote_identifier(target.database)),
        )
        .map(|_| ())
    }

    fn check_privileges(&self, target: &Target<'_>) -> Outcome<Report> {
        let server = self.probe(target)?;
        let answers = self.query(target, PRIVILEGES_SQL)?;

        // Every row is `id | applicable | held | detail`, and the id is on the row rather
        // than implied by its position: a UNION ALL is not promised to come back in the
        // order it was written, and matching by index would be a bug that only appears
        // when somebody's planner decides otherwise.
        let answer = |id: &str| {
            answers
                .iter()
                .find(|row| row.first().is_some_and(|found| found == id))
        };

        let role = answer(WHOAMI)
            .and_then(|row| row.get(3))
            .cloned()
            .unwrap_or_else(|| target.user.to_owned());

        let findings = privileges::for_engine(Engine::Postgres)
            .iter()
            .map(|requirement| {
                let verdict = match answer(requirement.id) {
                    None => Verdict::Unknown(format!(
                        "this server did not answer for {}",
                        requirement.id
                    )),
                    Some(row) => {
                        let yes = |column: usize| row.get(column).is_some_and(|it| it == "t");
                        let detail = row.get(3).cloned().unwrap_or_default();
                        verdict_from(requirement.applies, yes(1), yes(2), detail)
                    }
                };
                Finding {
                    requirement,
                    verdict,
                }
            })
            .collect();

        Ok(Report {
            role,
            server,
            findings,
        })
    }
}

/// The row of [`PRIVILEGES_SQL`] that carries the role rather than a finding.
const WHOAMI: &str = "pg-whoami";

/// What this role can and cannot do, in one round trip.
///
/// Every branch asks the server the *effect* rather than the route: not "does this role
/// hold `pg_read_all_data`" but "is there a table it cannot read". A database whose owner
/// granted `SELECT` table by table passes, and so does one on PostgreSQL 13, which has no
/// such predefined role to hold.
///
/// Read-only throughout — catalogue lookups and `has_*_privilege` functions, which is
/// what lets `doctor` run this against a production source without a second thought.
const PRIVILEGES_SQL: &str = "\
WITH me AS (
  SELECT current_setting('is_superuser') = 'on' AS super,
         coalesce((SELECT rolbypassrls FROM pg_roles WHERE rolname = current_user), false) AS bypass
), rel AS (
  SELECT c.oid, n.nspname, c.relname, c.relrowsecurity, c.relforcerowsecurity, c.relowner
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
   WHERE c.relkind IN ('r','p','v','m','S')
     AND n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%'
), sch AS (
  SELECT n.nspname FROM pg_namespace n
   WHERE n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%'
     AND (n.nspname = 'public' OR EXISTS (SELECT 1 FROM rel r WHERE r.nspname = n.nspname))
), unread AS (
  SELECT count(*) AS bad, (SELECT count(*) FROM rel) AS total,
         coalesce(string_agg(nspname||'.'||relname, ', ' ORDER BY nspname, relname), '') AS names
    FROM rel
   WHERE NOT (has_schema_privilege(nspname,'USAGE') AND has_table_privilege(oid,'SELECT'))
), rls AS (
  SELECT count(*) AS total,
         count(*) FILTER (WHERE NOT ((SELECT bypass FROM me) OR (SELECT super FROM me)
              OR (pg_has_role(current_user, relowner, 'USAGE') AND NOT relforcerowsecurity))) AS bad,
         coalesce(string_agg(nspname||'.'||relname, ', ' ORDER BY nspname, relname), '') AS names
    FROM rel WHERE relrowsecurity
), lo AS (
  SELECT count(*) AS total,
         count(*) FILTER (WHERE NOT ((SELECT super FROM me)
              OR pg_has_role(current_user, m.lomowner, 'USAGE')
              OR (m.lomacl IS NOT NULL AND EXISTS (SELECT 1 FROM aclexplode(m.lomacl) a
                    WHERE a.privilege_type = 'SELECT'
                      AND (a.grantee = 0 OR pg_has_role(current_user, a.grantee, 'USAGE')))))) AS bad
    FROM pg_largeobject_metadata m
), nocreate AS (
  -- USAGE as well as CREATE. A role with only CREATE builds the tables and then fails on
  -- the first ALTER TABLE against one, because naming an object needs USAGE on its schema.
  SELECT count(*) AS bad,
         coalesce(string_agg(nspname, ', ' ORDER BY nspname), '') AS names
    FROM sch
   WHERE NOT (has_schema_privilege(nspname,'USAGE') AND has_schema_privilege(nspname,'CREATE'))
), ext AS (
  SELECT count(*) AS bad, coalesce(string_agg(e.extname, ', ' ORDER BY e.extname), '') AS names
    FROM pg_extension e
   WHERE e.extname <> 'plpgsql'
     AND NOT coalesce((SELECT v.trusted FROM pg_available_extension_versions v
                        WHERE v.name = e.extname AND v.version = e.extversion), false)
)
          SELECT 'pg-whoami', true, true, quote_ident(current_user)
UNION ALL SELECT 'pg-connect', true, has_database_privilege(current_database(),'CONNECT'), ''
UNION ALL SELECT 'pg-read-everything', true, u.bad = 0,
                 u.bad||' of '||u.total||' cannot be read: '||u.names FROM unread u
UNION ALL SELECT 'pg-bypass-row-security', r.total > 0, r.bad = 0,
                 r.bad||' with row-level security: '||r.names FROM rls r
UNION ALL SELECT 'pg-read-large-objects', l.total > 0, l.bad = 0,
                 l.bad||' of '||l.total||' cannot be opened' FROM lo l
UNION ALL SELECT 'pg-create-in-database', true, has_database_privilege(current_database(),'CREATE'), ''
UNION ALL SELECT 'pg-create-in-schemas', true, c.bad = 0, 'cannot enter or create in: '||c.names FROM nocreate c
UNION ALL SELECT 'pg-untrusted-extensions', e.bad > 0, (SELECT super FROM me), e.names FROM ext e";

/// Base tables, in every schema that is not the server's own.
const TABLES_SQL: &str = "\
SELECT n.nspname, c.relname \
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
WHERE c.relkind = 'r' AND n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%' \
ORDER BY 1, 2";

/// What the statistics collector thinks is in every table. Never a count.
///
/// `n_live_tup` is the number `CLAUDE.md` names, and it is a running tally the server keeps
/// rather than anything it has just measured. It is zero on a table nobody has touched
/// since the server started — which is every table in a database that has only just been
/// restored into — and it has been seen at double the truth on a table that had been
/// churned. The same `relkind` and schema filters as the exact query, so the two modes list
/// the same tables and a fast run cannot appear to lose one.
const ESTIMATED_ROW_COUNTS_SQL: &str = "\
SELECT n.nspname, c.relname, coalesce(s.n_live_tup, 0) \
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
LEFT JOIN pg_stat_all_tables s ON s.relid = c.oid \
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

/// The database PostgreSQL always has, for the statements that cannot be run from inside
/// the database they are about.
///
/// `postgres` rather than `template1`: dropping a database needs a connection somewhere
/// else, and `template1` is the one PostgreSQL copies to make new databases — connecting
/// to it blocks `CREATE DATABASE` for as long as the session lasts.
const MAINTENANCE_DATABASE: &str = "postgres";

/// The same connection, pointed at the maintenance database.
///
/// The role and its password are unchanged: dropping a database is something the account
/// that owns it does, and asking for a second credential to do it would be a second
/// credential to store.
fn maintenance_target<'a>(target: &'a Target<'a>) -> Target<'a> {
    Target {
        database: MAINTENANCE_DATABASE,
        ..*target
    }
}

/// Quote an identifier for PostgreSQL. Doubling the quotes is the whole rule.
///
/// A database name is not a literal and cannot be parameterised, so this is the only thing
/// standing between a name with a quote in it and a statement that means something else.
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Quote a string for PostgreSQL. Doubling the apostrophes is the whole rule.
fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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
    from_stderr(
        tool,
        &String::from_utf8_lossy(&output.stderr),
        otherwise,
        target,
    )
}

/// The same, where the complaint was collected from a pipe rather than by `output()`.
///
/// A dump that streams cannot use `Command::output` — its standard output is being read a
/// block at a time — so its stderr arrives as a string that was drained on a thread.
fn from_stderr(tool: &Path, said: &str, otherwise: Exit, target: &Target<'_>) -> Failure {
    let first = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it said nothing");

    let exit = if looks_like_a_connection_problem(said) {
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
