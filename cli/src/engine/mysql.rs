//! MySQL and MariaDB, over `mysqldump` / `mariadb-dump` and the `mysql` / `mariadb` client.
//!
//! **Two engines, one file, and that is deliberate.** MariaDB is a separate [`Engine`], a
//! separate arm in [`super::adapter_for`] and a separate [`Adapter`] value with its own tool
//! names — but the wire protocol and the client flags are the same ones, so a second copy of
//! this code would be a second copy that drifts. What actually differs is named in
//! [`Family`] and nowhere else: the programs to run, how they print their version, and which
//! server each one is willing to talk to.
//!
//! **What this engine cannot do is said out loud rather than worked around.**
//! `mysqldump` writes a stream of SQL. There is no archive a restore can read selectively
//! and no way to restore in parallel, so [`Capabilities`] reports `false` for both and
//! nothing here pretends otherwise by, say, splitting the file up and hoping.
//!
//! **There is no client-newer-than-server rule here**, and its absence is on purpose.
//! PostgreSQL refuses outright and sloop refuses first with a better sentence; MySQL has no
//! such rule, so inventing one would block dumps that work. The two real cross-version
//! traps are handled instead, by asking the tool what flags it has — see [`Flags`].
//!
//! Three things every child process gets, exactly as in `postgres`:
//!
//! - **`--no-defaults`**, first argument, so a `~/.my.cnf` or a `MYSQL_HOST` cannot quietly
//!   send a backup to a different server than the one that was asked for.
//! - **standard input closed** — except the restore, whose input *is* the dump — so nothing
//!   can stop and wait for an answer nobody is there to give.
//! - **`MYSQL_PWD` in its own environment and nowhere wider.** An environment is readable by
//!   the user who owns the process; `argv` is readable by everyone on the machine.

use std::borrow::Cow;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use super::privileges::{self, Finding, Report, Verdict, verdict_from};
use super::{
    Adapter, Capabilities, Engine, Provisioned, Provisioning, ServerInfo, Table, TableCount,
    Target, Version,
};

/// Long enough to cross a slow link, short enough that a scheduled run does not sit on a
/// dead host until someone notices. Matches the PostgreSQL adapter.
const CONNECT_TIMEOUT_SECONDS: &str = "10";

/// One encoding from end to end.
///
/// Left to itself a MariaDB 10 client connects as `latin1` and a MySQL 8 one as `utf8mb4`,
/// which is how the same table dumps differently depending on which machine ran the backup.
/// Naming it means the dump says what it is, and the restore reads it back the same way.
const CHARSET: &str = "utf8mb4";

/// Which of the two this is.
///
/// Everything that genuinely differs between MySQL and MariaDB lives on this enum. Adding a
/// third fork of MySQL would be a variant here rather than another file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// MySQL, whose tools are `mysqldump` and `mysql`.
    Mysql,
    /// MariaDB, whose tools are `mariadb-dump` and `mariadb`.
    Mariadb,
}

impl Family {
    /// The engine this family answers to.
    #[must_use]
    pub const fn engine(self) -> Engine {
        match self {
            Self::Mysql => Engine::Mysql,
            Self::Mariadb => Engine::Mariadb,
        }
    }

    /// What the dump program is called.
    const fn dump_program(self) -> &'static str {
        match self {
            Self::Mysql => "mysqldump",
            Self::Mariadb => "mariadb-dump",
        }
    }

    /// What the client program is called.
    const fn client_program(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
        }
    }

    /// The word a server of this family puts in its own version string.
    const fn marker(self) -> &'static str {
        match self {
            Self::Mysql => "MySQL",
            Self::Mariadb => "MariaDB",
        }
    }
}

/// Where the client tools are.
///
/// Bare names by default, found on `PATH`. R6 replaces that with real discovery — and with
/// the offer to go and fetch them — but the adapter only ever needs to know where they
/// ended up, so that work can arrive without touching this file.
#[derive(Debug, Clone)]
pub struct Tools {
    /// `mysqldump`, or `mariadb-dump`.
    pub dump: PathBuf,
    /// `mysql`, or `mariadb`.
    pub client: PathBuf,
}

impl Tools {
    /// The names this family ships under, resolved on `PATH`.
    #[must_use]
    pub fn named_for(family: Family) -> Self {
        Self {
            dump: PathBuf::from(family.dump_program()),
            client: PathBuf::from(family.client_program()),
        }
    }
}

/// The MySQL adapter.
#[must_use]
pub fn mysql() -> MysqlFamily {
    MysqlFamily::new(Family::Mysql, Tools::named_for(Family::Mysql))
}

/// The MariaDB adapter. A different engine, different programs, the same protocol.
#[must_use]
pub fn mariadb() -> MysqlFamily {
    MysqlFamily::new(Family::Mariadb, Tools::named_for(Family::Mariadb))
}

/// MySQL, or MariaDB. See [`Family`] for what separates them.
pub struct MysqlFamily {
    family: Family,
    tools: Tools,
    /// Asked of the dump program once, the first time a dump needs it. See [`Flags`].
    flags: OnceLock<Flags>,
}

/// Which of the optional arguments this particular build of the tools will take.
///
/// Every one of them exists to survive a real pairing, and every one is *asked for* rather
/// than inferred from a version number — a distribution's fork of `mysqldump` answers the
/// question about itself better than a table in this file ever could.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Flags {
    /// Added to every invocation, because they are about the connection.
    connection: Vec<&'static str>,
    /// Added to the dump and nothing else.
    dump: Vec<&'static str>,
}

/// The option to look for in `--help`, and what to pass when it is there.
///
/// These two belong on every program, because both are about getting connected at all.
const CONNECTION_OPTIONS: [(&str, &str); 2] = [
    // TLS when the server offers it, plain when it does not — stated rather than left to
    // whatever this build's default happens to be.
    ("--ssl-mode", "--ssl-mode=PREFERRED"),
    // Without this, sloop cannot log in to a MySQL 8 server that has no certificate — and
    // that is most of them. See [`MysqlFamily::spawn`] for what it costs.
    ("--get-server-public-key", "--get-server-public-key"),
];

/// The same, for the dump program alone.
const DUMP_OPTIONS: [(&str, &str); 3] = [
    // Without it MySQL 8's dump wants the `PROCESS` privilege, which an ordinary backup
    // role has no business holding.
    ("--no-tablespaces", "--no-tablespaces"),
    // MySQL 8's dump reads `information_schema.column_statistics` by default; no MariaDB
    // and no MySQL before 8 has that table, and the dump dies on it.
    ("--column-statistics", "--column-statistics=0"),
    // Otherwise a dump from a replicated server carries a `GTID_PURGED` statement that
    // refuses to load into a server with a history of its own.
    ("--set-gtid-purged", "--set-gtid-purged=OFF"),
];

impl MysqlFamily {
    /// An adapter of this family, running the tools at these paths.
    #[must_use]
    pub fn new(family: Family, tools: Tools) -> Self {
        Self {
            family,
            tools,
            flags: OnceLock::new(),
        }
    }

    /// Which of the two this is.
    #[must_use]
    pub const fn family(&self) -> Family {
        self.family
    }

    /// What the dump program says when asked its version.
    ///
    /// Not [`Version::from_tool_output`], because MariaDB's tools print two versions and the
    /// first one is the wrong one — see [`version_of_a_client_tool`].
    pub fn client_version(&self) -> Outcome<Version> {
        let output = Command::new(&self.tools.dump)
            .arg("--no-defaults")
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| self.missing_tool(&self.tools.dump, &error))?;

