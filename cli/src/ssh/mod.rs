//! Reaching a database that is only reachable from the server — `R19e`.
//!
//! **A local port forward, and nothing cleverer.** sloop opens one SSH connection to the
//! server and binds a port on `127.0.0.1` that comes out on the far side at the database.
//! Every client program `R4`–`R5` already shells out to — `psql`, `pg_dump`, `pg_restore`,
//! `mysql`, `mysqldump`, `mariadb-dump` — then connects to `127.0.0.1:<local port>` and knows
//! nothing about SSH. Not one adapter changes; the only thing that changes is which address
//! it is handed.
//!
//! **The connection is held for the session, which is the owner's whole point.** One
//! long-lived `ssh -N -L …` per remote server, opened the first time something needs it and
//! closed when sloop exits. Ten commands in a menu session authenticate once — *"if he wants
//! to query or something like that then it will login after each task which is not a proper
//! apprach"*.
//!
//! **The system's `ssh`, never an SSH crate**, and it is the same decision `R6` made about
//! `curl`:
//!
//! - **The guarantee survives literally.** `cargo tree` still shows nothing in the binary
//!   that can open a socket to anywhere. An embedded SSH client could, and the promise above
//!   the fold would stop being true even though the CI grep — which looks for HTTP clients —
//!   would still pass. A claim that holds only because the check is narrow is not worth
//!   making.
//! - **The user's own SSH already works.** `~/.ssh/config`, `ProxyJump` through a bastion,
//!   `IdentityFile`, the agent, a hardware key, `Match` blocks, a corporate CA. Anybody whose
//!   `ssh user@server` works gets a working sloop with nothing else configured.
//! - **`known_hosts` stays OpenSSH's**, and is not weakened by a single option. See
//!   [`askpass`] for the part of that which turned out to need defending on purpose.
//!
//! **All of it is wired now.** The registry holds the settings — `db add --ssh-host`, five
//! columns on `registered_database`, a [`Reach`] inside every registered database — and
//! every command that reaches a database asks [`crate::commands::reach`] where to dial
//! instead of reading an address off the record. [`tunnel`] opens and holds the forwards,
//! [`askpass`] answers `ssh` and refuses the one question it must never answer, and
//! [`health`] is what `doctor` reports.

pub mod askpass;
pub mod health;
pub mod tunnel;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::path::PathBuf;

use crate::secret::Route;

/// The port `ssh` listens on when nobody says otherwise.
pub const DEFAULT_PORT: u16 = 22;

/// The address a forward is bound on, and the only one.
///
/// A port that tunnels into somebody's private database has no business being reachable from
/// the network. This is not a hardening choice that could make a feature refuse — nothing
/// sloop does reaches a forward from anywhere else — so rule 0d has nothing to say about it.
pub const LOOPBACK: &str = "127.0.0.1";

/// Where the SSH server is and who to be on it.
///
/// **Not one field of this is a secret**, which is what lets the whole struct be written into
/// the registry and printed in a report. A private key *path* is a path; the key it names is
/// never read by sloop and never copied. The passphrase that opens it, if there is one, is a
/// [`Route`] on [`Through`] and never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// The server's name or address, as `ssh` would take it.
    pub host: String,
    /// Its port.
    pub port: u16,
    /// Who to be. `None` leaves it to `~/.ssh/config` and then to the local username, which
    /// is what plain `ssh server` does.
    pub user: Option<String>,
    /// `-i`, when a particular key is wanted. `None` leaves it to the agent and to
    /// `~/.ssh/config`, which is the documented default.
    pub identity: Option<PathBuf>,
}

impl Server {
    /// `user@host`, or `host` when the user is left to `ssh`.
    #[must_use]
    pub fn destination(&self) -> String {
        match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// How it reads in a report. Carries no secret, because nothing here is one.
    #[must_use]
    pub fn describe(&self) -> String {
        if self.port == DEFAULT_PORT {
            self.destination()
        } else {
            format!("{}:{}", self.destination(), self.port)
        }
    }

    /// Which login this is — the one string that answers two questions.
    ///
    /// **Ten commands against one server authenticate once**, and this is what makes that
    /// true: two databases whose servers give the same key share one held connection. The
    /// identity is part of it because two entries naming different keys are two different
    /// logins; the passphrase route is not, because it opens the same key.
    ///
    /// **It is also where that passphrase is filed**, in the keyring or in the encrypted
    /// store, for exactly the same reason — a passphrase belongs to a key on a server, not
    /// to a database, so five databases behind one bastion ask for it once and store it
    /// once. It is written the way [`crate::engine::connection_string`] writes a database's
    /// key, because it ends up in the same places: a Credential Manager entry somebody
    /// reads, a name inside the sealed store.
    ///
    /// **It carries no secret**, which is what lets it be printed. A host, a port, a
    /// username and a path to a key are all in `ps` the moment `ssh` runs.
    #[must_use]
    pub fn credential_key(&self) -> String {
        let mut key = format!("ssh://{}:{}", self.destination(), self.port);
        if let Some(identity) = &self.identity {
            key.push('#');
            key.push_str(&identity.display().to_string());
        }
        key
    }
}

/// A database reached through one of those.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Through {
    /// The server in between.
    pub server: Server,
    /// Where the key's passphrase — or the account's password — comes from.
    ///
    /// **`None` is the documented default and the one to aim for**: with `ssh-agent` or
    /// Pageant holding the key, sloop never sees a secret at all. A route here is one of
    /// `R3`'s four, exactly as a database's password is, and is never a value.
    pub secret: Option<Route>,
}

