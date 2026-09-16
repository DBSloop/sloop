//! Everything sloop knows about talking to a database engine.
//!
//! **Adding an engine is meant to be a small, obvious job**, and this module is shaped so
//! that it is. To add one you touch four things and nothing else:
//!
//! 1. a variant on [`Engine`], with its default port and URL scheme;
//! 2. a module beside `postgres`, implementing [`Adapter`];
//! 3. one arm in [`adapter_for`];
//! 4. its [`Capabilities`], if they differ.
//!
//! The compiler finds the rest. [`adapter_for`] matches exhaustively and has no catch-all,
//! so a new variant fails to build until it is handled — which is the point: the list of
//! places to change is a compiler error, not something to remember.
//!
//! Nothing in here knows what a registry file looks like. The dependency runs one way:
//! `registry` builds a [`Target`] and hands it over. That is what keeps an engine's code
//! about the engine.
//!
//! Two rules run through all of it.
//!
//! **A password never reaches `argv`.** Connection details go in flags; the password goes
//! into the environment of the one child process that needs it, and nowhere wider.
//!
//! **The source of a copy is only ever read.** Nothing here issues a statement that
//! writes, not even to collect statistics. Counting is `count(*)` on a read-only
//! connection; [`Adapter::restore`] is the only method that changes anything, and what it
//! changes is the destination.
//!
//! Nothing here has a caller yet: R9's `backup` is the first command that reaches for an
//! adapter. The trait is settled now because three engines have to fit it and the shape is
//! easier to get right before two of them are written than after — which is why the module
//! is allowed to sit unused rather than each item carrying its own excuse.
#![allow(dead_code)]

pub mod mysql;
pub mod postgres;
pub mod privileges;

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod cluster_tests;
#[cfg(test)]
#[path = "mysql_cluster_tests.rs"]
mod mysql_cluster_tests;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::failure::Outcome;
use crate::secret::Secret;

/// The engines. Three today; the shape of this module is what keeps a fourth cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// PostgreSQL.
    Postgres,
    /// MySQL.
    Mysql,
    /// MariaDB — a separate engine, not an alias, because it ships `mariadb-dump`.
    Mariadb,
}

impl Engine {
    /// Every engine there is, for the places that have to cover all of them.
    pub const ALL: [Self; 3] = [Self::Postgres, Self::Mysql, Self::Mariadb];

    /// The port this engine listens on when nobody says otherwise.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Postgres => 5432,
            Self::Mysql | Self::Mariadb => 3306,
        }
    }

    /// The scheme this engine uses in a URL.
    #[must_use]
    pub const fn scheme(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
        }
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.scheme())
    }
}

/// The adapter for an engine.
///
/// No catch-all arm, on purpose. Add a variant to [`Engine`] and this stops compiling
/// until the new engine is handled, which is a better reminder than a comment.
///
/// It cannot fail, and says so. Every engine there is now has an adapter, and building one
/// is only ever a matter of naming the programs to run — whether those programs are on the
/// machine is a question for the first command that tries to run one, which is where the
/// failure has a database to name and an exit code to carry.
#[must_use]
pub fn adapter_for(engine: Engine) -> Box<dyn Adapter> {
    match engine {
        Engine::Postgres => Box::new(postgres::Postgres::default()),
        Engine::Mysql => Box::new(mysql::mysql()),
        Engine::Mariadb => Box::new(mysql::mariadb()),
    }
}

/// How a connection reads in a message: no password in it, and the same string a server's
/// own log would show.
#[must_use]
pub fn connection_string(
    engine: Engine,
    user: &str,
    host: &str,
    port: u16,
    database: &str,
) -> String {
    format!("{}://{user}@{host}:{port}/{database}", engine.scheme())
}

/// A database to act on, and the password to reach it with.
///
/// Plain fields rather than a borrowed registry entry, so that nothing in this module has
/// to know where the details came from — a registry file today, an interactive prompt or a
/// URL on the command line later.
pub struct Target<'a> {
    /// Which engine.
    pub engine: Engine,
    /// Host name or address.
    pub host: &'a str,
    /// Port.
    pub port: u16,
    /// The database name on the server.
    pub database: &'a str,
    /// The role to connect as.
    pub user: &'a str,
    /// Its password, already resolved through whichever of the four routes applies.
    pub password: &'a Secret,
}

impl Target<'_> {
    /// How this connection reads in a message.
    #[must_use]
    pub fn describe(&self) -> String {
        connection_string(self.engine, self.user, self.host, self.port, self.database)
    }
}

/// An engine version, compared the way the engines themselves compare.
///
/// PostgreSQL moved to a single-number major at 10, so `9.6` and `17.2` both have to parse
/// and order correctly. Only the major matters for the rule that bites — `pg_dump` refuses
/// a server newer than itself — but the minor is kept, because a message that says `17.2`
/// is more use than one that says `17`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    /// `17` in `17.2`, `9` in `9.6.24`.
    pub major: u32,
    /// `2` in `17.2`, `6` in `9.6.24`.
    pub minor: u32,
}