        let said = String::from_utf8_lossy(&output.stdout);
        version_of_a_client_tool(&said).ok_or_else(|| {
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

    /// Ask the dump program which of the optional arguments it takes, once.
    ///
    /// `--help` does not connect to anything, so this costs one process start per run and
    /// answers five questions that would otherwise be guesses about a version number. The
    /// client is not asked separately: the two programs come out of the same distribution
    /// and link the same client library, so they share every connection option there is.
    fn flags(&self) -> &Flags {
        self.flags.get_or_init(|| {
            let Ok(output) = Command::new(&self.tools.dump)
                .arg("--no-defaults")
                .arg("--help")
                .stdin(Stdio::null())
                .output()
            else {
                // The tool is not there at all. Saying "it takes no optional arguments" is
                // harmless: whatever needed it is about to fail with a sentence about the
                // missing program, which is the useful message.
                return Flags::default();
            };

            let help = String::from_utf8_lossy(&output.stdout);
            let supported = |options: &[(&str, &'static str)]| {
                options
                    .iter()
                    .filter(|(look_for, _)| help.contains(look_for))
                    .map(|&(_, pass)| pass)
                    .collect()
            };

            Flags {
                connection: supported(&CONNECTION_OPTIONS),
                dump: supported(&DUMP_OPTIONS),
            }
        })
    }

    /// The flags that say which server, in every invocation.
    ///
    /// `--connect-timeout` is **not** among them, although it belongs to exactly this list:
    /// the dump program does not have the option and refuses to start when given it. It is
    /// added by the two places that run the client instead — which are also the places a
    /// dead host is met first, since every command probes before it does anything else.
    fn connection_args(target: &Target<'_>) -> Vec<String> {
        vec![
            // TCP even when the host is `localhost`, which both clients would otherwise
            // read as "use the socket" — and then ignore the port entirely.
            "--protocol=TCP".to_owned(),
            format!("--host={}", target.host),
            format!("--port={}", target.port),
            format!("--user={}", target.user),
            format!("--default-character-set={CHARSET}"),
        ]
    }

    /// A child process with the password in its environment and nowhere else.
    fn spawn(&self, program: &Path, target: &Target<'_>) -> Command {
        let mut command = Command::new(program);

        command
            .stdin(Stdio::null())
            // First argument, and it has to be: both tools reject `--no-defaults` anywhere
            // else. Everything after it is ours, and nothing on this machine can add to it.
            .arg("--no-defaults")
            .env("MYSQL_PWD", target.password.expose());

        // Anything that could quietly redirect the connection somewhere else. The flags
        // above already win over most of these, and `--no-defaults` covers the config
        // files, but an environment variable is read before either.
        for redirect in [
            "MYSQL_HOST",
            "MYSQL_TCP_PORT",
            "MYSQL_UNIX_PORT",
            "MYSQL_GROUP_SUFFIX",
            "MYSQL_HOME",
            "MYSQL_DEBUG",
            "MYSQL_HISTFILE",
            "LIBMYSQL_PLUGIN_DIR",
            "LIBMYSQL_PLUGINS",
        ] {
            command.env_remove(redirect);
        }

        // `--ssl-mode=PREFERRED`, and `--get-server-public-key` where the build has it.
        //
        // The second is the one worth explaining, because it is a trade and not a freebie.
        // `caching_sha2_password` has been MySQL's default since 8.0, and it will not send a
        // password over an unencrypted connection unless the client holds the server's RSA
        // public key to seal it with. Without this argument the answer on a server with no
        // certificate is `Authentication requires secure connection` — on every
        // private-network database this tool exists to back up.
        //
        // The cost: a client that takes the key from whoever answered can be handed one by
        // somebody in the middle, who then learns the password. That can only happen on a
        // connection which is already unencrypted, where the same attacker is already reading
        // every row and every statement — and never on one where TLS came up, because then
        // the exchange happens inside it. A managed endpoint offers TLS and never reaches
        // this line.
        command.args(&self.flags().connection);

        command
    }

    /// Run a read-only statement and hand back its rows.
    ///
    /// `--batch` is what makes the output parseable: one row per line, columns separated by
    /// a tab, and any tab, newline or backslash *inside* a value escaped on the way out. So
    /// splitting on a raw tab cannot be fooled by a value that contains one.
    fn query(&self, target: &Target<'_>, sql: &str) -> Outcome<Vec<Vec<String>>> {
        let mut command = self.spawn(&self.tools.client, target);
        command
            .args(Self::connection_args(target))
            .arg(format!("--connect-timeout={CONNECT_TIMEOUT_SECONDS}"))
            .arg(format!("--database={}", target.database))
            .arg("--batch")
            .arg("--skip-column-names")
            .arg("--execute")
            .arg(sql);

        let output = command
            .output()
            .map_err(|error| self.missing_tool(&self.tools.client, &error))?;

        if !output.status.success() {
            return Err(from_tool(
                &self.tools.client,
                &output,
                Exit::Connect,
                target,
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(|line| line.split('\t').map(str::to_owned).collect())
            .collect())
    }

    /// Refuse a server of the other family.
    ///
    /// Not pedantry. `mysqldump` from MySQL 8 against a MariaDB server fails outright on
    /// `information_schema.column_statistics`, and the pairings that do not fail produce a
    /// dump shaped by assumptions that the other server does not share. Two engines exist
    /// precisely so this can be caught here, in a sentence, before a backup that nobody
    /// will read until they need it.
    fn refuse_the_other_family(&self, said: &str, target: &Target<'_>) -> Outcome<()> {
        let is_mariadb = said.to_ascii_lowercase().contains("mariadb");
        let expected = matches!(self.family, Family::Mariadb);
        if is_mariadb == expected {
            return Ok(());
        }

        let (found, wanted) = if is_mariadb {
            (Family::Mariadb, Family::Mysql)
        } else {
            (Family::Mysql, Family::Mariadb)
        };

        Err(Failure::new(
            Exit::Usage,
            format!(
                "{} is {} {said}, and this database is registered as {}",
                target.describe(),
                found.marker(),
                wanted.engine()
            ),
        )
        .hint(format!(
            "register it as `{}` instead — {} ships {} and dumps its own catalogue",
            found.engine(),
            found.marker(),
            found.dump_program()
        )))
    }

    /// The tool is not on this machine, or not where we were told it was.
    fn missing_tool(&self, tool: &Path, error: &std::io::Error) -> Failure {
        let failure = Failure::new(
            Exit::Usage,
            format!("could not run {}: {error}", tool.display()),
        );

        match self.family {
            Family::Mysql => failure.hint(
                "MySQL's client tools are not bundled with sloop. `sloop doctor` reports what \
                 this machine has",
            ),
            // Worth naming: MariaDB only renamed its tools at 10.5, so a machine with an
            // older MariaDB has `mysqldump` and no `mariadb-dump`, and the error otherwise
            // reads as "MariaDB is not installed" on a machine where it is.
            Family::Mariadb => failure.hint(
                "MariaDB's client tools are not bundled with sloop, and before MariaDB 10.5 \
                 they were called `mysqldump` and `mysql`. `sloop doctor` reports what this \
                 machine has",
            ),
        }
    }
}

/// Turn a tool's own complaint into a failure with the right code on it.
///
/// A connection problem is `3` whatever was being attempted when it happened, because
/// that is the distinction a scheduler acts on: a server that is down will be up later,
/// and a dump that failed on its own terms will not fix itself.
fn from_tool(tool: &Path, output: &Output, otherwise: Exit, target: &Target<'_>) -> Failure {
    let said = String::from_utf8_lossy(&output.stderr);
    from_stderr(tool, &said, otherwise, target)
}

fn from_stderr(tool: &Path, said: &str, otherwise: Exit, target: &Target<'_>) -> Failure {
    let first = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !is_a_password_warning(line))
        .unwrap_or("it said nothing");

    let exit = if looks_like_a_connection_problem(said) {
        Exit::Connect
    } else {
        otherwise
    };

    let failure = Failure::new(
        exit,
        format!(
            "{} failed against {}: {first}",
            tool.file_name()
                .unwrap_or(tool.as_os_str())
                .to_string_lossy(),
            target.describe()
        ),
    );

    if mentions_a_definer(said) {
        failure.hint(
            "the dump names an account that this one may not impersonate. sloop strips \
             the DEFINER from views, triggers, routines and events it dumps, so this is a \
             dump that something else wrote",
        )
    } else {
        failure
    }
}

impl Adapter for MysqlFamily {
    fn engine(&self) -> Engine {
        self.family.engine()
    }

    fn capabilities(&self) -> Capabilities {
        // Said out loud rather than worked around. `mysqldump` writes SQL text: there is no
        // archive to read selectively and no second connection to restore it with.
        Capabilities {
            custom_format: false,
            parallel_restore: false,
        }
    }

    fn probe(&self, target: &Target<'_>) -> Outcome<ServerInfo> {
        let rows = self.query(target, "SELECT VERSION()")?;
        let said = rows
            .first()
            .and_then(|row| row.first())
            .map(String::as_str)
            .filter(|said| !said.is_empty())
            .ok_or_else(|| {
                Failure::new(
                    Exit::Connect,
                    format!(
                        "{} answered nothing when asked its version",
                        target.describe()
                    ),
                )
            })?;

        self.refuse_the_other_family(said, target)?;

        let version = Version::from_tool_output(said).ok_or_else(|| {
            Failure::new(
                Exit::Connect,
                format!(
                    "{} gave a version this sloop cannot read: {said}",
                    target.describe()
                ),
            )
        })?;

        Ok(ServerInfo {
            engine: self.engine(),
            version,
            tls: self.connection_is_encrypted(target)?,
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
        let tables = self.tables(target)?;
        if tables.is_empty() {
            return Ok(Vec::new());
        }

        // One statement, so every count is read from one snapshot. InnoDB gives a single
        // statement a consistent read view of its own, which is what makes a hundred
        // `count(*)`s in one query comparable with each other.
        //
        // MySQL has no `query_to_xml`, so the statement is built from the table list rather
        // than generated by the server. Both halves of every name are quoted twice over:
        // as a string, to be printed back out, and as an identifier, to be selected from.
        let statement = tables
            .iter()
            .map(|table| {
                format!(
                    "SELECT {} AS s, {} AS t, COUNT(*) AS c FROM {}.{}",
                    sql_literal(&table.schema),
                    sql_literal(&table.name),
                    quote_identifier(&table.schema),
                    quote_identifier(&table.name),
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");

        let rows = self.query(target, &statement)?;
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
                    // `TABLE_ROWS` is `NULL` for a table InnoDB has no statistics for, and
                    // the client prints that as `NULL`. Zero is the honest reading of it —
                    // and an estimate either way, which `verify` never fails a run on.
                    rows: row.get(2).and_then(|value| value.parse().ok()).unwrap_or(0),
                })
            })
            .collect())
    }

    fn dump_into(&self, target: &Target<'_>, sink: &mut dyn Write) -> Outcome<()> {
        // Before anything is written: is this the engine it was registered as?
        self.probe(target)?;

        let mut command = self.spawn(&self.tools.dump, target);
        command
            .args(Self::connection_args(target))
            // The four the product asks for. `--single-transaction` is also what keeps this
            // a read: the alternative, `--lock-tables`, takes locks on the source.
            .arg("--single-transaction")
            .arg("--routines")
            .arg("--triggers")
            .arg("--events")
            // A BLOB written as hex cannot be mangled by whatever charset the text around
            // it is in.
            .arg("--hex-blob");

        // Whichever of `DUMP_OPTIONS` this build takes.
        command.args(&self.flags().dump);

        // One database, named positionally and without `--databases`, so the dump carries
        // no `CREATE DATABASE` and no `USE`. What it restores into is then the restore's
        // decision — which is what lets a backup of `app` land in `app_staging`.
        command.arg(target.database);

        self.run_the_dump(command, target, sink)
    }

    fn restore(&self, target: &Target<'_>, from: &Path) -> Outcome<()> {
        if !from.is_file() {
            return Err(Failure::new(
                Exit::Restore,
                format!("there is no dump at {}", from.display()),
            ));
        }

        let dump = std::fs::File::open(from).map_err(|error| {
            Failure::new(
                Exit::Restore,
                format!("could not read {}: {error}", from.display()),
            )
        })?;

        let output = self
            .spawn(&self.tools.client, target)
            .args(Self::connection_args(target))
            .arg(format!("--connect-timeout={CONNECT_TIMEOUT_SECONDS}"))
            .arg(format!("--database={}", target.database))
            // Batch mode, which is also what makes an error fatal: reading statements from
            // anything but a terminal, the client stops at the first one that fails and
            // exits non-zero unless `--force` says otherwise. A restore that partly worked
            // is a failure, so `--force` is exactly what this must never pass.
            .arg("--batch")
            .stdin(Stdio::from(dump))
            .output()
            .map_err(|error| self.missing_tool(&self.tools.client, &error))?;

        if !output.status.success() {
            return Err(from_tool(
                &self.tools.client,
                &output,
                Exit::Restore,
                target,
            ));
        }

        Ok(())
    }

    fn restore_into(&self, target: &Target<'_>, source: &mut dyn std::io::Read) -> Outcome<()> {
        let mut child = self
            .spawn(&self.tools.client, target)
            .args(Self::connection_args(target))
            .arg(format!("--connect-timeout={CONNECT_TIMEOUT_SECONDS}"))
            .arg(format!("--database={}", target.database))
            // Batch mode, which is also what makes an error fatal: reading statements from
            // anything but a terminal, the client stops at the first one that fails and
            // exits non-zero unless `--force` says otherwise. A restore that partly worked
            // is a failure, so `--force` is exactly what this must never pass.
            .arg("--batch")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| self.missing_tool(&self.tools.client, &error))?;

        // Both output pipes drained on threads of their own: a full pipe stops the client
        // reading the input this call is still writing, which would be a restore that hangs
        // rather than one that fails.
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
            // Dropped before the wait, so the client sees the end of the script.
            drop(sink);
            copied
        };

        let status = child.wait().map_err(|error| {
            Failure::new(
                Exit::Restore,
                format!("{} would not finish: {error}", self.tools.client.display()),
            )
        })?;
        let said = String::from_utf8_lossy(&draining.join().unwrap_or_default()).into_owned();
        let _ = listening.join();

        if !status.success() {
            return Err(from_stderr(
                &self.tools.client,
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

    fn copy_into(&self, source: &Target<'_>, destination: &Target<'_>) -> Outcome<()> {
        // Before anything is spawned: is this the engine it was registered as?
        self.probe(source)?;

        let mut command = self.spawn(&self.tools.dump, source);
        command
            .args(Self::connection_args(source))
            .arg("--single-transaction")
            .arg("--routines")
            .arg("--triggers")
            .arg("--events")
            .arg("--hex-blob");
        command.args(&self.flags().dump);
        command.arg(source.database);

        let mut dumping = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| self.missing_tool(&self.tools.dump, &error))?;

        // **The script goes down an OS pipe, not through this process** — the same thing
        // `mysqldump | mysql` is at a shell prompt.
        let script = dumping.stdout.take().expect("stdout was piped");
        let restoring = self
            .spawn(&self.tools.client, destination)
            .args(Self::connection_args(destination))
            .arg(format!("--connect-timeout={CONNECT_TIMEOUT_SECONDS}"))
            .arg(format!("--database={}", destination.database))
            // Batch mode is what makes an error fatal. `--force` is exactly what this must
            // never pass: a copy that partly worked is a failure.
            .arg("--batch")
            .stdin(Stdio::from(script))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let restoring = match restoring {
            Ok(child) => child,
            Err(error) => {
                let _ = dumping.kill();
                let _ = dumping.wait();
                return Err(self.missing_tool(&self.tools.client, &error));
            }
        };

        // Drained while both run: a full stderr pipe stops the process that owns it, and
        // either one stopping deadlocks the other end of the script.
        let mut complaining = dumping.stderr.take().expect("stderr was piped");
        let dump_said = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = Read::read_to_end(&mut complaining, &mut said);
            said
        });

        let restored = restoring.wait_with_output().map_err(|error| {
            Failure::new(
                Exit::Restore,
                format!("{} would not finish: {error}", self.tools.client.display()),
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

        // The dump's complaint outranks the restore's: a client fed half a script fails at
        // the script, which is the symptom rather than the cause.
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
                &self.tools.client,
                &restored,
                Exit::Restore,
                destination,
            ));
        }

        Ok(())
    }

    fn provision(&self, target: &Target<'_>, asked: &Provisioning<'_>) -> Outcome<Provisioned> {
        // The script is built in a `String`, which takes `fmt::Write` rather than the
        // `io::Write` this module imports for the pipes.
        use std::fmt::Write as _;

        let already = self.query(
            target,
            &format!(
                "SELECT schema_name FROM information_schema.schemata WHERE schema_name = {}",
                sql_literal(asked.database)
            ),
        )?;
        if !already.is_empty() {
            return Err(Failure::usage(format!(
                "{} already exists on {}",
                asked.database,
                target.describe()
            ))
            .hint("`sloop db add` registers a database that is already there"));
        }

        // `mysql.user` needs a privileged connection, which this one is — but a managed
        // server can withhold it, and not knowing whether the account existed is not a
        // reason to refuse to create a database. `CREATE USER IF NOT EXISTS` below covers
        // both answers; this one only decides what gets printed afterwards.
        let role_existed = self
            .query(
                target,
                &format!(
                    "SELECT user FROM mysql.user WHERE user = {}",
                    sql_literal(asked.role)
                ),
            )
            .is_ok_and(|rows| !rows.is_empty());

        // **`@'%'`, deliberately.** The application that will use this database may be on
        // another host, and sloop has no way to know which; an account tied to `localhost`
        // would be a database nothing outside the machine can reach, which is the opposite
        // of the feature. Who may reach the port is the operator's to decide.
        let account = quote_account(&format!("{}@%", asked.role));
        let database = quote_identifier(asked.database);

        let mut script = String::new();
        let _ = writeln!(
            script,
            "CREATE USER IF NOT EXISTS {account} IDENTIFIED BY {};",
            sql_literal(asked.password.expose())
        );
        // utf8mb4 and the server's own collation for it: naming a collation would pin this
        // to one server's idea of the default and break on the other family.
        let _ = writeln!(script, "CREATE DATABASE {database} CHARACTER SET utf8mb4;");
        let _ = writeln!(script, "GRANT ALL PRIVILEGES ON {database}.* TO {account};");
        let _ = writeln!(script, "FLUSH PRIVILEGES;");

        self.restore_into(target, &mut script.as_bytes())?;

        Ok(Provisioned {
            role_existed,
            grants: vec![format!(
                "ALL PRIVILEGES ON {}.* TO {}@%",
                asked.database, asked.role
            )],
        })
    }

    fn clear_contents(&self, target: &Target<'_>) -> Outcome<u64> {
        // The script is built in a `String`, which takes `fmt::Write` rather than the
        // `io::Write` this module imports for the pipes.
        use std::fmt::Write as _;

        // A MySQL database *is* its schema, so there is nothing below it to drop and start
        // again with: what goes is every object in it that a dump of it would have carried.
        // `--routines --triggers --events` are all on the dump, so all of them are cleared;
        // triggers go with the tables they are attached to.
        let objects = self
            .query(
                target,
                "SELECT table_name, table_type FROM information_schema.tables \
                 WHERE table_schema = DATABASE() ORDER BY table_type DESC, table_name",
            )
            .map_err(|failure| failure.at(Exit::Restore))?;

        let mut tables = 0_u64;
        // **Foreign keys off for the duration, and only for this session.** Dropping tables
        // in an order that satisfies every constraint means sorting a graph that may have a
        // cycle in it; switching the checks off for one session is what the engine's own
        // tooling does and is undone the moment the connection closes.
        let mut script = String::from("SET FOREIGN_KEY_CHECKS = 0;\n");
        for row in &objects {
            let Some(name) = row.first() else { continue };
            let quoted = quote_identifier(name);
            if row.get(1).is_some_and(|kind| kind == "VIEW") {
                let _ = writeln!(script, "DROP VIEW IF EXISTS {quoted};");
            } else {
                let _ = writeln!(script, "DROP TABLE IF EXISTS {quoted};");
                tables += 1;
            }
        }

        let routines = self
            .query(
                target,
                "SELECT routine_name, routine_type FROM information_schema.routines \
                 WHERE routine_schema = DATABASE() ORDER BY routine_name",
            )
            .map_err(|failure| failure.at(Exit::Restore))?;
        for row in &routines {
            let Some(name) = row.first() else { continue };
            let quoted = quote_identifier(name);
            if row.get(1).is_some_and(|kind| kind == "FUNCTION") {
                let _ = writeln!(script, "DROP FUNCTION IF EXISTS {quoted};");
            } else {
                let _ = writeln!(script, "DROP PROCEDURE IF EXISTS {quoted};");
            }
        }

        let events = self
            .query(
                target,
                "SELECT event_name FROM information_schema.events \
                 WHERE event_schema = DATABASE() ORDER BY event_name",
            )
            .map_err(|failure| failure.at(Exit::Restore))?;
        for row in &events {
            let Some(name) = row.first() else { continue };
            let _ = writeln!(script, "DROP EVENT IF EXISTS {};", quote_identifier(name));
        }

        // One connection for the whole script, because `SET FOREIGN_KEY_CHECKS` only holds
        // for the session that set it.
        self.restore_into(target, &mut script.as_bytes())?;

        Ok(tables)
    }

    fn terminate_connections(&self, target: &Target<'_>) -> Outcome<u64> {
        // **`information_schema.processlist` rather than `SHOW PROCESSLIST`**, because this
        // needs a filter and a machine-readable answer, and both engines still have the
        // table. Without the `PROCESS` privilege a role sees only its own threads — so the
        // honest answer there is "none to end", and the drop then succeeds or fails on its
        // own terms rather than on a guess made here.
        let ids = self.query(
            target,
            &format!(
                "SELECT id FROM information_schema.processlist \
                  WHERE db = {} AND id <> CONNECTION_ID()",
                sql_literal(target.database)
            ),
        )?;

        let ids: Vec<&str> = ids
            .iter()
            .filter_map(|row| row.first())
            .map(String::as_str)
            .filter(|id| !id.is_empty())
            .collect();

        if ids.is_empty() {
            return Ok(0);
        }

        // One statement per thread, in one round trip. A thread that ended by itself
        // between the two queries makes `KILL` fail, and that is not a reason to stop:
        // the point was for it to be gone.
        let mut ended = 0;
        for id in ids {
            if self.query(target, &format!("KILL {id}")).is_ok() {
                ended += 1;
            }
        }

        Ok(ended)
    }

    fn drop_database(&self, target: &Target<'_>) -> Outcome<()> {
        // No maintenance database, and none is needed: MySQL and MariaDB both allow a
        // session to drop the database it is connected to, leaving it with no default
        // one. PostgreSQL is the engine that refuses, which is why the trait leaves the
        // question to the adapter instead of settling it in the command.
        //
        // `IF EXISTS` is deliberately absent — see the PostgreSQL adapter for why.
        self.query(
            target,
            &format!("DROP DATABASE {}", quote_identifier(target.database)),
        )
        .map(|_| ())
    }

    fn check_privileges(&self, target: &Target<'_>) -> Outcome<Report> {
        let server = self.probe(target)?;
        let held = self.grants(target)?;
        let contents = self.contents(target)?;
        // For the table-by-table case below. A dump reads every base table there is, so
        // "granted on each of these" and "granted on the database" come to the same thing.
        let tables: Vec<String> = self
            .tables(target)?
            .into_iter()
            .map(|table| table.name)
            .collect();

        let findings = privileges::for_engine(self.engine())
            .iter()
            .map(|requirement| {
                let satisfied = needs_of(self.family, requirement.id).iter().any(|all| {
                    all.iter()
                        .all(|need| held.has(need, target.database, &tables))
                });

                let verdict = if satisfied {
                    Verdict::Held
                } else {
                    let (applicable, detail) = contents.applicability(requirement);
                    verdict_from(requirement.applies, applicable, false, detail)
                };

                Finding {
                    requirement,
                    verdict,
                }
            })
            .collect();

        Ok(Report {
            role: held.role,
            server,
            findings,
        })
    }
}

// ---------------------------------------------------------------------------------------
// Privileges
// ---------------------------------------------------------------------------------------

/// A privilege, and where it has to be held for a requirement to count as met.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    /// Granted globally, or on the database being checked. The ordinary case.
    Here(&'static str),
    /// Granted globally and nowhere narrower. MySQL's dynamic privileges are global-only:
    /// `GRANT SHOW_ROUTINE ON db.*` is refused with `Illegal privilege level specified`.
    Globally(&'static str),
    /// Granted on one named table, wherever that table lives. MariaDB's older route to a
    /// routine definition is read access to `mysql.proc`.
    OnTable(&'static str, &'static str, &'static str),
}

/// Which grants satisfy each requirement.
///
/// The outer slice is alternatives — any one of them is enough — and the inner slice is a
/// set that has to be held in full. Separate from [`privileges::Requirement`] on purpose:
/// that table is prose for a human and for the site, this one is what the grant table is
/// searched for, and conflating them would make one of the two worse.
type Needs = &'static [&'static [Need]];

/// MySQL's mapping.
const MYSQL_NEEDS: &[(&str, Needs)] = &[
    ("my-select", &[&[Need::Here("SELECT")]]),
    ("my-show-view", &[&[Need::Here("SHOW VIEW")]]),
    ("my-trigger-dump", &[&[Need::Here("TRIGGER")]]),
    ("my-event-dump", &[&[Need::Here("EVENT")]]),
    (
        "my-show-routine",
        // Either the narrow privilege MySQL 8.0.20 added, or the broad one it replaced.
        &[
            &[Need::Globally("SHOW_ROUTINE")],
            &[Need::Globally("SELECT")],
        ],
    ),
    (
        "my-load",
        &[&[
            Need::Here("SELECT"),
            Need::Here("INSERT"),
            Need::Here("CREATE"),
            Need::Here("DROP"),
            Need::Here("ALTER"),
            Need::Here("REFERENCES"),
            Need::Here("LOCK TABLES"),
        ]],
    ),
    ("my-create-view", &[&[Need::Here("CREATE VIEW")]]),
    ("my-trigger-restore", &[&[Need::Here("TRIGGER")]]),
    (
        "my-routine-restore",
        &[&[Need::Here("CREATE ROUTINE"), Need::Here("ALTER ROUTINE")]],
    ),
    ("my-event-restore", &[&[Need::Here("EVENT")]]),
    ("my-function-binlog", &[&[Need::Globally("SUPER")]]),
];

/// MariaDB's, which differs only in how a routine definition is reached.
const MARIADB_ROUTINE_NEEDS: Needs = &[
    // MariaDB 11.3 and newer, and unlike MySQL's it can be granted per database.
    &[Need::Here("SHOW CREATE ROUTINE")],
    // Older servers: read access to the table routines actually live in.
    &[Need::OnTable("SELECT", "mysql", "proc")],
    &[Need::Globally("SELECT")],
];

/// What satisfies one requirement on this family.
fn needs_of(family: Family, id: &str) -> Needs {
    if matches!(family, Family::Mariadb) && id == "maria-show-create-routine" {
        return MARIADB_ROUTINE_NEEDS;
    }
    MYSQL_NEEDS
        .iter()
        .find(|(known, _)| *known == id)
        .map_or(&[], |(_, needs)| *needs)
}

/// One `GRANT ... ON ... TO ...` line, understood.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Grant {
    /// Privileges held on everything.
    Global(Vec<String>),
    /// Privileges held on one database.
    Database(String, Vec<String>),
    /// Privileges held on one table.
    Table(String, String, Vec<String>),
    /// A role this account holds, to be expanded in turn.
    Role(String),
}

/// Everything an account can do, as the grant table tells it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Held {
    /// What the server calls this account — `bk@%`, not the `bk` the registry named.
    role: String,
    grants: Vec<Grant>,
}

impl Held {
    /// Does this account satisfy one need, for this database?
    fn has(&self, need: &Need, database: &str, tables: &[String]) -> bool {
        let directly = self.grants.iter().any(|grant| match (need, grant) {
            (Need::Here(want) | Need::Globally(want), Grant::Global(held)) => carries(held, want),
            (Need::Here(want), Grant::Database(on, held)) => {
                on.eq_ignore_ascii_case(database) && carries(held, want)
            }
            (Need::OnTable(want, schema, table), Grant::Table(on, named, held)) => {
                on.eq_ignore_ascii_case(schema)
                    && named.eq_ignore_ascii_case(table)
                    && carries(held, want)
            }
            _ => false,
        });

        if directly {
            return true;
        }

        // Table by table. Unusual for a backup account, but reporting `SELECT` as missing
        // from a role that plainly holds it on every table would be the health check
        // crying wolf — and a report people learn to skip is a report that saves nobody.
        // It is only ever a *widening*: a privilege that is not table-scoped, or a
        // database with no tables, falls through to the answer above.
        match need {
            Need::Here(want) if !tables.is_empty() => tables.iter().all(|table| {
                self.grants.iter().any(|grant| match grant {
                    Grant::Table(on, named, held) => {
                        on.eq_ignore_ascii_case(database)
                            && named.eq_ignore_ascii_case(table)
                            && carries(held, want)
                    }
                    _ => false,
                })
            }),
            _ => false,
        }
    }
}

/// Is this privilege in a granted set — either named, or covered by `ALL PRIVILEGES`?
fn carries(held: &[String], want: &str) -> bool {
    held.iter()
        .any(|have| have == "ALL PRIVILEGES" || have == want)
}

/// What the database holds, as far as this role is allowed to see.
///
/// **The gaps in it are the point.** A role without `TRIGGER` sees no triggers in
/// `information_schema`, one without `EVENT` sees no events, and one without
/// `SHOW_ROUTINE` sees no routines — the server reports a function it may not read as
/// not existing. So "I found none" and "I am not allowed to know" are the same answer,
/// and only a role that already holds the privilege can be told it does not need it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Contents {
    views: u64,
}

impl Contents {
    /// Does this requirement apply to this database, and what is worth saying about it?
    fn applicability(self, requirement: &privileges::Requirement) -> (bool, String) {
        match requirement.applies {
            privileges::Applies::Always => (true, String::new()),
            privileges::Applies::OnlyWith(_) => match requirement.id {
                // The one object type a role can count honestly: a view is listed in
                // `information_schema.tables` with nothing more than SELECT on the
                // database, so finding none here really does mean there are none.
                "my-show-view" | "my-create-view" => (self.views > 0, String::new()),
                _ => (true, UNKNOWABLE.to_owned()),
            },
        }
    }
}

/// Said wherever a role cannot see whether it needs a privilege.
///
/// Deliberately says only the thing that is true in both phases. What it leaves out is the
/// requirement's own `consequence`, printed directly underneath — which is a different
/// sentence for a dump than for a restore, and would be wrong here half the time.
const UNKNOWABLE: &str = "and this role cannot see whether the database has any: short of \
                          the privilege the server reports none, so sloop cannot rule it out";

impl MysqlFamily {
    /// Everything the grant table says this account can do, roles expanded.
    ///
    /// `SHOW GRANTS` lists a granted role as a grant of the role rather than of what the
    /// role carries, so a backup account set up the modern way would otherwise look as
    /// though it held nothing at all.
    fn grants(&self, target: &Target<'_>) -> Outcome<Held> {
        let role = self
            .query(target, "SELECT CURRENT_USER()")?
            .first()
            .and_then(|row| row.first())
            .map_or_else(|| quote_account(target.user), |said| quote_account(said));

        let mut held = Held {
            role,
            grants: Vec::new(),
        };

        // Breadth-first over the roles this account holds, with a visited set: MySQL lets
        // roles be granted to roles, and a cycle is a thing an administrator can create.
        let mut asked = std::collections::BTreeSet::new();
        let mut ask = vec!["CURRENT_USER()".to_owned()];

        while let Some(account) = ask.pop() {
            if !asked.insert(account.clone()) {
                continue;
            }
            // A role that has since been dropped, or one this account may not read, is not
            // a reason to fail a health check: it contributes nothing and the report is
            // still worth printing.
            let Ok(rows) = self.query(target, &format!("SHOW GRANTS FOR {account}")) else {
                continue;
            };

            for line in rows.iter().filter_map(|row| row.first()) {
                match parse_grant(line) {
                    Some(Grant::Role(name)) => ask.push(name),
                    Some(grant) => held.grants.push(grant),
                    None => {}
                }
            }
        }

        Ok(held)
    }

