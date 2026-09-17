//! Finding the client tools, and knowing what is there.
//!
//! sloop does not bundle `pg_dump` or `mysqldump`. `mysqldump` is GPL v2 against an
//! MIT/Apache tool, three platforms times two architectures turns a 5 MB binary into a
//! 150 MB release set, and a bundled `pg_dump` older than somebody's server simply fails.
//! So the tools are the ones already on the machine, and where there are none this module
//! offers — once, and only with an answer — to go and get them. That is [`acquire`].
//!
//! **Everything here is local.** Looking for a program and asking it its version needs no
//! network, so `sloop doctor` never opens a socket and there is no version ping hiding in
//! a health check. The one path that does reach the internet is `acquire`, it runs only
//! after somebody says yes, and it shells out to the system's own `curl` rather than
//! linking anything that could.
//!
//! **The newest copy wins, wherever it came from.** A machine can easily have three
//! `pg_dump`s — one on `PATH`, one a package manager put somewhere off it, one sloop
//! fetched itself — and picking by position would mean a tool sloop downloaded *because*
//! the system one was too old still losing to the system one. Picking by version is the
//! rule that gives the right answer in every arrangement, and `sloop doctor` prints all of
//! them so the choice is never a mystery.

pub mod acquire;
pub mod releases;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::engine::{Engine, Version, mysql, postgres};

/// One of the programs sloop drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tool {
    /// PostgreSQL's dump program.
    PgDump,
    /// PostgreSQL's restore program.
    PgRestore,
    /// PostgreSQL's client.
    Psql,
    /// MySQL's dump program.
    MysqlDump,
    /// MySQL's client.
    Mysql,
    /// MariaDB's dump program — a different binary, which is why MariaDB is its own engine.
    MariadbDump,
    /// MariaDB's client.
    Mariadb,
}

impl Tool {
    /// Every tool there is.
    pub const ALL: [Self; 7] = [
        Self::PgDump,
        Self::PgRestore,
        Self::Psql,
        Self::MysqlDump,
        Self::Mysql,
        Self::MariadbDump,
        Self::Mariadb,
    ];

    /// The ones an engine cannot work without.
    #[must_use]
    pub const fn needed_by(engine: Engine) -> &'static [Self] {
        match engine {
            Engine::Postgres => &[Self::PgDump, Self::PgRestore, Self::Psql],
            Engine::Mysql => &[Self::MysqlDump, Self::Mysql],
            Engine::Mariadb => &[Self::MariadbDump, Self::Mariadb],
        }
    }

    /// What the program is called, without any extension.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::PgDump => "pg_dump",
            Self::PgRestore => "pg_restore",
            Self::Psql => "psql",
            Self::MysqlDump => "mysqldump",
            Self::Mysql => "mysql",
            Self::MariadbDump => "mariadb-dump",
            Self::Mariadb => "mariadb",
        }
    }

    /// Which engine it belongs to.
    #[must_use]
    pub const fn engine(self) -> Engine {
        match self {
            Self::PgDump | Self::PgRestore | Self::Psql => Engine::Postgres,
            Self::MysqlDump | Self::Mysql => Engine::Mysql,
            Self::MariadbDump | Self::Mariadb => Engine::Mariadb,
        }
    }

    /// What it is for, in a word, for the report.
    #[must_use]
    pub const fn what_for(self) -> &'static str {
        match self {
            Self::PgDump | Self::MysqlDump | Self::MariadbDump => "taking a dump",
            Self::PgRestore => "loading one back",
            Self::Psql | Self::Mysql | Self::Mariadb => "connecting, counting and restoring",
        }
    }

    /// The names this program has shipped under, newest first.
    ///
    /// MariaDB renamed every one of its tools at 10.5 and kept the old names as links;
    /// 11 made the new names the real ones. A machine with MariaDB 10.4 has `mysqldump`
    /// and no `mariadb-dump`, and reporting that as "MariaDB is not installed" would be
    /// wrong on a machine where it plainly is.
    #[must_use]
    pub const fn also_known_as(self) -> &'static [&'static str] {
        match self {
            Self::MariadbDump => &["mariadb-dump", "mysqldump"],
            Self::Mariadb => &["mariadb", "mysql"],
            _ => &[],
        }
    }

    /// The file name to look for on this platform.
    fn file_names(self) -> Vec<String> {
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let mut names = vec![format!("{}{suffix}", self.program())];
        for alias in self.also_known_as().iter().skip(1) {
            names.push(format!("{alias}{suffix}"));
        }
        names
    }

    /// Read the version out of whatever this program printed.
    ///
    /// The two families print differently enough that one parser gets MariaDB wrong — see
    /// [`mysql::version_of_a_client_tool`] — so each is asked its own way.
    fn read_version(self, printed: &str) -> Option<Version> {
        match self.engine() {
            Engine::Postgres => Version::from_tool_output(printed),
            Engine::Mysql | Engine::Mariadb => mysql::version_of_a_client_tool(printed),
        }
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.program())
    }
}

