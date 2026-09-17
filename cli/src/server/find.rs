//! What PostgreSQL this machine already has, and whether any of it is serving.
//!
//! **A server, not a client.** `psql` and `pg_dump` arrive on their own all the time — a
//! `postgresql-client` package, a libpq from Homebrew, the copy `R6` fetched. None of them
//! can hold a database. What says a directory holds a *server* is `pg_ctl` beside `initdb`
//! and `postgres`, so that is what is looked for.
//!
//! **Nothing here opens a socket to anywhere but this machine**, and the only questions
//! asked are of programs already on it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::engine::Version;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;
use crate::style;

use super::{LOOPBACK, Origin, Ready, SUPERUSER, Server, WANTED_MAJOR, make};

/// The three programs a directory needs before it counts as a server.
const SERVER_PROGRAMS: [&str; 3] = ["pg_ctl", "initdb", "postgres"];

/// PostgreSQL's own default port — the one the machine's own server would be on.
const DEFAULT_PORT: u16 = 5432;

/// One PostgreSQL server installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    /// Its `bin` directory.
    pub bin: PathBuf,
    /// What `pg_ctl --version` said.
    pub version: Version,
}

/// Every PostgreSQL server on this machine, newest first.
///
/// Public because `R19c5`'s Setup screen will want to show them, and because a machine where
/// this answers surprisingly is a machine somebody has to be able to see into.
#[must_use]
pub fn servers(global: &Path) -> Vec<Installation> {
    let mut found: Vec<Installation> = Vec::new();

    for bin in candidate_bins(global) {
        if found.iter().any(|already| already.bin == bin) {
            continue;
        }
        if let Some(version) = version_of(&bin) {
            found.push(Installation { bin, version });
        }
    }

    // Newest first, so "the 18 on this machine" is the newest 18 rather than whichever was
    // listed first.
    found.sort_by_key(|found| std::cmp::Reverse(found.version));
    found
}

/// The newest PostgreSQL 18 on this machine, if it has one.
#[must_use]
pub fn wanted(global: &Path) -> Option<Installation> {
    servers(global)
        .into_iter()
        .find(|found| found.version.major == WANTED_MAJOR)
}

/// The machine's own PostgreSQL 18, if it has one and it is already serving on 5432.
///
/// **Three things have to be true at once**, and the third is the one that is easy to
/// forget: something is listening on 5432, the superuser password opens it, and the server
/// that answered really is an 18. A PostgreSQL 17 serving on 5432 beside an installed-but-
/// stopped 18 would otherwise be accepted as "the 18 on 5432", and sloop would put its
/// schema somewhere it was never meant to go.
pub fn the_machines_own(global: &Path, asking: &make::Asking<'_>) -> Outcome<Option<Ready>> {
    let Some(installed) = wanted(global) else {
        return Ok(None);
    };

    // Not sloop's own cluster wearing the machine's clothes. That one lives under the global
    // store and is reached by its record, not by this path.
    if installed.bin.starts_with(super::home(global)) {
        return Ok(None);
    }

    if !is_serving(&installed.bin, DEFAULT_PORT) {
        crate::say!(
            "  {}",
            style::dim(&format!(
                "PostgreSQL {} is installed at {} but nothing is serving on {DEFAULT_PORT}",
                installed.version,
                installed.bin.display()
            ))
        );
        return Ok(None);
    }

    let server = Server {
        bin: installed.bin,
        data: None,
        port: DEFAULT_PORT,
        superuser: SUPERUSER.to_owned(),
        origin: Origin::Machines,
    };

    crate::say!(
        "{} PostgreSQL {} is serving on {DEFAULT_PORT}.",
        style::heading("Found:"),
        installed.version
    );
    let password = asking.superuser_password(&server)?;

    let Some(answered) = make::server_version(&server, &password)? else {
        return Err(Failure::new(
            Exit::Connect,
            format!(
                "{} would not accept {SUPERUSER}'s password",
                server.url("postgres")
            ),
        )
        .hint("the password is the one for the PostgreSQL that is already on this machine"));
    };

    if answered.major != WANTED_MAJOR {
        // The install is 18; the thing answering on 5432 is not. Not an error — it is a
        // machine running two PostgreSQLs, which is exactly what 5433 exists for.
        crate::say!(
            "  {}",
            style::dim(&format!(
                "what answered on {DEFAULT_PORT} is PostgreSQL {answered}, not {WANTED_MAJOR} — \
                 sloop will use a cluster of its own instead"
            ))
        );
        return Ok(None);
    }

    Ok(Some(Ready {
        server,
        password,
        made_now: false,
    }))
}

/// Is anything accepting connections on this port?
///
/// `pg_isready` and not a bare TCP connect: something else listening on 5432 would answer a
/// connect and is not a PostgreSQL. It is a question about this machine, asked of a program
/// on it, and it opens no socket to anywhere else.
#[must_use]
pub fn is_serving(bin: &Path, port: u16) -> bool {
    let program = bin.join(if cfg!(windows) {
        "pg_isready.exe"
    } else {
        "pg_isready"
    });

    Command::new(&program)
        .args(["-h", LOOPBACK, "-p", &port.to_string(), "-q"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Every directory that might hold a PostgreSQL server, in the order they are worth trying.
fn candidate_bins(global: &Path) -> Vec<PathBuf> {
    let mut directories = vec![super::fetched_bin(global)];

    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }

    // The same roots `tools` looks in for the client programs — `C:\Program Files\
    // PostgreSQL\18\bin`, `/usr/lib/postgresql/18/bin` and the rest. Shared rather than
    // copied, because a list of install locations that exists twice is a list that goes out
    // of date once.
    directories.extend(crate::tools::installed_directories(
        crate::engine::Engine::Postgres,
    ));

    directories
}

/// The version of the server in this directory, if it holds one at all.
fn version_of(bin: &Path) -> Option<Version> {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    if !SERVER_PROGRAMS
        .iter()
        .all(|name| bin.join(format!("{name}{suffix}")).is_file())
    {
        return None;
    }

    // `pg_ctl --version` rather than `postgres --version`: on Windows `postgres.exe` run
    // with no data directory has been known to pop a console window, and `pg_ctl` answers
    // the same question quietly.
    let output = Command::new(bin.join(format!("pg_ctl{suffix}")))
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;

    Version::from_tool_output(&String::from_utf8_lossy(&output.stdout))
}

/// A secret that came from a terminal, a pipe or a password manager — the three ways in.
///
/// Here rather than in `make` because it is a question about a server that already exists,
/// and `make` is about one that does not yet.
pub(super) fn from_stdin() -> Outcome<Secret> {
    use std::io::Read as _;

    let mut typed = String::new();
    std::io::stdin()
        .read_to_string(&mut typed)
        .map_err(|error| Failure::usage(format!("could not read the password: {error}")))?;

    // Exactly what was piped in, minus the newline a shell adds. A password is whatever
    // somebody's password manager printed, and trimming more than the line ending would
    // silently change it.
    Ok(Secret::new(
        typed
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_owned(),
    ))
}
