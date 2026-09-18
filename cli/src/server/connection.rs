//! Where sloop's own database is, so the person whose machine it is can open it themselves.
//!
//! **`R19c` hardened the cluster on purpose, and that is what left nothing to do this with.**
//! `make::require_a_password_from_now_on` rewrites every `trust` in `pg_hba.conf` to
//! `scram-sha-256`, so there is no passwordless path into `sloop_database`, not even a local
//! one — and both passwords were generated and filed where this machine keeps secrets, which
//! meant `psql -U sloop_db_admin` and DataGrip both asked for something nobody had ever been
//! shown. The hardening is right and stays. What was missing is a way for the owner of the
//! machine to be told their own credential, and that is all this is.
//!
//! **Telling somebody their own password is not what rule 3 forbids.** Rule 3 is about a
//! password *at rest* — in a config file, in a log, in a dump filename, in `ps`. It has never
//! forbidden printing one on a terminal somebody asked at; that is what every password
//! manager does. What it does forbid is letting it escape afterwards, so [`say_password`] is
//! the one line in this module that goes through [`crate::report::secret`], which has no
//! logging path in it at all.

use std::io::IsTerminal as _;
use std::path::Path;

use crate::failure::Outcome;
use crate::secret::{Route, Secret};
use crate::style;

use super::{LOOPBACK, Server, Settled, find, own::Own, record};

/// Everything a client needs to open sloop's own database, and nothing that opens it.
///
/// The password is deliberately not in here. This value is printed, recorded as JSON and
/// handed around; a secret inside it would be a secret in all three.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// Always [`LOOPBACK`] — sloop's own state has no business being reachable from the
    /// network, and `R19c` has never bound it anywhere else.
    pub host: &'static str,
    /// Which port the server answers on.
    pub port: u16,
    /// The database.
    pub database: String,
    /// The role to connect as, which owns every table in it.
    pub role: String,
    /// Where that role's password is kept — the route, never the value.
    pub route: Route,
    /// Whether the server is answering right now.
    pub answering: bool,
}

impl Connection {
    /// What a previous Setup settled, or `None` on a machine that has not been set up.
    ///
    /// **Nothing here reads the password**, and it is read-only in the other sense too: the
    /// record is parsed, `pg_isready` is asked whether anything is listening, and no cluster
    /// is started. `record::reopen` would start a stopped one, which is right for a command
    /// about to use the server and wrong for one that was asked where it is.
    pub fn of(global: &Path) -> Outcome<Option<Self>> {
        let (Some(server), Some(own), Some(route)) = (
            record::read_server(global)?,
            record::recorded_own(global)?,
            record::database_route(global)?,
        ) else {
            return Ok(None);
        };

        let answering = find::is_serving(&server.bin, server.port);
        Ok(Some(Self::to(&server, &own, route, answering)))
    }

    /// The connection Setup has just finished making.
    ///
    /// `answering` is `true` without asking, because `own::ensure` proved this exact
    /// connection a moment ago — running `pg_isready` to find out something already known
    /// would be a slower way of printing the same word.
    #[must_use]
    pub fn settled(settled: &Settled) -> Self {
        Self::to(
            &settled.ready.server,
            &settled.own,
            settled.route.clone(),
            true,
        )
    }

    fn to(server: &Server, own: &Own, route: Route, answering: bool) -> Self {
        Self {
            host: LOOPBACK,
            port: server.port,
            database: own.database.clone(),
            role: own.role.clone(),
            route,
            answering,
        }
    }

    /// How a client spells it, with no password anywhere in it.
    #[must_use]
    pub fn url(&self) -> String {
        format!(
            "postgres://{}@{}:{}/{}",
            self.role, self.host, self.port, self.database
        )
    }

    /// The same thing for a `--json` run. The password route is a word like `keyring`; there
    /// is no spelling of this document that carries a password.
    #[must_use]
    pub fn document(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "port": self.port,
            "database": self.database,
            "role": self.role,
            "url": self.url(),
            "password_route": self.route.as_field(),
            "answering": self.answering,
        })
    }

    /// Print it, as the six rows somebody copies into a client one at a time.
    ///
    /// Rows rather than the URL alone because a GUI asks for the parts separately: DataGrip
    /// has a host box, a port box, a database box and a user box, and somebody holding one
    /// string has to take it apart before they can fill them in.
    pub fn say(&self) {
        crate::say!();
        crate::say!(
            "{} {}",
            style::heading("sloop's own database"),
            if self.answering {
                style::in_hue(style::Hue::Ok, "answering")
            } else {
                style::in_hue(style::Hue::Warn, "not running")
            }
        );
        for (label, value) in [
            ("Host", self.host.to_owned()),
            ("Port", self.port.to_string()),
            ("Database", self.database.clone()),
            ("User", self.role.clone()),
            ("Password", format!("kept in {}", self.route.describe())),
            ("URL", self.url()),
        ] {
            // Padded before it is styled, so the invisible escape bytes never count towards
            // the column — the same rule `init`'s rows follow.
            crate::say!("  {}  {value}", style::label(&format!("{label:<8}")));
        }
    }
}

/// Why this run has nobody to print a password to.
///
/// **One definition, because two callers act on it and they must not drift.** Setup decides
/// whether to show the password it just invented, and `sloop server connection` decides
/// whether to refuse the one it was asked for — and a Setup that printed into a cron log
/// while the command refused to print into a pipe would be the same rule enforced twice with
/// two different answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    /// `--json`: the run is being parsed.
    Json,
    /// `--quiet`: the run is saying nothing.
    Quiet,
    /// The output is a file or a pipe rather than a terminal.
    NotATerminal,
}

impl Unread {
    /// How it reads at the end of *"a password is only ever printed for somebody to read,
    /// and …"*.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Json => "--json says this run is being parsed",
            Self::Quiet => "--quiet says this run is saying nothing",
            Self::NotATerminal => {
                "this one would land in a file or a pipe rather than on a terminal"
            }
        }
    }
}

/// Is there anybody at this run to print a password to? `None` when there is.
///
/// **Both streams have to be terminals, not just one.** A password is written to standard
/// error, so checking only standard output would let `2> secrets.txt` collect it with nothing
/// said — which is the very thing this exists to stop.
#[must_use]
pub fn nobody_is_reading() -> Option<Unread> {
    if crate::report::is_json() {
        return Some(Unread::Json);
    }
    if crate::report::is_quiet() {
        return Some(Unread::Quiet);
    }
    if std::io::stdout().is_terminal() && std::io::stderr().is_terminal() {
        return None;
    }
    Some(Unread::NotATerminal)
}

/// Print a password, on standard error, and **never anywhere it could be kept**.
///
/// [`crate::report::secret`] is the whole of why this is safe to call: it has no logging path
/// in it, so `--log-file` cannot pick the line up, and it writes to standard error, so a run
/// whose standard output is being piped somewhere does not pipe the password along with it.
///
/// `closing` is the line under it, which differs by caller — Setup says *copy it now*,
/// because it will not offer again; the command that was asked for it has nothing to add.
pub fn say_password(role: &str, password: &Secret, closing: &str) {
    crate::report::secret("");
    crate::report::secret(&format!(
        "{} {}",
        style::paint(role),
        style::dim("— its password:")
    ));
    crate::report::secret(&format!("  {}", password.expose()));
    crate::report::secret(&format!("  {}", style::dim(closing)));
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