/// How a copy of a program came to be on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// On `PATH`, which is what the user's own shell would run.
    OnPath,
    /// In the directory sloop keeps what it fetched itself.
    Fetched,
    /// Somewhere a package manager or an installer puts things, off `PATH`.
    Installed,
}

impl Source {
    /// How it reads in the report.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::OnPath => "on PATH",
            Self::Fetched => "fetched by sloop",
            Self::Installed => "installed, off PATH",
        }
    }
}

/// One copy of one program, and what it said about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Where it is.
    pub path: PathBuf,
    /// How it got there.
    pub source: Source,
    /// What `--version` said, when it could be read at all.
    pub version: Option<Version>,
}

impl Candidate {
    /// Newest first, and among equals the one the user's own shell would have run.
    fn rank(&self) -> (std::cmp::Reverse<Option<Version>>, u8) {
        let position = match self.source {
            Source::OnPath => 0,
            Source::Fetched => 1,
            Source::Installed => 2,
        };
        (std::cmp::Reverse(self.version), position)
    }
}

/// Every copy of every program this machine has, and which one sloop will use.
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    found: BTreeMap<Tool, Vec<Candidate>>,
}

impl Inventory {
    /// Look for everything. This is what `sloop doctor` reports on.
    #[must_use]
    pub fn everything(fetched_into: &Path) -> Self {
        Self::of(&Tool::ALL, fetched_into)
    }

    /// Look only for what one engine needs, which is all a command about one database has
    /// any reason to ask.
    #[must_use]
    pub fn for_engine(engine: Engine, fetched_into: &Path) -> Self {
        Self::of(Tool::needed_by(engine), fetched_into)
    }

    fn of(wanted: &[Tool], fetched_into: &Path) -> Self {
        let mut found = BTreeMap::new();
        for &tool in wanted {
            let mut candidates = look_for(tool, fetched_into);
            candidates.sort_by_key(Candidate::rank);
            candidates.dedup_by(|a, b| a.path == b.path);
            found.insert(tool, candidates);
        }
        Self { found }
    }

    /// The copy sloop will use, if there is one.
    #[must_use]
    pub fn best(&self, tool: Tool) -> Option<&Candidate> {
        self.found.get(&tool).and_then(|found| found.first())
    }

    /// Every copy, newest first.
    #[must_use]
    pub fn every(&self, tool: Tool) -> &[Candidate] {
        self.found.get(&tool).map_or(&[], Vec::as_slice)
    }

    /// The tools this engine needs and does not have.
    #[must_use]
    pub fn missing_for(&self, engine: Engine) -> Vec<Tool> {
        Tool::needed_by(engine)
            .iter()
            .copied()
            .filter(|&tool| self.best(tool).is_none())
            .collect()
    }

    /// Is this engine usable at all?
    #[must_use]
    pub fn has_everything_for(&self, engine: Engine) -> bool {
        self.missing_for(engine).is_empty()
    }

    /// The adapter for this engine, pointed at the programs that were actually found.
    ///
    /// R9's `backup` is the first command to reach for this; today only the tests do, which
    /// is why it is allowed to sit here rather than being written twice later. The three
    /// below are the same shape and the same reason.
    ///
    /// This is the seam R4 was shaped for, closed from the `tools` side rather than the
    /// `engine` side. `engine` still knows nothing about discovery — it is handed paths —
    /// so the dependency runs one way and an engine's code stays about the engine.
    #[allow(dead_code)]
    #[must_use]
    pub fn adapter_for(&self, engine: Engine) -> Box<dyn crate::engine::Adapter> {
        match engine {
            Engine::Postgres => Box::new(postgres::Postgres::new(self.postgres())),
            Engine::Mysql => Box::new(mysql::MysqlFamily::new(
                mysql::Family::Mysql,
                self.mysql_family(mysql::Family::Mysql),
            )),
            Engine::Mariadb => Box::new(mysql::MysqlFamily::new(
                mysql::Family::Mariadb,
                self.mysql_family(mysql::Family::Mariadb),
            )),
        }
    }

    /// Where to run each of PostgreSQL's programs from.
    ///
    /// Falls back to the bare name, which is what `PATH` lookup at spawn time does. That
    /// keeps the adapter working on a machine this module could not see into — a `PATH`
    /// entry that cannot be listed, a program behind a shim — rather than refusing on the
    /// strength of not having found something.
    #[allow(dead_code)]
    #[must_use]
    pub fn postgres(&self) -> postgres::Tools {
        postgres::Tools {
            dump: self.program_path(Tool::PgDump),
            restore: self.program_path(Tool::PgRestore),
            query: self.program_path(Tool::Psql),
        }
    }