    /// What this role can see in the database.
    fn contents(&self, target: &Target<'_>) -> Outcome<Contents> {
        let rows = self.query(
            target,
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_type = 'VIEW'",
        )?;

        Ok(Contents {
            views: rows
                .first()
                .and_then(|row| row.first())
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        })
    }
}

/// Read one line of `SHOW GRANTS`.
///
/// The shapes, all of which both engines produce:
///
/// ```text
/// GRANT USAGE ON *.* TO `bk`@`%`
/// GRANT SELECT, SHOW VIEW ON `shop`.* TO `bk`@`%` WITH GRANT OPTION
/// GRANT SELECT ON `mysql`.`proc` TO `bk`@`%`
/// GRANT `reader`@`%` TO `bk`@`%`
/// GRANT USAGE ON *.* TO `bk`@`%` IDENTIFIED BY PASSWORD '*F801A6...'
/// ```
///
/// The last of those is why this reads a prefix rather than the whole line: MariaDB puts
/// the password hash on the end, and nothing after the scope is ever wanted.
fn parse_grant(line: &str) -> Option<Grant> {
    let rest = line.trim().strip_prefix("GRANT ")?;

    // No ` ON ` at all means a role grant: `GRANT `reader`@`%` TO `bk`@`%``.
    //
    // Kept exactly as the server wrote it, backticks and all, because the only thing done
    // with it is asking `SHOW GRANTS FOR` it — and an account taken apart here would have
    // to be put back together identically to be asked about. Unquoting `` `reader`@`%` ``
    // yields `` reader`@`% ``, which is a syntax error on the way back in.
    let Some((privileges, after)) = rest.split_once(" ON ") else {
        let (role, _) = rest.split_once(" TO ")?;
        return Some(Grant::Role(role.trim().to_owned()));
    };

    let scope = after.split_once(" TO ").map_or(after, |(scope, _)| scope);
    let names = split_privileges(privileges);

    let (schema, object) = scope.trim().rsplit_once('.')?;
    let schema = unquote(schema);

    Some(match (schema, unquote(object)) {
        ("*", _) => Grant::Global(names),
        (schema, "*") => Grant::Database(schema.to_owned(), names),
        (schema, table) => Grant::Table(schema.to_owned(), table.to_owned(), names),
    })
}

/// Split a granted privilege list, keeping a column list out of it.
///
/// `GRANT SELECT (id, name), UPDATE ON ...` is legal, and splitting on every comma would
/// turn one privilege into three. A column-scoped grant is narrower than the table-level
/// one a dump needs, so the columns are dropped rather than recorded.
fn split_privileges(list: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0_usize;
    let mut current = String::new();

    for character in list.chars() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => names.push(std::mem::take(&mut current)),
            _ if depth == 0 => current.push(character),
            _ => {}
        }
    }
    names.push(current);

    names
        .into_iter()
        .map(|name| name.trim().to_ascii_uppercase())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Write an account the way a `GRANT` statement has to have it.