impl Through {
    /// How this reads in a listing or a confirmation. Carries no secret — see
    /// [`Server::credential_key`] for why none of it could.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.secret {
            Some(route) => format!(
                "over {}, unlocked from {}",
                self.server.describe(),
                route.describe()
            ),
            // Said as what it *is* rather than as "nothing", because an empty half of a
            // sentence reads like a missing setting. A key with no passphrase and a key in
            // an agent look identical from here and both are correct.
            None => format!("over {}, from the agent", self.server.describe()),
        }
    }

    /// The arguments `ssh` is run with to hold one forward open.
    ///
    /// **`-N`** because there is no command to run — the forward is the whole point.
    ///
    /// **`ExitOnForwardFailure=yes`** so a forward that could not be bound is a failure
    /// rather than a connection that silently is not carrying anything. Without it, `ssh`
    /// stays up, the client program connects to a local port nothing is listening on, and the
    /// error it reports is about the database rather than about the tunnel.
    ///
    /// **`ServerAliveInterval`** so a connection held for a long menu session notices a
    /// server that has gone away, rather than hanging the next command on a dead socket.
    ///
    /// **`BatchMode=yes` only when there is no secret to send.** It is rule 4's guard — it
    /// stops `ssh` asking anything of its own — but it also disables the askpass helper, so
    /// setting it unconditionally would make a configured passphrase unusable. When there
    /// *is* a secret, [`askpass`] is what keeps the run from ever touching a terminal, and it
    /// is the stronger guarantee of the two: `ssh` is not merely forbidden to ask, it is
    /// pointed at something that answers.
    #[must_use]
    pub fn arguments(
        &self,
        local_port: u16,
        database_host: &str,
        database_port: u16,
    ) -> Vec<String> {
        let mut arguments = vec![
            "-N".to_owned(),
            "-o".to_owned(),
            "ExitOnForwardFailure=yes".to_owned(),
            "-o".to_owned(),
            "ServerAliveInterval=30".to_owned(),
            "-o".to_owned(),
            "ServerAliveCountMax=3".to_owned(),
        ];

        if self.secret.is_none() {
            arguments.push("-o".to_owned());
            arguments.push("BatchMode=yes".to_owned());
        }

        arguments.push("-L".to_owned());
        arguments.push(format!(
            "{LOOPBACK}:{local_port}:{database_host}:{database_port}"
        ));

        if self.server.port != DEFAULT_PORT {
            arguments.push("-p".to_owned());
            arguments.push(self.server.port.to_string());
        }

        if let Some(identity) = &self.server.identity {
            arguments.push("-i".to_owned());
            arguments.push(identity.display().to_string());
        }

        arguments.push(self.server.destination());
        arguments
    }
}

/// How a database is reached.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Reach {
    /// Straight at the address the record names, which is every database before `R19e`.
    #[default]
    Direct,
    /// Through an SSH server, because the database's own port is not open to the outside.
    ///
    /// **Boxed because it is the rare one.** Most databases are reached directly, and a
    /// `Reach` sits inside every registered database — so the variant nobody uses should not
    /// be what decides the size of the one everybody does.
    Over(Box<Through>),
}

impl Reach {
    /// The server in between, when there is one.
    ///
    /// **The only way to ask.** There was a `is_over_ssh` beside this and it was removed
    /// rather than kept: two ways to ask one question is how half the call sites end up
    /// checking the flag and the other half unwrapping the value.
    #[must_use]
    pub fn through(&self) -> Option<&Through> {
        match self {
            Self::Direct => None,
            Self::Over(through) => Some(through),
        }
    }
}

/// Where the system's `ssh` is, if it has one.
///
/// **Windows has two and the difference matters.** `C:\Windows\System32\OpenSSH\ssh.exe` is
/// the one Windows ships; a Git installation puts an MSYS build on `PATH` that does not read
/// a `C:\…` path as absolute. Whichever `PATH` resolves is the one the user's own
/// `ssh user@server` runs, and that is deliberately the one used here: sloop's connection
/// should behave exactly as theirs does, including reading the same `~/.ssh/config`.
#[must_use]
pub fn program() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    crate::tools::acquire::on_path_at(name)
}
