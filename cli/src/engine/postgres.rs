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
use super::{
    Adapter, Capabilities, Cell, Engine, ForeignKey, Merged, Provisioned, Provisioning, Reading,
    Rows, ServerInfo, Table, TableCount, TableShape, Target, Version,
};

/// A separator that cannot turn up inside an identifier or a number.
const FIELD: &str = "\u{1f}";

/// The separator *inside* one of those fields, for a column list gathered into one value.
const INNER: &str = "\u{1e}";

/// And the one between a schema and a table name inside an entry of such a list.
const PAIR: &str = "\u{1d}";

/// The staging table a merge lands in. Temporary, so it belongs to the session and goes
/// with it — a run that is killed leaves nothing to find.
const STAGING: &str = "sloop_merging";

/// How the counts a merge needs are picked out of everything else psql prints.
const MARKER: &str = "sloop-merge";

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
            .hint("`sloop doctor` reports which client tools sloop found and where")
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

    /// Run a script, fed on standard input.
    ///
    /// **Not `--command`, and that is the whole reason this exists.** `psql --command` puts
    /// the statement in `argv`, where every other process on the machine can read it — and
    /// `CREATE ROLE … PASSWORD '…'` is a statement with a password in its text. Standard
    /// input is a pipe between two processes and is readable by neither.
    fn execute(&self, target: &Target<'_>, sql: &str) -> Outcome<()> {
        let mut child = Self::spawn(&self.tools.query, target)
            .args(Self::connection_args(target))
            .arg("--no-psqlrc")
            .arg("--quiet")
            .arg("--variable")
            .arg("ON_ERROR_STOP=1")
            // `-` is standard input. Without it psql would read the script and still be
            // waiting for a terminal that is not there.
            .arg("--file")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| missing_tool(&self.tools.query, &error))?;

        let mut complaining = child.stderr.take().expect("stderr was piped");
        let draining = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut complaining, &mut said);
            said
        });
        let mut talking = child.stdout.take().expect("stdout was piped");
        let listening = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut talking, &mut said);
        });

        {
            use std::io::Write as _;
            let mut sink = child.stdin.take().expect("stdin was piped");
            let _ = sink.write_all(sql.as_bytes());
            // Dropped here, so psql sees the end of the script rather than waiting.
        }

        let status = child.wait().map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("{} would not finish: {error}", self.tools.query.display()),
            )
        })?;
        let said = String::from_utf8_lossy(&draining.join().unwrap_or_default()).into_owned();
        let _ = listening.join();

        if !status.success() {
            return Err(from_stderr(&self.tools.query, &said, Exit::Usage, target));
        }

        Ok(())
    }

    /// Land the source's rows in a temporary table, count what is about to happen, and
    /// upsert.
    ///
    /// **One session, one transaction, one temporary table that the session owns.** A merge
    /// that is killed halfway leaves nothing to find — the staging table goes with the
    /// connection, and the transaction means the destination is either as it was or as it
    /// should be, never half of each.
    ///
    /// **The numbers come back out of the same script that does the work**, marked so they
    /// can be picked out of psql's output, because a second connection asking afterwards
    /// would be asking about a different moment.
    fn merge_through_a_temporary_table(
        &self,
        source: &Target<'_>,
        destination: &Target<'_>,
        shape: &TableShape,
        table: &str,
        columns: &[String],
        mut reading: std::process::Child,
    ) -> Outcome<Merged> {
        let (header, footer) = merge_script(shape, table, columns);

        let mut writing = Self::spawn(&self.tools.query, destination)
            .args(Self::connection_args(destination))
            .arg("--no-psqlrc")
            .arg("--quiet")
            .arg("--tuples-only")
            .arg("--no-align")
            .arg("--field-separator")
            .arg(FIELD)
            .arg("--variable")
            .arg("ON_ERROR_STOP=1")
            .arg("--file")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                let _ = reading.kill();
                let _ = reading.wait();
                missing_tool(&self.tools.query, &error)
            })?;

        // Both children's stderr is drained on threads of its own, for the reason
        // `copy_into` gives: a full pipe stops the process that owns it, and either one
        // stopping deadlocks the other end of the stream.
        let mut source_said = reading.stderr.take().expect("stderr was piped");
        let source_saying = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut source_said, &mut said);
            said
        });

        // **The rows pass through this process and never become a file.** `COPY … FROM
        // STDIN` takes its data inline in the script it is part of, so there is no second
        // stream to hand psql and no OS pipe that could carry both. A copy is not a backup —
        // so what is between the two servers is a buffer, not a disk.
        let streamed = {
            use std::io::Write as _;
            let mut sink = writing.stdin.take().expect("stdin was piped");
            let mut rows = reading.stdout.take().expect("stdout was piped");
            sink.write_all(header.as_bytes())
                .and_then(|()| std::io::copy(&mut rows, &mut sink).map(|_| ()))
                .and_then(|()| sink.write_all(footer.as_bytes()))
        };

        let written = writing.wait_with_output().map_err(|error| {
            Failure::new(
                Exit::Restore,
                format!("{} would not finish: {error}", self.tools.query.display()),
            )
        })?;
        let read = reading
            .wait()
            .map_err(|error| Failure::new(Exit::Dump, format!("psql would not finish: {error}")))?;
        let complained = String::from_utf8_lossy(&source_saying.join().unwrap_or_default())
            .trim()
            .to_owned();

        // The source's complaint first: a short stream is the symptom when the read failed,
        // and reporting the write's confusion about it would name the wrong end.
        if !read.success() {
            return Err(from_stderr(
                &self.tools.query,
                &complained,
                Exit::Dump,
                source,
            ));
        }
        if !written.status.success() {
            return Err(from_tool(
                &self.tools.query,
                &written,
                Exit::Restore,
                destination,
            ));
        }
        streamed.map_err(|error| {
            Failure::new(Exit::Restore, format!("the rows stopped moving: {error}"))
        })?;

        Merged::from_markers(
            &shape.table,
            &String::from_utf8_lossy(&written.stdout),
            FIELD,
        )
    }

    /// Stream `pg_dump` straight into `pg_restore`, with whatever extra flags the caller
    /// needs on the dump.
    ///
    /// **One OS pipe between two client processes**, which is the same thing
    /// `pg_dump … | pg_restore …` is at a shell prompt: the archive never becomes a file and
    /// never passes through sloop's memory either.
    fn stream_a_dump(
        &self,
        source: &Target<'_>,
        destination: &Target<'_>,
        extra: &[&str],
    ) -> Outcome<()> {
        // Before anything is spawned: is this pg_dump even allowed to read that server?
        let server = self.probe(source)?;
        self.refuse_an_old_client(server.version)?;

        let mut dumping = self
            .spawn_dump(source)
            .args(Self::connection_args(source))
            .arg("--format=custom")
            .arg("--no-owner")
            .arg("--no-privileges")
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| missing_tool(&self.tools.dump, &error))?;

        // **The archive goes down an OS pipe, not through this process.** The dump's own
        // standard output becomes the restore's standard input, which is what
        // `pg_dump | pg_restore` is at a shell prompt.
        let archive = dumping.stdout.take().expect("stdout was piped");
        let restoring = self
            .spawn_restore(destination)
            .args(Self::connection_args(destination))
            .arg("--no-owner")
            .arg("--no-privileges")
            // Without this pg_restore prints warnings, carries on, and exits 0 with a
            // half-restored database. A copy that partly worked is a failure.
            .arg("--exit-on-error")
            .stdin(Stdio::from(archive))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let restoring = match restoring {
            Ok(child) => child,
            Err(error) => {
                // The dump is already running and writing into a pipe nothing will read.
                let _ = dumping.kill();
                let _ = dumping.wait();
                return Err(missing_tool(&self.tools.restore, &error));
            }
        };

        // Drained while both run: a full stderr pipe stops the process that owns it, and
        // either one stopping deadlocks the other end of the archive.
        let mut complaining = dumping.stderr.take().expect("stderr was piped");
        let dump_said = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut complaining, &mut said);
            said
        });

        let restored = restoring.wait_with_output().map_err(|error| {
            Failure::new(
                Exit::Restore,
                format!("{} would not finish: {error}", self.tools.restore.display()),
            )
        })?;
        let dumped = dumping.wait().map_err(|error| {
            Failure::new(
                Exit::Dump,
                format!("{} would not finish: {error}", self.tools.dump.display()),
            )
        })?;
        let dump_complaint =
            String::from_utf8_lossy(&dump_said.join().unwrap_or_default()).into_owned();

        // **The dump's complaint outranks the restore's.** A restore fed half an archive
        // fails at the archive, and reporting that would send somebody to the wrong end of
        // the copy.
        if !dumped.success() {
            return Err(from_stderr(
                &self.tools.dump,
                &dump_complaint,
                Exit::Dump,
                source,
            ));
        }
        if !restored.status.success() {
            return Err(from_tool(
                &self.tools.restore,
                &restored,
                Exit::Restore,
                destination,
            ));
        }

        Ok(())
    }

    /// Is there a row in `catalogue` whose `column` is `name`?
    ///
    /// The name goes in as a literal rather than being interpolated into an identifier: it
    /// came from a command line and this is a question, not a statement about it.
    fn exists(
        &self,
        target: &Target<'_>,
        catalogue: &str,
        column: &str,
        name: &str,
    ) -> Outcome<bool> {
        let rows = self.query(
            target,
            &format!(
                "SELECT 1 FROM {catalogue} WHERE {column} = {}",
                sql_literal(name)
            ),
        )?;
        Ok(!rows.is_empty())
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
            .hint("check it answers at all: `sloop db test <name>`")
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
                .hint(
                    "sloop speaks to PostgreSQL; a pooler in front of one can answer for it \
                     in a shape sloop does not know",
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
                            .hint(
                                "verification counts every row on both sides; `VERIFY=fast` \
                                 skips the count and reports the planner's estimate instead",
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

    /// **One statement, off `pg_stat_database` and `pg_database_size`.**
    ///
    /// `tup_inserted + tup_updated + tup_deleted` is what went in and
    /// `tup_returned + tup_fetched` is what came out. Neither is a byte count and neither
    /// pretends to be: `blks_read` is disk blocks rather than network, and there is no
    /// per-database network counter to read.
    ///
    /// **A row that is not there is `None`.** `pg_stat_database` has no row for a database
    /// nothing has connected to since the statistics were reset, and "zero rows moved" is not
    /// the same claim as "the server would not say".
    fn activity(&self, target: &Target<'_>) -> Outcome<super::Activity> {
        let rows = self.query(target, ACTIVITY_SQL)?;
        let Some(row) = rows.first() else {
            return Ok(super::Activity::default());
        };

        Ok(super::Activity {
            rows_in: number(row.first()),
            rows_out: number(row.get(1)),
            size_bytes: number(row.get(2)),
            // Nothing to explain: `pg_stat_database` is readable by anybody who can connect,
            // and a row that is not there means nothing has connected since the statistics
            // were reset — which the next reading fixes by itself.
            note: None,
        })
    }

    fn dump_into(
        &self,
        target: &Target<'_>,
        sink: &mut dyn std::io::Write,
        only: &[Table],
    ) -> Outcome<()> {
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
            .args(only.iter().map(table_pattern))
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
            )
            .hint("`sloop backups list` shows the backups that are there"));
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

    fn restore_into(&self, target: &Target<'_>, source: &mut dyn std::io::Read) -> Outcome<()> {
        // **No `--jobs`.** A parallel restore seeks around inside the archive, and this one
        // is arriving down a pipe one byte at a time. The trade is deliberate and is the
        // reason [`Adapter::restore`] still exists: an encrypted backup restores
        // single-threaded and never becomes a file, an unencrypted one is already a file.
        let mut child = self
            .spawn_restore(target)
            .args(Self::connection_args(target))
            .arg("--no-owner")
            .arg("--no-privileges")
            // Without this pg_restore prints warnings, carries on, and exits 0 with a
            // half-restored database. A restore that partly worked is a failure.
            .arg("--exit-on-error")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| missing_tool(&self.tools.restore, &error))?;

        // Both of the child's output pipes are drained on threads of their own. A full pipe
        // stops the child reading its standard input, and this call is the thing writing to
        // that input — so not draining them is a restore that hangs rather than one that
        // fails.
        let mut complaining = child.stderr.take().expect("stderr was piped");
        let draining = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut complaining, &mut said);
            said
        });
        let mut talking = child.stdout.take().expect("stdout was piped");
        let listening = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut talking, &mut said);
        });

        let fed = {
            let mut sink = child.stdin.take().expect("stdin was piped");
            let copied = std::io::copy(source, &mut sink);
            // Dropped before the wait, so the child sees the end of its input rather than
            // waiting for more of an archive that has all arrived.
            drop(sink);
            copied
        };

        let status = child.wait().map_err(|error| {
            Failure::new(
                Exit::Restore,
                format!("{} would not finish: {error}", self.tools.restore.display()),
            )
        })?;
        let said = String::from_utf8_lossy(&draining.join().unwrap_or_default()).into_owned();
        let _ = listening.join();

        if !status.success() {
            // The tool's own complaint outranks a write that failed: if the restore died,
            // a broken pipe is the symptom rather than the cause.
            return Err(from_stderr(
                &self.tools.restore,
                &said,
                Exit::Restore,
                target,
            ));
        }

        fed.map(|_| ()).map_err(|error| {
            Failure::new(
                Exit::Restore,
                format!("could not feed the dump in: {error}"),
            )
        })
    }

    fn copy_schema_into(
        &self,
        source: &Target<'_>,
        destination: &Target<'_>,
        only: &[Table],
    ) -> Outcome<()> {
        // `--schema-only` is the whole difference from `copy_into`, and `--no-owner` and
        // `--no-privileges` are there for the same reason they are there: the destination has
        // its own role, and the source's does not exist on it.
        //
        // **`--table` takes a pattern, and a quoted one is a literal.** Unquoted, `pg_dump`
        // folds the name to lower case and reads `.`, `*` and `?` as syntax — so a table
        // called `Orders.2024` would match nothing at all.
        let mut named: Vec<String> = vec!["--schema-only".to_owned()];
        named.extend(only.iter().map(|table| {
            format!(
                "--table={}.{}",
                quote_identifier(&table.schema),
                quote_identifier(&table.name)
            )
        }));

        let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
        self.stream_a_dump(source, destination, &borrowed)
    }

    fn copy_into(&self, source: &Target<'_>, destination: &Target<'_>) -> Outcome<()> {
        self.stream_a_dump(source, destination, &[])
    }

    fn copy_tables_into(
        &self,
        source: &Target<'_>,
        destination: &Target<'_>,
        only: &[Table],
    ) -> Outcome<()> {
        let named: Vec<String> = only.iter().map(table_pattern).collect();
        let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
        self.stream_a_dump(source, destination, &borrowed)
    }

    fn drop_tables(&self, target: &Target<'_>, tables: &[Table]) -> Outcome<u64> {
        let mut gone = 0;
        for table in tables {
            // `IF EXISTS` rather than a look-up first: a scoped mirror into a destination
            // that never had one of the named tables is an ordinary run, not a failure.
            self.query(
                target,
                &format!(
                    "DROP TABLE IF EXISTS {}.{}",
                    quote_identifier(&table.schema),
                    quote_identifier(&table.name)
                ),
            )
            .map_err(|failure| {
                failure.at(Exit::Restore).hint(
                    "something outside this list may still point at it. Name that table too,                      or mirror the whole database",
                )
            })?;
            gone += 1;
        }
        Ok(gone)
    }

    fn shapes(&self, target: &Target<'_>) -> Outcome<Vec<TableShape>> {
        let rows = self.query(target, SHAPES_SQL)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let split = |at: usize| -> Vec<String> {
                    row.get(at)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.split(INNER).map(str::to_owned).collect())
                        .unwrap_or_default()
                };
                Some(TableShape {
                    table: Table {
                        schema: row.first()?.clone(),
                        name: row.get(1)?.clone(),
                    },
                    columns: split(2),
                    server_assigned: row.get(3).is_some_and(|value| value == "t"),
                    primary_key: split(4),
                    references: split(5)
                        .iter()
                        .filter_map(|parent| {
                            let (schema, name) = parent.split_once(PAIR)?;
                            Some(Table {
                                schema: schema.to_owned(),
                                name: name.to_owned(),
                            })
                        })
                        .collect(),
                })
            })
            .collect())
    }

    fn merge_table(
        &self,
        source: &Target<'_>,
        destination: &Target<'_>,
        shape: &TableShape,
    ) -> Outcome<Merged> {
        let table = format!(
            "{}.{}",
            quote_identifier(&shape.table.schema),
            quote_identifier(&shape.table.name)
        );
        let columns: Vec<String> = shape
            .columns
            .iter()
            .map(|name| quote_identifier(name))
            .collect();
        let listed = columns.join(", ");

        // **One `SELECT`, and it is everything this method ever sends to the source.** Rule
        // 6, and the column list is the source's own, so a column only the destination has
        // is not named, not written, and not lost.
        let reading = Self::spawn(&self.tools.query, source)
            .args(Self::connection_args(source))
            .arg("--no-psqlrc")
            .arg("--quiet")
            .arg("--variable")
            .arg("ON_ERROR_STOP=1")
            .arg("--command")
            .arg(format!("COPY (SELECT {listed} FROM {table}) TO STDOUT"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| missing_tool(&self.tools.query, &error))?;

        self.merge_through_a_temporary_table(source, destination, shape, &table, &columns, reading)
            .map_err(|failure| failure.prefixed(&shape.table))
    }

    fn reset_sequences(&self, target: &Target<'_>) -> Outcome<Vec<String>> {
        let rows = self
            .query(target, RESET_SEQUENCES_SQL)
            .map_err(|failure| failure.at(Exit::Restore))?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let sequence = row.first()?;
                let next = row.get(1)?;
                Some(format!("{sequence} now hands out {next} next"))
            })
            .collect())
    }

    fn provision(&self, target: &Target<'_>, asked: &Provisioning<'_>) -> Outcome<Provisioned> {
        // The script is built in a `String`, which takes `fmt::Write`.
        use std::fmt::Write as _;

        // **Refused before anything is created.** A database that is already there belongs
        // to somebody, and quietly reusing it is how a `db create` typo ends up pointed at
        // production.
        if self.exists(target, "pg_database", "datname", asked.database)? {
            return Err(Failure::usage(format!(
                "{} already exists on {}",
                asked.database,
                target.describe()
            ))
            .hint("`sloop db add` registers a database that is already there"));
        }

        let role_existed = self.exists(target, "pg_roles", "rolname", asked.role)?;
        let role = quote_identifier(asked.role);
        let database = quote_identifier(asked.database);

        // One script, down standard input, so the password in it never reaches `argv`.
        let mut script = String::new();
        if role_existed {
            // A role that was already there keeps the password it has. Changing somebody
            // else's credentials because a name collided is not this command's to do, and
            // the command says so where it prints what it did.
            let _ = writeln!(script, "-- {} was already there", asked.role);
        } else {
            let _ = writeln!(
                script,
                "CREATE ROLE {role} LOGIN PASSWORD {};",
                sql_literal(asked.password.expose())
            );
        }
        let _ = writeln!(script, "CREATE DATABASE {database} OWNER {role};");
        self.execute(target, &script)?;

        // The rest runs *inside* the new database, because `public` is a schema in it.
        let inside = Target {
            database: asked.database,
            ..*target
        };
        let grants = vec![
            format!("USAGE, CREATE ON SCHEMA public TO {}", asked.role),
            format!(
                "default privileges on tables and sequences to {}",
                asked.role
            ),
        ];
        self.execute(
            &inside,
            &format!(
                // Owning the database is not the same as being able to create in `public`:
                // before PostgreSQL 15 that schema belongs to the bootstrap superuser, and
                // from 15 it belongs to `pg_database_owner`. Granting explicitly is correct
                // on both and costs nothing on the one where it was already true.
                "GRANT USAGE, CREATE ON SCHEMA public TO {role};\n\
                 ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO {role};\n\
                 ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO {role};\n"
            ),
        )?;

        Ok(Provisioned {
            role_existed,
            grants,
        })
    }

    fn clear_contents(&self, target: &Target<'_>) -> Outcome<u64> {
        let had = self.tables(target)?.len();

        // Every schema that is not the system's. `pg_catalog`, `information_schema` and
        // anything else starting `pg_` belong to PostgreSQL; the rest is what a dump of this
        // database would have carried, so the rest is what a restore has to replace.
        let schemas = self
            .query(
                target,
                // `left(nspname, 3)` rather than `LIKE 'pg\_%'`: the underscore is a
                // wildcard in `LIKE` and would have to be escaped, and an escape inside a
                // Rust string inside a SQL literal is three readings of the same character
                // for no gain.
                "SELECT nspname FROM pg_namespace \
                 WHERE left(nspname, 3) <> 'pg_' AND nspname <> 'information_schema' \
                 ORDER BY nspname",
            )
            .map_err(|failure| failure.at(Exit::Restore))?;

        for row in &schemas {
            let Some(schema) = row.first() else { continue };
            self.query(
                target,
                &format!("DROP SCHEMA {} CASCADE", quote_identifier(schema)),
            )
            .map_err(|failure| failure.at(Exit::Restore))?;
        }

        // **`public` goes back.** A database with no `public` is not an empty database, it
        // is a broken one: a dump that does not create the schema itself will fail to
        // restore into it, and anything else connecting afterwards finds a search path
        // pointing at nothing.
        self.query(target, "CREATE SCHEMA IF NOT EXISTS public")
            .map_err(|failure| failure.at(Exit::Restore))?;

        Ok(u64::try_from(had).unwrap_or(u64::MAX))
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

    fn read(&self, target: &Target<'_>, asked: &Reading<'_>) -> Outcome<Rows> {
        // **A statement of psql's own is not a statement.** Given a string beginning with a
        // backslash, `--command` reads it as a meta-command rather than sending it to the
        // server — and `\!` runs a program. `--sql` promises SQL, so anything that is not
        // SQL is a refusal here rather than a surprise on the way to a shell.
        let sql = asked.sql.trim();
        if sql.starts_with('\\') {
            return Err(Failure::new(
                Exit::Usage,
                "that is a psql command rather than SQL, and sloop only ever sends SQL",
            )
            .hint("run it in psql if that is what you meant"));
        }

        let mut command = Self::spawn(&self.tools.query, target);
        command
            .args(Self::connection_args(target))
            .arg("--no-psqlrc")
            .arg("--quiet")
            .arg("--no-align")
            .args(["--pset", "footer=off"])
            .args(["--pset", &format!("null={NULL}")])
            .arg("--field-separator")
            .arg(FIELD)
            // **The record separator, so a value with a newline in it is still one row.**
            // `--no-align` otherwise ends a record with a newline, and a `text` column
            // holding an address would be read back as three rows of the wrong width.
            .arg("--record-separator")
            .arg(RECORD)
            .arg("--variable")
            .arg("ON_ERROR_STOP=1")
            // **Three `--command`s in one session, and the fence is the first.** psql runs
            // them in order on one connection, so the transaction the first one opens is
            // still open for the second — which is what makes the server, rather than
            // anything here, the thing that refuses a write. Only the middle one produces
            // rows, so only its output has to be parsed.
            .arg("--command")
            .arg(format!(
                "BEGIN; SET TRANSACTION READ ONLY; SET LOCAL statement_timeout = {};",
                asked.timeout.as_millis()
            ))
            .arg("--command")
            .arg(sql)
            .arg("--command")
            .arg("ROLLBACK;");

        let output = command
            .output()
            .map_err(|error| missing_tool(&self.tools.query, &error))?;

        if !output.status.success() {
            return Err(from_tool(&self.tools.query, &output, Exit::Usage, target));
        }

        Ok(rows_from(&String::from_utf8_lossy(&output.stdout)))
    }

    fn foreign_keys(&self, target: &Target<'_>) -> Outcome<Vec<ForeignKey>> {
        let rows = self.query(target, FOREIGN_KEYS_SQL)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let listed = |at: usize| -> Vec<String> {
                    row.get(at)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.split(INNER).map(str::to_owned).collect())
                        .unwrap_or_default()
                };
                Some(ForeignKey {
                    from: Table {
                        schema: row.first()?.clone(),
                        name: row.get(1)?.clone(),
                    },
                    columns: listed(2),
                    to: Table {
                        schema: row.get(3)?.clone(),
                        name: row.get(4)?.clone(),
                    },
                    to_columns: listed(5),
                })
            })
            .filter(|key| !key.columns.is_empty() && key.columns.len() == key.to_columns.len())
            .collect())
    }

    fn quoted_name(&self, name: &str) -> String {
        quote_identifier(name)
    }

    fn quoted_value(&self, value: &str) -> String {
        sql_literal(value)
    }
}