///
/// `CURRENT_USER()` answers `bk@%`, and `GRANT SELECT ON shop.* TO bk@%` is a syntax error
/// — `%` is not an identifier. Every statement this report prints is meant to be pasted,
/// so it comes out as ``  `bk`@`%`  ``, which is also how `SHOW GRANTS` writes it back.
///
/// Split on the **last** `@`: a host name cannot contain one and a user name can.
fn quote_account(said: &str) -> String {
    let quote = |part: &str| format!("`{}`", part.replace('`', "``"));

    said.rsplit_once('@').map_or_else(
        || quote(said),
        |(user, host)| format!("{}@{}", quote(user), quote(host)),
    )
}

/// Take the backticks off an identifier.
///
/// A backtick *inside* a name is doubled, and is left as it is: names are only ever
/// compared with `eq_ignore_ascii_case` against a name that came out of the same server,
/// so both sides carry the doubling or neither does.
fn unquote(value: &str) -> &str {
    value
        .trim()
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .unwrap_or(value.trim())
}

impl MysqlFamily {
    /// Whether the connection ended up encrypted.
    ///
    /// `Ssl_cipher` is a session status variable on both engines and in every version that
    /// matters, which the tables that hold it are not: MySQL 8 dropped
    /// `information_schema.session_status`, MariaDB never had
    /// `performance_schema.session_status`, and a server can be built without
    /// `performance_schema` at all. `SHOW SESSION STATUS` survives all three.
    fn connection_is_encrypted(&self, target: &Target<'_>) -> Outcome<bool> {
        let rows = self.query(target, "SHOW SESSION STATUS LIKE 'Ssl_cipher'")?;
        Ok(rows
            .first()
            .and_then(|row| row.get(1))
            .is_some_and(|cipher| !cipher.is_empty()))
    }

