//! The PostgreSQL 18 sloop keeps its own state in.
//!
//! **Gitea's shape, which the owner named.** sloop stops keeping its state in files and
//! keeps it in a database of its own. This module is the first half of getting there: making
//! sure there *is* a PostgreSQL 18 on the machine that sloop may use. What goes in it —
//! `sloop_database`, the role, the schema — is `R19c2` and `R19c3`.
//!
//! Three machines, three answers, and the third is the one the port number exists for:
//!
//! ```text
//! PostgreSQL 18 already serving on 5432   use it as it stands, and ask for its superuser
//! PostgreSQL 18 on disk but not serving   sloop's own cluster on 5433, from those binaries
//! an older PostgreSQL, or none at all     get 18, then sloop's own cluster on 5433
//! ```
//!
//! **5433, so nothing is ever disturbed.** A machine that already runs PostgreSQL keeps
//! running it on 5432, and a machine with an older PostgreSQL gets 18 *beside* it rather
//! than instead of it. sloop's cluster is its own directory, its own port and its own
//! superuser password, and uninstalling sloop is deleting one directory.
//!
//! **The connection details live outside the database, because they cannot live in it.**
//! `<global>/server.toml` holds where the server is and which of `R3`'s four routes its
//! superuser password takes — never the password, which is rule 3 and is why the file can be
//! pasted into an issue.
//!
//! **Nothing here links a network client.** Getting the binaries is `tools::acquire`, which
//! shells out to the system's own `curl`, and everything else is `initdb`, `pg_ctl` and
//! `psql` — programs, not libraries. `cargo tree` is unchanged by this module.

pub mod find;
pub mod make;
pub mod own;
pub mod record;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cluster_tests;

use std::path::{Path, PathBuf};

use crate::failure::Outcome;
use crate::secret::Secret;
use crate::style;

/// The major sloop keeps its state in. Not "18 or newer": the schema `R19c3` writes is
/// written against 18, and a machine that has 19 and not 18 gets 18 beside it for the same
/// reason a machine with 17 does.
pub const WANTED_MAJOR: u32 = 18;

/// The port sloop's own cluster listens on.
///
/// **Chosen so it can never collide.** 5432 belongs to whatever the machine already runs,
/// and the whole point of a second cluster is that installing sloop changes nothing about
/// the first one.
pub const SLOOPS_PORT: u16 = 5433;

/// The role sloop's own cluster is created with. The conventional name, so that anything a
/// human does with this cluster by hand behaves the way they expect.
pub const SUPERUSER: &str = "postgres";

/// The address sloop's own cluster answers on, and the only one.
///
/// A database holding one machine's own state has no business being reachable from the
/// network. This is not a hardening choice that could make a feature refuse — nothing sloop
/// does reaches this cluster from anywhere else — so rule 0d has nothing to say about it.
pub const LOOPBACK: &str = "127.0.0.1";

/// Where sloop's own PostgreSQL lives, under the global store.
#[must_use]
pub fn home(global: &Path) -> PathBuf {
    global.join("postgres")
}

/// Where a cluster sloop made keeps its data.
#[must_use]
pub fn data_dir(global: &Path) -> PathBuf {
    home(global).join("data")
}

/// Where binaries sloop fetched itself are unpacked to.
#[must_use]
pub fn fetched_dir(global: &Path) -> PathBuf {
    home(global).join(WANTED_MAJOR.to_string())
}

/// The `bin` of a fetched PostgreSQL. Named here because [`crate::tools`] looks in it too:
/// a machine that has just been given a whole PostgreSQL has `psql` and `pg_dump` on it, and
/// offering to download them again would be absurd.
#[must_use]
pub fn fetched_bin(global: &Path) -> PathBuf {
    fetched_dir(global).join("bin")
}

/// Whose server this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The machine's own PostgreSQL 18, used as it stands. sloop never starts, stops or
    /// reconfigures it — it is somebody else's server and sloop is a guest on it.
    Machines,
    /// A cluster sloop created, under the global store, on [`SLOOPS_PORT`].
    Sloops,
}

impl Origin {
    /// How it reads in a report.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Machines => "this machine's own PostgreSQL 18",
            Self::Sloops => "sloop's own cluster",
        }
    }
}

/// A PostgreSQL 18 sloop can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// The directory `initdb`, `pg_ctl` and `psql` are in.
    pub bin: PathBuf,
    /// The cluster's data directory — only for one sloop made. The machine's own is not
    /// sloop's to start or stop, so there is nothing here to remember about it.
    pub data: Option<PathBuf>,
    /// Which port it answers on.
    pub port: u16,
    /// The superuser sloop connects as.
    pub superuser: String,
    /// Whose it is.
    pub origin: Origin,
}

