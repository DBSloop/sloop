//! The commands themselves.

pub mod backup;
pub mod backups;
pub mod db;
pub mod doctor;
pub mod init;
pub mod key;

use std::path::Path;

use crate::engine::{Adapter, Engine};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
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

/// A yes-or-no question, for the things that do not destroy a database.
///
/// **Rule 4: with no terminal there is nobody to ask**, so it exits `2` naming the flag that
/// would have answered instead of waiting for somebody who is not there.
///
/// Rule 5 — destructive means typing the name — is [`db::drop`]'s and not this: what that
/// rule names is dropping or replacing a *database*, and making routine retention type a
/// label out would be hostility rather than safety. Deleting old backups asks a question and
/// shows the whole list first.
pub fn confirmed(already: bool, question: &str, flag: &str) -> Outcome<bool> {
    use std::io::IsTerminal as _;

    if already {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            format!("{question} — and there is no terminal to ask at"),
        )
        .hint(format!("pass {flag} to answer it up front")));
    }

    anstream::print!("{} {question} ", crate::style::paint("?"));
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;

    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