    /// Run the dump program and write what it prints, a line at a time.
    ///
    /// The dump goes through this process rather than through `--result-file` for two
    /// reasons, and both of them are corrections the file needs:
    ///
    /// **The `DEFINER` comes off.** `mysqldump` stamps every view, trigger, routine and
    /// event with the account that created it, and restoring one of those as anybody else
    /// needs `SUPER` — so a backup taken by `app_rw` can only be restored by an
    /// administrator, on a server that happens to have an `app_rw`. This is the same
    /// decision `--no-owner --no-privileges` makes for PostgreSQL: the destination has its
    /// own accounts, and carrying the source's across is how a restore fails on a machine
    /// that never had them. Removing the clause leaves the object defined by whoever
    /// restores it, which is the one account known to exist.
    ///
    /// **The line endings come out as `\n`.** On Windows the dump program's own standard
    /// output goes through a C runtime in text mode, so every `\n` it writes becomes
    /// `\r\n` and the same database dumps to two different files on two platforms.
    ///
    /// Both edits are safe because they are made *per line*, and because the lines a
    /// `DEFINER` is taken off are only ever the two shapes in [`without_a_definer`] —
    /// neither of which a row of data can have. `mysqldump` escapes a newline inside a
    /// value as `\n`, so one statement is always exactly one line.
    fn run_the_dump(
        &self,
        mut command: Command,
        target: &Target<'_>,
        sink: &mut dyn Write,
    ) -> Outcome<()> {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| self.missing_tool(&self.tools.dump, &error))?;