impl Version {
    /// Build one directly.
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// Pull a version out of whatever a tool printed for `--version`.
    ///
    /// That line is not a stable interface — `pg_dump (PostgreSQL) 17.2`, `psql (PostgreSQL)
    /// 18beta1`, `mysqldump  Ver 8.4.3 for Linux` — so this looks for the first thing shaped
    /// like a version rather than trusting the words around it.
    #[must_use]
    pub fn from_tool_output(text: &str) -> Option<Self> {
        text.split_whitespace().find_map(Self::from_number)
    }

    /// Parse `17.2`, `9.6.24`, `18beta1`, `17`.
    #[must_use]
    pub fn from_number(token: &str) -> Option<Self> {
        let digits = |field: &str| -> Option<u32> {
            let leading: String = field.chars().take_while(char::is_ascii_digit).collect();
            leading.parse().ok()
        };

        let mut parts = token.split('.');
        let major = digits(parts.next()?)?;
        let minor = parts.next().and_then(digits).unwrap_or(0);
        Some(Self { major, minor })
    }

    /// Read PostgreSQL's `server_version_num`.
    ///
    /// The encoding changed at 10: `170002` is 17.2, while `90624` is 9.6.24 — the older
    /// form packs major, minor and patch into the same six digits.
    #[must_use]
    pub const fn from_server_version_num(number: u32) -> Self {
        if number >= 100_000 {
            Self {
                major: number / 10_000,
                minor: number % 10_000,
            }
        } else {
            Self {
                major: number / 10_000,
                minor: (number / 100) % 100,
            }
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

/// What a server said when it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// The engine that answered.
    pub engine: Engine,
    /// Its version.
    pub version: Version,
    /// Whether the connection ended up encrypted.
    pub tls: bool,
}

/// A table, schema and all.
///
/// MySQL has no schemas and uses the database name in that position, which keeps one type
/// honest across all three engines instead of two nearly-identical ones.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Table {
    /// The schema it lives in.
    pub schema: String,
    /// The table's own name.
    pub name: String,
}

impl fmt::Display for Table {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.schema, self.name)
    }
}

/// A table and how many rows are really in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCount {
    /// Which table.
    pub table: Table,
    /// `count(*)`, never an estimate.
    pub rows: u64,
}

/// What an engine can and cannot do, said out loud.
///
/// This type is why the trait does not have to lie. `mysqldump` writes a stream of SQL —
/// there is no format a restore can read selectively, and no way to restore in parallel —
/// and a caller that asks first behaves correctly on all three engines without knowing
/// which one it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// A dump format a restore can read selectively.
    pub custom_format: bool,
    /// Whether a restore can use more than one connection.
    pub parallel_restore: bool,
}

/// Where a dump ended up, and what it cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpSummary {
    /// The file that was written.
    pub path: PathBuf,
    /// Its size in bytes.
    pub bytes: u64,
    /// How long it took.
    pub took: std::time::Duration,
}

/// Everything sloop asks of a database engine.
pub trait Adapter {
    /// Which engine this is.
    fn engine(&self) -> Engine;

    /// What it can do.
    fn capabilities(&self) -> Capabilities;

    /// Connect, and report what answered.
    ///
    /// The first thing every command does, so that a connection failure is reported as one
    /// rather than as whatever it was interrupting.
    fn probe(&self, target: &Target<'_>) -> Outcome<ServerInfo>;

    /// Every table a dump would carry.
    fn tables(&self, target: &Target<'_>) -> Outcome<Vec<Table>>;

    /// Exact `count(*)` for every table. Never an estimate.
    fn row_counts(&self, target: &Target<'_>) -> Outcome<Vec<TableCount>>;

    /// Write a dump of `target` to `to`. Reads the source and nothing else.
    fn dump(&self, target: &Target<'_>, to: &Path) -> Outcome<DumpSummary>;

    /// Load a dump written by [`Adapter::dump`] into `target`.
    fn restore(&self, target: &Target<'_>, from: &Path) -> Outcome<()>;

    /// What a role must be able to do on this engine, and what breaks without it.
    ///
    /// Static, and needs no connection: it is the documented minimum rather than an
    /// answer about one server. `sloop doctor` prints it where there is nothing
    /// registered to check, and the site publishes the same table.
    fn required_privileges(&self) -> &'static [privileges::Requirement] {
        privileges::for_engine(self.engine())
    }

    /// What this engine's own manual asks a backup role for that sloop does not need.
    fn waived_privileges(&self) -> &'static [privileges::Waived] {
        privileges::waived_by(self.engine())
    }

    /// Ask the live connection which of those this role actually holds.
    ///
    /// **A read, like everything else here.** It inspects catalogues and grant tables and
    /// changes nothing — including nothing about the role's own privileges, which are an
    /// administrator's to grant and never a backup tool's to take.
    fn check_privileges(&self, target: &Target<'_>) -> Outcome<privileges::Report>;
}