/// What psql is told to print where the server said `NULL`.
///
/// **A character no value can contain**, which is the whole point: without it a `NULL` and
/// an empty string arrive as the same empty field, and a grid cannot tell the reader which
/// one is really there.
const NULL: &str = "\u{1c}";

/// What separates one row from the next.
///
/// Not a newline, because a value is allowed to contain one — see [`Adapter::read`].
const RECORD: &str = "\u{1e}";

/// Take psql's delimited output apart into a header and rows.
///
/// **Windows puts a `\r` in front of every `\n` psql writes**, including the ones inside a
/// value, because the client's standard output is a text-mode handle and there is no flag
/// that turns that off. So a `\r\n` inside a cell is put back to the `\n` it started as. The
/// one thing this cannot tell apart is a value that really did contain `\r\n` — on this
/// platform, through this client, the two are the same bytes.
fn rows_from(said: &str) -> Rows {
    let body = said.trim_end_matches(['\n', '\r']);
    if body.is_empty() {
        return Rows::default();
    }

    let mut records = body.split(RECORD).map(|record| {
        record
            .split(FIELD)
            .map(|field| {
                if field == NULL {
                    Cell::Null
                } else if cfg!(windows) {
                    Cell::Text(field.replace("\r\n", "\n"))
                } else {
                    Cell::Text(field.to_owned())
                }
            })
            .collect::<Vec<Cell>>()
    });

    let columns = records
        .next()
        .map(|heading| {
            heading
                .into_iter()
                .map(|cell| cell.shown().to_owned())
                .collect()
        })
        .unwrap_or_default();

    Rows {
        rows: records.collect(),
        columns,
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
/// What `pg_stat_database` and `pg_database_size` say about the database being connected to.
///
/// `current_database()` rather than a name carried in from the registry: the connection has
/// already decided which database this is, and naming it again is a second chance to ask about
/// the wrong one.
const ACTIVITY_SQL: &str = "\
SELECT coalesce(tup_inserted, 0) + coalesce(tup_updated, 0) + coalesce(tup_deleted, 0), \
coalesce(tup_returned, 0) + coalesce(tup_fetched, 0), \
pg_database_size(current_database()) \
FROM pg_stat_database WHERE datname = current_database()";

/// One field of a row as a number, or `None` for anything that is not one.
///
/// **`psql --tuples-only --no-align` prints NULL as an empty string**, so "there was no row"
/// and "the server would not say" arrive the same way and both mean `None`.
fn number(field: Option<&String>) -> Option<i64> {
    field?.trim().parse().ok()
}

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

/// The two halves of the script a merge runs on the destination, with the rows in between.
///
/// **Separate from the plumbing that runs it**, because they are two different kinds of care:
/// one is about quoting and SQL, the other about pipes and which child's complaint outranks
/// which.
fn merge_script(shape: &TableShape, table: &str, columns: &[String]) -> (String, String) {
    let listed = columns.join(", ");
    let keys: Vec<String> = shape
        .primary_key
        .iter()
        .map(|name| quote_identifier(name))
        .collect();
    let matching = keys
        .iter()
        .map(|key| format!("d.{key} = s.{key}"))
        .collect::<Vec<_>>()
        .join(" AND ");

    // **Everything but the key is replaced; a table that is nothing but its key has nothing
    // to replace.** `DO UPDATE SET` with an empty list is a syntax error, and a row whose
    // every column is part of the key is already identical when the key matches — so
    // `DO NOTHING` is not a shortcut, it is the same outcome.
    let replaced: Vec<String> = columns
        .iter()
        .filter(|column| !keys.contains(column))
        .map(|column| format!("{column} = EXCLUDED.{column}"))
        .collect();
    let resolution = if replaced.is_empty() {
        "DO NOTHING".to_owned()
    } else {
        format!("DO UPDATE SET {}", replaced.join(", "))
    };

    // `GENERATED ALWAYS AS IDENTITY` refuses an explicit value outright, and a merge whose
    // whole job is to carry the source's keys has to say so.
    let overriding = if shape.server_assigned {
        " OVERRIDING SYSTEM VALUE"
    } else {
        ""
    };

    // **One statement per number, each labelled.** Reading them back by name rather than by
    // position is what lets the MySQL family emit the same five: it cannot name a temporary
    // table twice in one query, so a single wide `SELECT` would have been PostgreSQL-shaped
    // and nothing else.
    let counted = |name: &str, what: &str| {
        format!(
            "SELECT {}, {}, ({what});\n",
            sql_literal(MARKER),
            sql_literal(name)
        )
    };

    let header = format!(
        "BEGIN;\n\
         CREATE TEMP TABLE {STAGING} (LIKE {table}) ON COMMIT DROP;\n\
         COPY {STAGING} ({listed}) FROM STDIN;\n"
    );
    let footer = format!(
        "\\.\n\
         {loaded}{before}{matched}{kept}\
         INSERT INTO {table} ({listed}){overriding} SELECT {listed} FROM {STAGING} \
         ON CONFLICT ({keyed}) {resolution};\n\
         {after}\
         COMMIT;\n",
        keyed = keys.join(", "),
        loaded = counted("loaded", &format!("SELECT count(*) FROM {STAGING}")),
        before = counted("before", &format!("SELECT count(*) FROM {table}")),
        matched = counted(
            "matched",
            &format!(
                "SELECT count(*) FROM {table} d \
                 WHERE EXISTS (SELECT 1 FROM {STAGING} s WHERE {matching})"
            )
        ),
        kept = counted(
            "kept",
            &format!(
                "SELECT count(*) FROM {table} d \
                 WHERE NOT EXISTS (SELECT 1 FROM {STAGING} s WHERE {matching})"
            )
        ),
        after = counted("after", &format!("SELECT count(*) FROM {table}")),
    );

    (header, footer)
}

/// Every table's shape, in one statement.
///
/// **One round trip, not three per table.** Columns, primary key and foreign keys are three
/// catalogue questions, and a database with three hundred tables would be nine hundred
/// connections' worth of them before a single row moved.
///
/// `attgenerated <> ''` is excluded from the column list on purpose: a stored generated
/// column is computed from the others and refuses a value outright, so carrying it would
/// turn every merge into an error. `attidentity = 'a'` is *kept* and flagged instead —
/// `GENERATED ALWAYS AS IDENTITY` is a key worth copying, and PostgreSQL will hand it over
/// to `OVERRIDING SYSTEM VALUE`.
///
/// A foreign key pointing at the table's own rows is left out of `references`: an employee
/// with a manager is not a cycle, and the rows inside one table arrive in one statement.
const SHAPES_SQL: &str = "\
WITH cols AS (\
  SELECT a.attrelid, string_agg(a.attname, E'\\x1e' ORDER BY a.attnum) AS names, \
         bool_or(a.attidentity = 'a') AS assigned \
    FROM pg_attribute a \
   WHERE a.attnum > 0 AND NOT a.attisdropped AND a.attgenerated = '' \
   GROUP BY a.attrelid\
), pk AS (\
  SELECT c.conrelid, string_agg(a.attname, E'\\x1e' ORDER BY k.ord) AS names \
    FROM pg_constraint c \
    JOIN LATERAL unnest(c.conkey) WITH ORDINALITY AS k(num, ord) ON true \
    JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.num \
   WHERE c.contype = 'p' \
   GROUP BY c.conrelid\
), fk AS (\
  SELECT c.conrelid, string_agg(DISTINCT pn.nspname || E'\\x1d' || pc.relname, E'\\x1e') AS parents \
    FROM pg_constraint c \
    JOIN pg_class pc ON pc.oid = c.confrelid \
    JOIN pg_namespace pn ON pn.oid = pc.relnamespace \
   WHERE c.contype = 'f' AND c.confrelid <> c.conrelid \
   GROUP BY c.conrelid\
) \
SELECT n.nspname, c.relname, coalesce(cols.names, ''), \
       coalesce(cols.assigned, false), coalesce(pk.names, ''), coalesce(fk.parents, '') \
  FROM pg_class c \
  JOIN pg_namespace n ON n.oid = c.relnamespace \
  LEFT JOIN cols ON cols.attrelid = c.oid \
  LEFT JOIN pk ON pk.conrelid = c.oid \
  LEFT JOIN fk ON fk.conrelid = c.oid \
 WHERE c.relkind = 'r' AND n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%' \
 ORDER BY 1, 2";

/// Every foreign key, column by column, so a join can be offered from one.
///
/// **The columns come back in key order and the two lists line up**, which is what
/// `WITH ORDINALITY` is doing: `conkey` and `confkey` are arrays whose nth entries are a
/// pair, and aggregating either without the ordinal would leave the builder joining the
/// wrong column to the wrong column on any key of more than one.
///
/// A self-reference is kept here, unlike in `SHAPES_SQL`. `sync` leaves it out because it is
/// ordering tables and a table cannot come before itself; a join of a table to itself is an
/// ordinary thing to want — every employee beside their manager — and is worth offering.
const FOREIGN_KEYS_SQL: &str = "\
SELECT n.nspname, c.relname, \
       (SELECT string_agg(a.attname, E'\\x1e' ORDER BY k.ord) \
          FROM unnest(con.conkey) WITH ORDINALITY AS k(num, ord) \
          JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.num), \
       pn.nspname, pc.relname, \
       (SELECT string_agg(a.attname, E'\\x1e' ORDER BY k.ord) \
          FROM unnest(con.confkey) WITH ORDINALITY AS k(num, ord) \
          JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.num) \
  FROM pg_constraint con \
  JOIN pg_class c ON c.oid = con.conrelid \
  JOIN pg_namespace n ON n.oid = c.relnamespace \
  JOIN pg_class pc ON pc.oid = con.confrelid \
  JOIN pg_namespace pn ON pn.oid = pc.relnamespace \
 WHERE con.contype = 'f' \
   AND n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%' \
 ORDER BY 1, 2, con.conname";

/// Put every sequence back above the rows that are now in the column it feeds.
///
/// **Identity columns are sequences too**, which is why this walks `pg_depend` rather than
/// `pg_get_serial_sequence`: `deptype = 'a'` is a `serial`'s sequence and `'i'` is an
/// identity column's, and both hand out numbers that a merge has just inserted past.
///
/// `setval(…, max + 1, false)` rather than `setval(…, max)`: the third argument is
/// `is_called`, and `false` means the next `nextval` returns exactly this number. An empty
/// table comes back to 1, which is where it started.
///
/// `query_to_xml` is how a `max()` over a table named in a row is run inside the same
/// statement — the same trick the exact row counts use, and for the same reason.
const RESET_SEQUENCES_SQL: &str = "\
SELECT x.seq, x.next, setval(x.seq::regclass, x.next, false) \
  FROM (\
    SELECT quote_ident(ns.nspname) || '.' || quote_ident(s.relname) AS seq, \
           coalesce((xpath('/row/m/text()', query_to_xml(\
             format('select max(%I) as m from %I.%I', a.attname, tn.nspname, t.relname), \
             false, true, '')))[1]::text::bigint, 0) + 1 AS next \
      FROM pg_class s \
      JOIN pg_namespace ns ON ns.oid = s.relnamespace \
      JOIN pg_depend d ON d.objid = s.oid AND d.classid = 'pg_class'::regclass \
                      AND d.deptype IN ('a', 'i') \
      JOIN pg_class t ON t.oid = d.refobjid AND t.relkind = 'r' \
      JOIN pg_namespace tn ON tn.oid = t.relnamespace \
      JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = d.refobjsubid \
     WHERE s.relkind = 'S' AND tn.nspname <> 'information_schema' \
       AND tn.nspname NOT LIKE 'pg\\_%'\
  ) x \
 ORDER BY 1";

/// A table as `pg_dump --table` wants it.
///
/// **Quoted, because the argument is a pattern.** Unquoted, `pg_dump` folds the name to lower
/// case and reads `.`, `*` and `?` as syntax — so a table called `Orders` would match nothing
/// and one called `a.b` would be read as a schema. Double quotes turn the whole thing into a
/// literal, which is what a name that has already been resolved needs to be.
fn table_pattern(table: &Table) -> String {
    format!(
        "--table={}.{}",
        quote_identifier(&table.schema),
        quote_identifier(&table.name)
    )
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
    .hint(crate::engine::advice_for(exit))
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