        // Drained on a thread of its own. A dump of a thousand tables can print a thousand
        // warnings, and a full stderr pipe stops the child writing to stdout — which would
        // be a backup that hangs rather than one that fails.
        let mut stderr = child.stderr.take().expect("stderr was piped");
        let draining = std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = stderr.read_to_end(&mut said);
            said
        });

        let copied = Self::copy_the_dump(&mut child, sink);

        let status = child.wait().map_err(|error| {
            Failure::new(
                Exit::Dump,
                format!("{} would not finish: {error}", self.family.dump_program()),
            )
        })?;
        let said = String::from_utf8_lossy(&draining.join().unwrap_or_default()).into_owned();

        if !status.success() {
            // The tool's own complaint outranks whatever went wrong writing the bytes: if
            // the dump failed, a short stream is the symptom and not the cause.
            return Err(from_stderr(&self.tools.dump, &said, Exit::Dump, target));
        }

        copied
    }

    /// Copy the dump program's output into `out`, correcting it on the way. See
    /// [`MysqlFamily::run_the_dump`].
    fn copy_the_dump(child: &mut Child, out: &mut dyn Write) -> Outcome<()> {
        let failed = |what: &str, error: &std::io::Error| {
            Failure::new(Exit::Dump, format!("{what} the dump: {error}"))
        };

        let mut reading =
            BufReader::with_capacity(256 * 1024, child.stdout.take().expect("stdout was piped"));

        // Bytes, never a `String`. A dump carries whatever the columns carry, and a `latin1`
        // table is not valid UTF-8 — reading it as text would replace the bytes it could not
        // decode and quietly corrupt the backup.
        let mut line = Vec::with_capacity(8 * 1024);

        loop {
            line.clear();
            let read = reading
                .read_until(b'\n', &mut line)
                .map_err(|error| failed("could not read", &error))?;
            if read == 0 {
                break;
            }

            let had_newline = line.last() == Some(&b'\n');
            if had_newline {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }

            let corrected = without_a_definer(&line);
            out.write_all(&corrected)
                .map_err(|error| failed("could not write", &error))?;

            if had_newline {
                out.write_all(b"\n")
                    .map_err(|error| failed("could not write", &error))?;
            }
        }

        out.flush()
            .map_err(|error| failed("could not finish writing", &error))
    }
}

