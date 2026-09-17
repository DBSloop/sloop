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
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::Database;
use crate::tools::{Inventory, acquire};

/// Refuse to connect to a database whose tunnel this build cannot open yet.
///
/// **A guard with a shelf life, and it is here rather than nowhere for one reason.**
/// `R19e`'s registry half landed before its tunnel half, so a record can now say *"reach me
/// through bastion.internal"* while nothing in the binary opens a forward. Its `host` is the
/// address the **server** sees — almost always `127.0.0.1:5432` — so a command that simply
/// connected would aim at *this* machine's loopback: a connection error if nothing is
/// listening, and something far worse if something is. `mirror --to prod` writing into a
/// local database that happens to share the name is not a failure anybody would catch.
///
/// Every call to this goes when the tunnel is wired, and each one is replaced by opening it.
pub fn reachable(database: &Database) -> Outcome<()> {
    let Some(through) = database.reach.through() else {
        return Ok(());
    };

    Err(Failure::new(
        Exit::Usage,
        format!(
            "this database is reached over {} and this build does not open the tunnel yet",
            through.server.describe()
        ),
    )
    .hint(format!(
        "`sloop db edit <name> --no-ssh` makes it a direct connection again — but {}:{} is \
         the address {} sees, so it is almost certainly not reachable from here",
        database.host, database.port, through.server.host
    )))
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