    /// The same, for whichever of the two MySQL-family engines is asking.
    #[allow(dead_code)]
    #[must_use]
    pub fn mysql_family(&self, family: mysql::Family) -> mysql::Tools {
        let (dump, client) = match family {
            mysql::Family::Mysql => (Tool::MysqlDump, Tool::Mysql),
            mysql::Family::Mariadb => (Tool::MariadbDump, Tool::Mariadb),
        };
        mysql::Tools {
            dump: self.program_path(dump),
            client: self.program_path(client),
        }
    }

    #[allow(dead_code)]
    fn program_path(&self, tool: Tool) -> PathBuf {
        self.best(tool).map_or_else(
            || PathBuf::from(tool.program()),
            |candidate| candidate.path.clone(),
        )
    }
}

/// Every copy of one program this machine has.
fn look_for(tool: Tool, fetched_into: &Path) -> Vec<Candidate> {
    let names = tool.file_names();
    let mut found = Vec::new();

    let mut consider = |directory: &Path, source: Source| {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                found.push(Candidate {
                    version: version_of(&candidate, tool),
                    path: candidate,
                    source,
                });
                // One name per directory. On a MariaDB install both names are there, and
                // they are the same program twice.
                break;
            }
        }
    };

    for directory in path_entries() {
        consider(&directory, Source::OnPath);
    }
    consider(fetched_into, Source::Fetched);
    // **The PostgreSQL sloop keeps its own state in is a whole PostgreSQL.** Its `bin` holds
    // `psql` and `pg_dump` beside the server, so a machine that has just been given one has
    // the client tools too — and offering to download a third of a gigabyte of them again
    // would be absurd. `fetched_into` is `<global>/tools/bin`, so the global store is its
    // grandparent.
    if let Some(global) = fetched_into.parent().and_then(Path::parent) {
        consider(&crate::server::fetched_bin(global), Source::Fetched);
    }
    for directory in installed_directories(tool.engine()) {
        consider(&directory, Source::Installed);
    }

    found
}

/// The directories on `PATH`, in the order the shell searches them.
fn path_entries() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// Ask a program its version.
///
/// Standard input closed and both streams collected, because a program that stops to ask
/// something during `--version` would otherwise hang `sloop doctor` — and a health check
/// that hangs is worse than one that says nothing.
fn version_of(program: &Path, tool: Tool) -> Option<Version> {
    let output = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;

    let printed = String::from_utf8_lossy(&output.stdout);
    tool.read_version(&printed)
}

/// The places each engine's installers put things that are not on `PATH`.
///
/// Two shapes. A directory that is simply the `bin`, and a root holding one directory per
/// version — `C:\Program Files\PostgreSQL\17\bin` — where every version is worth finding,
/// because which of them is newest is the question this module exists to answer.
///
/// `pub(crate)` for `server::find`, which asks the same question about a *server* rather than
/// a client and would otherwise keep a second copy of this list that went stale on its own.
pub(crate) fn installed_directories(engine: Engine) -> Vec<PathBuf> {
    let (direct, versioned): (&[&str], &[&str]) = match engine {
        Engine::Postgres => (
            &[
                "/usr/local/pgsql/bin",
                "/opt/homebrew/opt/libpq/bin",
                "/usr/local/opt/libpq/bin",
            ],
            &[
                "/usr/lib/postgresql",
                "/opt/homebrew/opt",
                "/usr/local/opt",
                "/Applications/Postgres.app/Contents/Versions",
                "C:/Program Files/PostgreSQL",
            ],
        ),
        Engine::Mysql => (
            &[
                "/usr/local/mysql/bin",
                "/opt/homebrew/opt/mysql-client/bin",
                "/usr/local/opt/mysql-client/bin",
            ],
            &[
                "/opt/homebrew/opt",
                "/usr/local/opt",
                "C:/Program Files/MySQL",
            ],
        ),
        Engine::Mariadb => (
            &[
                "/usr/local/mariadb/bin",
                "/opt/homebrew/opt/mariadb/bin",
                "/usr/local/opt/mariadb/bin",
            ],
            &["/opt/homebrew/opt", "/usr/local/opt", "C:/Program Files"],
        ),
    };

    let mut directories: Vec<PathBuf> = direct.iter().map(PathBuf::from).collect();

    for root in versioned {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join("bin");
            if candidate.is_dir() {
                directories.push(candidate);
            }
        }
    }

    directories
}