/// Base tables in this database, views and system catalogues left out.
///
/// MySQL has no schemas: a database is the schema, which is why `TABLE_SCHEMA` lands in
/// [`Table::schema`] and the two engines share one type.
const TABLES_SQL: &str = "\
SELECT TABLE_SCHEMA, TABLE_NAME FROM information_schema.TABLES \
WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' \
ORDER BY 1, 2";

/// What `InnoDB` thinks is in every table. Never a count.
///
/// `TABLE_ROWS` is sampled from a handful of index pages and extrapolated; MySQL's own
/// manual puts it at 40 to 50 percent of the truth on an `InnoDB` table, which is the whole
/// reason `SLOOP_VERIFY=fast` is labelled rather than trusted. Same filters as
/// [`TABLES_SQL`], so a fast run and an exact one list the same tables.
const ESTIMATED_ROW_COUNTS_SQL: &str = "\
SELECT TABLE_SCHEMA, TABLE_NAME, coalesce(TABLE_ROWS, 0) FROM information_schema.TABLES \
WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' \
ORDER BY 1, 2";

/// The version a MySQL-family client tool is reporting about *itself*.
///
/// MySQL prints one version and MariaDB prints two, the older format putting its own tool
/// version first:
///
/// ```text
/// mysqldump  Ver 8.4.3 for Linux on x86_64 (MySQL Community Server - GPL)
/// mysqldump  Ver 10.19 Distrib 10.6.16-MariaDB, for Linux (x86_64)
/// mariadb-dump  from 11.4.4-MariaDB, client 10.19 for Linux
/// ```
///
/// Taking the first version-shaped token — which is all a general parser can do — reads the
/// middle line as 10.19. So the two words MariaDB puts in front of the number it means are
/// looked for first, and only a line with neither falls back to the general rule.
#[must_use]
pub fn version_of_a_client_tool(said: &str) -> Option<Version> {
    let mut words = said.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if matches!(word, "Distrib" | "from") {
            if let Some(version) = words.peek().and_then(|next| Version::from_number(next)) {
                return Some(version);
            }
        }
    }

    Version::from_tool_output(said)
}

/// A line of a dump with any `DEFINER=` clause taken out of it.
///
/// Only ever called on a whole line, and only two shapes of line are touched — see
/// [`MysqlFamily::run_the_dump`] for why a whole line is the safe boundary:
///
/// ```text
/// /*!50013 DEFINER=`alpha`@`%` SQL SECURITY DEFINER */
/// /*!50003 CREATE*/ /*!50017 DEFINER=`alpha`@`%`*/ /*!50003 TRIGGER `t` …
/// CREATE DEFINER=`alpha`@`%` FUNCTION `widget_count`() RETURNS int
/// ```
///
/// The third is why `/*!` alone is not enough. A view, a trigger and an event all arrive
/// wrapped in a versioned comment, but a stored routine cannot be — its body runs over
/// several lines — so `mysqldump` writes that one as a plain statement between `DELIMITER`
/// markers. Both openings are still shapes no row of data has: a data line begins `INSERT`.
///
/// What is removed is exactly `DEFINER=<account>`, so the first line above keeps its
/// `SQL SECURITY DEFINER`, which is a different clause and still means something.
///
/// Borrowed unless there is something to remove, so the millions of lines that carry data
/// cost no allocation at all.
#[must_use]
pub(super) fn without_a_definer(line: &[u8]) -> Cow<'_, [u8]> {
    if !line.starts_with(b"/*!") && !line.starts_with(b"CREATE DEFINER=") {
        return Cow::Borrowed(line);
    }

    let Some(at) = find(line, b"DEFINER=") else {
        return Cow::Borrowed(line);
    };

    let mut end = end_of_an_account(line, at + b"DEFINER=".len());
    // The clause had a space in front of it and usually one behind. Taking one of the two
    // leaves the statement spaced the way it was written rather than gapped.
    if at > 0 && line[at - 1] == b' ' && line.get(end) == Some(&b' ') {
        end += 1;
    }

    let mut edited = Vec::with_capacity(line.len());
    edited.extend_from_slice(&line[..at]);
    edited.extend_from_slice(&line[end..]);
    Cow::Owned(edited)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Where the account in a `DEFINER=` clause ends.
///
/// `` `user`@`host` ``, `user@host`, `` `user`@'host' `` — all three spellings turn up, and
/// a backtick inside a name is doubled rather than escaped.
fn end_of_an_account(line: &[u8], from: usize) -> usize {
    let mut at = skip_a_name(line, from);
    if line.get(at) == Some(&b'@') {
        at = skip_a_name(line, at + 1);
    }
    at
}

fn skip_a_name(line: &[u8], from: usize) -> usize {
    let mut at = from;

    let Some(&quote @ (b'`' | b'\'' | b'"')) = line.get(at) else {
        // Unquoted: it runs until something that cannot be part of a name.
        while at < line.len()
            && (line[at].is_ascii_alphanumeric() || matches!(line[at], b'_' | b'$' | b'.'))
        {
            at += 1;
        }
        return at;
    };

    at += 1;
    while at < line.len() {
        if line[at] == quote {
            // A doubled quote is one character of the name, not the end of it.
            if line.get(at + 1) == Some(&quote) {
                at += 2;
                continue;
            }
            return at + 1;
        }
        at += 1;
    }
    at
}

/// Quote a string for SQL.
///
/// Both characters matter. MySQL takes a backslash as an escape inside a string literal by
/// default, so a table called `` back\slash `` breaks a statement that only doubles the
/// apostrophes.
pub(super) fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

/// Quote an identifier. Backticks, doubled if the name contains one.
pub(super) fn quote_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

