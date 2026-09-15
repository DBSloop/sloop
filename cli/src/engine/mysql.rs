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
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::Instant;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use super::{
    Adapter, Capabilities, DumpSummary, Engine, ServerInfo, Table, TableCount, Target, Version,
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

    fn dump(&self, target: &Target<'_>, to: &Path) -> Outcome<DumpSummary> {
        // Before anything is written: is this the engine it was registered as?
        self.probe(target)?;

        if let Some(parent) = to.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| {
                Failure::new(
                    Exit::Dump,
                    format!("could not create {}: {error}", parent.display()),
                )
            })?;
        }

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

        let started = Instant::now();
        let bytes = self.run_the_dump(command, target, to)?;

        if bytes == 0 {
            return Err(Failure::new(
                Exit::Dump,
                format!(
                    "{} wrote nothing to {}",
                    self.family.dump_program(),
                    to.display()
                ),
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
    fn run_the_dump(&self, mut command: Command, target: &Target<'_>, to: &Path) -> Outcome<u64> {
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

        let written = Self::copy_the_dump(&mut child, to);

        let status = child.wait().map_err(|error| {
            Failure::new(
                Exit::Dump,
                format!("{} would not finish: {error}", self.family.dump_program()),
            )
        })?;
        let said = String::from_utf8_lossy(&draining.join().unwrap_or_default()).into_owned();

        if !status.success() {
            // The tool's own complaint outranks whatever went wrong writing the file: if
            // the dump failed, a short file is the symptom and not the cause.
            return Err(from_stderr(&self.tools.dump, &said, Exit::Dump, target));
        }

        written
    }

    /// Copy the dump program's output into `to`, correcting it on the way. See
    /// [`MysqlFamily::run_the_dump`].
    fn copy_the_dump(child: &mut Child, to: &Path) -> Outcome<u64> {
        let failed = |what: &str, error: &std::io::Error| {
            Failure::new(Exit::Dump, format!("{what} {}: {error}", to.display()))
        };

        let file = std::fs::File::create(to).map_err(|error| failed("could not create", &error))?;
        let mut out = BufWriter::with_capacity(256 * 1024, file);
        let mut reading =
            BufReader::with_capacity(256 * 1024, child.stdout.take().expect("stdout was piped"));

        // Bytes, never a `String`. A dump carries whatever the columns carry, and a `latin1`
        // table is not valid UTF-8 — reading it as text would replace the bytes it could not
        // decode and quietly corrupt the backup.
        let mut line = Vec::with_capacity(8 * 1024);
        let mut written = 0_u64;

        loop {
            line.clear();
            let read = reading
                .read_until(b'\n', &mut line)
                .map_err(|error| failed("could not read the dump for", &error))?;
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
            written += corrected.len() as u64;

            if had_newline {
                out.write_all(b"\n")
                    .map_err(|error| failed("could not write", &error))?;
                written += 1;
            }
        }

        out.flush()
            .map_err(|error| failed("could not finish writing", &error))?;
        Ok(written)
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