impl Server {
    /// One of this server's programs.
    #[must_use]
    pub fn program(&self, name: &str) -> PathBuf {
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        self.bin.join(format!("{name}{suffix}"))
    }

    /// How a connection to it is spelled, with no password anywhere in it.
    #[must_use]
    pub fn url(&self, database: &str) -> String {
        format!(
            "postgres://{}@{LOOPBACK}:{}/{database}",
            self.superuser, self.port
        )
    }
}

/// A server, sloop's own database on it, and the two passwords that open them.
///
/// The two travel together because neither is any use alone, and they are separate from
/// [`Server`] so that the half which is written to a file cannot accidentally be the half
/// that holds a secret.
///
/// `Debug` is safe by construction: [`Secret`]'s own `Debug` prints `<redacted>`, which is
/// the property that lets this type appear in an assertion message at all.
#[derive(Debug)]
pub struct Ready {
    /// Where it is.
    pub server: Server,
    /// How to get in.
    pub password: Secret,
    /// True when this run is what made it. The caller says so; `R19c5`'s screen will say
    /// more about it than a one-line report does.
    pub made_now: bool,
}

/// Everything Setup settled: where sloop's state lives, and how to open it.
///
/// **The two halves are separate because one of them is written to a file.** [`Server`] and
/// [`own::Own`] are paths and names and can be printed; the passwords beside them cannot, and
/// keeping them in a different type is what makes writing the wrong one an error rather than
/// an oversight.
#[derive(Debug)]
pub struct Settled {
    /// The server.
    pub ready: Ready,
    /// sloop's own database on it.
    pub own: own::Own,
    /// The password of the role that owns it.
    ///
    /// **Nothing reads it yet, and that is the seam.** `R19c3` opens this database to create
    /// its tables, and it opens it as the owning role rather than as the superuser — so the
    /// password leaves Setup from here rather than being fetched again. It is allowed to sit
    /// unread until then for the same reason `lock::Held::path` is: the value belongs to the
    /// type, and the alternative is a caller that has to go and find it.
    #[allow(dead_code)]
    pub password: Secret,
}

/// Find a PostgreSQL 18 sloop can use, making one if the machine has none.
///
/// **Idempotent, because Setup has to be re-runnable.** A second call finds the record from
/// the first, starts the cluster if it is stopped, and returns. Nothing is downloaded twice
/// and nothing is initialised twice.
pub fn ensure(global: &Path, asking: &make::Asking<'_>) -> Outcome<Ready> {
    // What a previous run settled, first. Re-deriving it would risk answering differently on
    // a machine whose PATH has changed since Setup ran.
    if let Some(ready) = record::reopen(global)? {
        return Ok(ready);
    }

    // The machine's own, if it has one and it is already serving. Used exactly as it stands:
    // not started, not stopped, not reconfigured.
    if let Some(guest) = find::the_machines_own(global, asking)? {
        record::write(global, &guest.server, &guest.password)?;
        return Ok(guest);
    }

    // Otherwise sloop's own, on its own port, out of whichever 18 binaries this machine can
    // be given — the ones already on it, or the ones `tools::acquire` fetches.
    let made = make::sloops_own(global)?;
    record::write(global, &made.server, &made.password)?;
    Ok(made)
}

/// Find a PostgreSQL 18, and make sure sloop's own database is on it.
///
/// **The whole of Setup as it stands**, and re-runnable: a second call finds the server, the
/// role and the database all already there, reads both passwords back from wherever this
/// machine keeps secrets, proves the connection and returns — with nothing typed.
pub fn set_up(global: &Path, asking: &make::Asking<'_>) -> Outcome<Settled> {
    let ready = ensure(global, asking)?;
    let (own, password) = own::ensure(global, &ready.server, &ready.password)?;

    Ok(Settled {
        ready,
        own,
        password,
    })
}

/// Say what `ensure` settled, in the two lines a report wants.
pub fn announce(ready: &Ready) {
    crate::say!(
        "{} {}",
        style::heading(if ready.made_now {
            "Prepared:"
        } else {
            "Using:"
        }),
        ready.server.origin.describe()
    );
    crate::say!(
        "  {}  {}",
        style::paint(&ready.server.url("postgres")),
        style::dim(&format!("binaries at {}", ready.server.bin.display()))
    );
    if let Some(data) = &ready.server.data {
        crate::say!(
            "  {}",
            style::dim(&format!("cluster at {}", data.display()))
        );
    }
}

/// Say where sloop's own state lives, once Setup has settled it.
pub fn announce_own(settled: &Settled) {
    crate::say!(
        "{} {}",
        style::heading("State:"),
        style::paint(&settled.own.url(&settled.ready.server))
    );
    crate::say!(
        "  {}",
        style::dim(&format!(
            "owned by {}, whose password is kept where this machine keeps secrets",
            settled.own.role
        ))
    );
}
