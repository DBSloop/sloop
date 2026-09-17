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
pub mod sync;
pub mod tables;

use std::path::Path;

use crate::engine::{Adapter, Engine};
use crate::tools::{Inventory, acquire};

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