/// The line both tools print on every single invocation because a password came from the
/// environment. It is advice, not a failure, and it must never be the sentence a user is
/// shown when something actually goes wrong.
fn is_a_password_warning(line: &str) -> bool {
    let said = line.to_ascii_lowercase();
    said.contains("using a password on the command line interface can be insecure")
        || (said.starts_with("warning") && said.contains("password"))
}

/// Does this read like the server was unreachable rather than unhappy?
fn looks_like_a_connection_problem(stderr: &str) -> bool {
    let said = stderr.to_ascii_lowercase();
    [
        "can't connect to",
        "cannot connect to",
        "access denied for user",
        "unknown database",
        "is not allowed to connect",
        "lost connection to",
        "server has gone away",
        "connection refused",
        "unknown mysql server host",
        "ssl connection error",
    ]
    .iter()
    .any(|marker| said.contains(marker))
}

/// Did this fail because of an account named in the dump?
fn mentions_a_definer(stderr: &str) -> bool {
    let said = stderr.to_ascii_lowercase();
    said.contains("super") && said.contains("privilege") || said.contains("set_user_id")
}

#[cfg(test)]
mod privilege_tests {
    use super::{Family, Grant, Held, Need, needs_of, parse_grant, quote_account};
    use crate::engine::Engine;
    use crate::engine::privileges::for_engine;

    /// Real lines, copied out of two real servers. Both quote with backticks, MariaDB adds
    /// a password hash to the end of the account line, and both list a granted role as a
    /// grant *of the role* rather than of anything it carries.
    #[test]
    fn a_show_grants_line_is_read_the_way_both_engines_write_it() {
        let cases: [(&str, Option<Grant>); 8] = [
            (
                "GRANT USAGE ON *.* TO `bk`@`%`",
                Some(Grant::Global(vec!["USAGE".into()])),
            ),
            (
                "GRANT SHOW_ROUTINE ON *.* TO `bk`@`%`",
                Some(Grant::Global(vec!["SHOW_ROUTINE".into()])),
            ),
            (
                "GRANT SELECT, SHOW VIEW, EVENT, TRIGGER ON `shop`.* TO `bk`@`%`",
                Some(Grant::Database(
                    "shop".into(),
                    vec![
                        "SELECT".into(),
                        "SHOW VIEW".into(),
                        "EVENT".into(),
                        "TRIGGER".into(),
                    ],
                )),
            ),
            (
                "GRANT SELECT ON `mysql`.`proc` TO `bk`@`%`",
                Some(Grant::Table(
                    "mysql".into(),
                    "proc".into(),
                    vec!["SELECT".into()],
                )),
            ),
            // MariaDB's, hash and all. Nothing after the scope is ever wanted.
            (
                "GRANT SELECT ON `shop`.* TO `bk`@`%` IDENTIFIED BY PASSWORD '*F801A670'",
                Some(Grant::Database("shop".into(), vec!["SELECT".into()])),
            ),
            (
                "GRANT ALL PRIVILEGES ON `shop`.* TO `bk`@`%` WITH GRANT OPTION",
                Some(Grant::Database(
                    "shop".into(),
                    vec!["ALL PRIVILEGES".into()],
                )),
            ),
            // Kept quoted: this string goes straight back to the server in a
            // `SHOW GRANTS FOR` and has to be valid syntax when it gets there.
            (
                "GRANT `reader`@`%` TO `bk`@`%`",
                Some(Grant::Role("`reader`@`%`".into())),
            ),
            ("this is not a grant", None),
        ];

        for (line, want) in cases {
            assert_eq!(parse_grant(line), want, "{line}");
        }
    }

    /// A comma inside a column list is not a separator. Splitting on every comma would
    /// turn `SELECT (id, name)` into three privileges, one of them called `name)`.
    #[test]
    fn a_column_list_does_not_become_three_privileges() {
        let grant =
            parse_grant("GRANT SELECT (id, name), UPDATE (name) ON `shop`.`widget` TO `x`@`%`");
        assert_eq!(
            grant,
            Some(Grant::Table(
                "shop".into(),
                "widget".into(),
                vec!["SELECT".into(), "UPDATE".into()],
            ))
        );
    }

    /// A global grant satisfies a database-scoped need; a database grant does not satisfy
    /// one that has to be global, which is the rule MySQL enforces for `SHOW_ROUTINE`.
    #[test]
    fn scope_decides_what_a_grant_satisfies() {
        let global = Held {
            role: "`bk`@`%`".into(),
            grants: vec![Grant::Global(vec!["SELECT".into()])],
        };
        let on_database = Held {
            role: "`bk`@`%`".into(),
            grants: vec![Grant::Database("shop".into(), vec!["SELECT".into()])],
        };

        assert!(global.has(&Need::Here("SELECT"), "shop", &[]));
        assert!(global.has(&Need::Globally("SELECT"), "shop", &[]));
        assert!(on_database.has(&Need::Here("SELECT"), "shop", &[]));
        assert!(!on_database.has(&Need::Globally("SELECT"), "shop", &[]));
        assert!(!on_database.has(&Need::Here("SELECT"), "other", &[]));
    }

    /// `ALL PRIVILEGES` covers whatever was asked for. A report that told somebody holding
    /// it that they were missing `SHOW VIEW` would be a report nobody believed again.
    #[test]
    fn all_privileges_covers_everything_named() {
        let held = Held {
            role: "`bk`@`%`".into(),
            grants: vec![Grant::Database(
                "shop".into(),
                vec!["ALL PRIVILEGES".into()],
            )],
        };
        for want in ["SELECT", "SHOW VIEW", "TRIGGER", "LOCK TABLES"] {
            assert!(held.has(&Need::Here(want), "shop", &[]), "{want}");
        }
    }

    /// Granted table by table, and covering every table there is, counts. Miss one and it
    /// does not — which is the honest answer, because the dump reads all of them.
    #[test]
    fn a_table_by_table_grant_counts_only_when_it_covers_every_table() {
        let tables = vec!["widget".to_owned(), "sale".to_owned()];
        let per_table = |named: &[&str]| Held {
            role: "`bk`@`%`".into(),
            grants: named
                .iter()
                .map(|name| Grant::Table("shop".into(), (*name).into(), vec!["SELECT".into()]))
                .collect(),
        };

        assert!(per_table(&["widget", "sale"]).has(&Need::Here("SELECT"), "shop", &tables));
        assert!(!per_table(&["widget"]).has(&Need::Here("SELECT"), "shop", &tables));
        // And an empty table list never turns "no grants at all" into a pass.
        assert!(!per_table(&[]).has(&Need::Here("SELECT"), "shop", &[]));
    }

    /// What `CURRENT_USER()` answers is not what `GRANT` will accept: `%` is not an
    /// identifier, so every statement this prints would be a syntax error unquoted.
    #[test]
    fn an_account_is_quoted_the_way_a_grant_statement_needs_it() {
        assert_eq!(quote_account("bk@%"), "`bk`@`%`");
        assert_eq!(quote_account("app_ro@10.0.0.%"), "`app_ro`@`10.0.0.%`");
        // A user name may contain `@`; a host name may not, so the split is on the last.
        assert_eq!(quote_account("a@b@localhost"), "`a@b`@`localhost`");
        assert_eq!(quote_account("weird`name@%"), "`weird``name`@`%`");
    }

    /// Every requirement in the printed table has something that satisfies it. A row with
    /// no mapping would silently read as "missing" on every server there is.
    #[test]
    fn every_requirement_has_a_grant_that_satisfies_it() {
        for (engine, family) in [
            (Engine::Mysql, Family::Mysql),
            (Engine::Mariadb, Family::Mariadb),
        ] {
            for requirement in for_engine(engine) {
                assert!(
                    !needs_of(family, requirement.id).is_empty(),
                    "{engine} lists {} and nothing satisfies it",
                    requirement.id
                );
            }
        }
    }
}
