//! The commands themselves.

pub mod backup;
pub mod backups;
pub mod db;
pub mod doctor;
pub mod init;
pub mod key;
pub mod menu;
pub mod mirror;
pub mod reset;
pub mod restore;
pub mod server;
pub mod sync;
pub mod tables;

use std::path::Path;

use crate::engine::{Adapter, Engine};
use crate::failure::Outcome;
use crate::registry::file::Database;
use crate::registry::{Registries, Scope};
use crate::secret::Lookup;
use crate::ssh::tunnel::Tunnels;
use crate::tools::{Inventory, acquire};

/// The address a client program should dial for this database, right now.
///
/// **The whole of `R19e` at the call site is one line**: a command asks where the database
/// is instead of reading `host` and `port` off the record, and everything below — is there a
/// tunnel, is one already open, what unlocks the key — happens in here. Every adapter still
/// receives a host and a port and still knows nothing about SSH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct At {
    /// What to connect to. `127.0.0.1` when a forward is in the way.
    pub host: String,
    /// Which port. The forward's, when there is one, which is different every run.
    pub port: u16,
    /// The server in between, for the line a command prints. `None` for a direct
    /// connection, which is most of them.
    pub through: Option<String>,
}

impl At {
    /// Straight at an address, with nothing in front of it.
    #[must_use]
    pub fn straight(host: &str, port: u16) -> Self {
        Self {
            host: host.to_owned(),
            port,
            through: None,
        }
    }

    /// Straight at the address the record names.
    fn directly(database: &Database) -> Self {
        Self::straight(&database.host, database.port)
    }
}

/// Where to reach `database`, opening the SSH forward if it needs one and there is not one.
///
/// **A direct database costs nothing here**: no `ssh`, no keyring, no branch a user can
/// notice. A tunnelled one opens a forward the first time and reuses it for the rest of the
/// session, which is the owner's *"ten commands, one login"* in one function.
///
/// The passphrase, when the record names a route for one, is resolved the same way a
/// database password is — it is filed under [`crate::ssh::Server::credential_key`], so five
/// databases behind one bastion read it once from one place.
pub fn reach(
    database: &Database,
    tunnels: &Tunnels,
    registries: &Registries,
    scope: Scope,
) -> Outcome<At> {
    let Some(through) = database.reach.through() else {
        return Ok(At::directly(database));
    };

    // **Asked for only if a forward actually has to be opened.** Five databases behind one
    // bastion would otherwise read the keyring — or run `op read` — five times over, four of
    // them for a connection that was already up.
    let unlock = || match &through.secret {
        None => Ok(None),
        Some(route) => {
            let key = through.server.credential_key();
            let vault = registries
                .vault_in(scope)
                .unwrap_or_else(crate::secret::sealed::Vault::nowhere);
            let resolved = crate::secret::resolve(
                route,
                &Lookup {
                    key: &key,
                    vault: &vault,
                },
            )
            .map_err(|failure| {
                failure.hint(format!(
                    "that is the passphrase for the SSH key to {}. `sloop db edit <name> \
                     --ssh-env VARIABLE` changes where it is read from",
                    through.server.describe()
                ))
            })?;
            for note in &resolved.notes {
                crate::note!("{}", crate::style::dim(note));
            }
            Ok(Some(resolved.secret))
        }
    };

    let port = tunnels.port_for(through, &database.host, database.port, unlock)?;

    Ok(At {
        host: crate::ssh::LOOPBACK.to_owned(),
        port,
        through: Some(through.server.describe()),
    })
}

/// The adapter for an engine, pointed at the client tools this machine actually has.
///
/// **Every command that runs a client tool goes through here**, because the alternative is
/// a machine where `sloop doctor` fetches PostgreSQL's tools, says so, and then every other
/// command reports that `pg_dump` is not installed. `engine::adapter_for` runs whatever
/// bare `pg_dump` the `PATH` resolves — which on Windows, where the fetched copy is the
/// only copy, is nothing at all.
///
/// A command with several databases to get through builds one [`Inventory`] and asks it
/// directly rather than calling this per database: looking for seven programs means
/// running seven of them to ask their versions, and doing that once per database in
/// `backup --all` would be the slowest part of the run.
#[must_use]
pub fn adapter_for(engine: Engine, global: &Path) -> Box<dyn Adapter> {
    Inventory::for_engine(engine, &acquire::fetched_dir(global)).adapter_for(engine)
}
