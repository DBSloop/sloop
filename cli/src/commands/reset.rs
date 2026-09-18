//! `sloop reset` — put the machine back to the moment sloop was installed.
//!
//! **What it destroys depends on who owns the PostgreSQL**, and `server.toml` already records
//! that. `R19c1` wrote an `origin` field to know whether it may start a cluster; this is the
//! second thing that needed it, and it is the whole hinge of the command:
//!
//! ```text
//! origin = sloop     sloop downloaded and installed PostgreSQL 18, so sloop owns it:
//!                    stop the cluster and delete it, binaries and all. The next
//!                    `sloop setup` downloads and installs again.
//! origin = machine   the PostgreSQL was already there and sloop is a guest on it:
//!                    drop sloop_database and its role, and touch nothing else. Not the
//!                    server, not another database, not another role.
//! ```
//!
//! **Backups are never deleted. Not here, not by `uninstall`, not ever.** The owner's line,
//! and the reason is that they are the one thing on a machine that cannot be regenerated: a
//! registry is a minute of typing and a PostgreSQL is a download, but a dump that is gone is
//! gone. Every `backups/` directory is left exactly where it is and named on the way out.
//!
//! **The one thing that can still be lost, and it is said before anything happens.** An
//! encrypted backup is readable only with the private key, and a machine that keeps its
//! secrets in the Argon2id vault keeps that key *inside* `sloop_database` — which is what
//! reset destroys. So reset says which keys are about to go and whether a copy was ever
//! exported, before the name is typed rather than after.
//!
//! **Rule 5 at its strongest.** This destroys a database server. It is typed, never a `y`,
//! and without a terminal it exits `2` naming the flag — rule 4, which a command this size is
//! the last place to bend.
//!
//! **`uninstall` is this plus the binary.** One implementation, called twice, so the two can
//! never drift into destroying different things.

#[cfg(test)]
#[path = "reset_tests.rs"]
mod tests;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::consent::{Consent, Destroying};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::locations::Locations;
use crate::server::{self, Origin};
use crate::style;

/// The name that has to be typed. The database that stops existing, which is the same thing
/// `db drop` asks for and for the same reason: a name somebody reads off the screen and
/// writes out is a name they have looked at.
const TYPE_THIS: &str = "sloop_database";

/// Everything reset is about to do, worked out before it does any of it.
///
/// **Surveyed first so it can be shown first.** The last words somebody sees before a
/// database server stops existing should be a list of what is going, not a yes/no.
#[derive(Debug)]
pub struct What {
    /// The global store.
    pub global: PathBuf,
    /// The server, if this machine has been set up at all.
    pub server: Option<server::Server>,
    /// True when sloop installed that PostgreSQL and may therefore remove it.
    pub owns_the_server: bool,
    /// Everything under the global store that goes.
    pub going: Vec<PathBuf>,
    /// Every `backups/` that stays, so the sentence about them names them.
    pub kept: Vec<PathBuf>,
}

impl What {
    /// Has this machine anything to reset?
    #[must_use]
    pub fn anything(&self) -> bool {
        self.server.is_some() || !self.going.is_empty()
    }
}

/// Work out what a reset would do.
pub fn survey(global: &Path) -> Outcome<What> {
    let record = server::record::read_server(global)?;
    let owns_the_server = record
        .as_ref()
        .is_some_and(|server| server.origin == Origin::Sloops);

    // Everything in the global store that is sloop's own state. Named one by one rather than
    // "the whole directory", because `backups/` lives in there too and must not be swept up
    // by a glob somebody adds to later.
    let mut going = Vec::new();
    for name in [
        server::record::FILE,
        crate::registry::file::SEALED_FILE,
        crate::registry::file::FILE,
    ] {
        let path = global.join(name);
        if path.exists() {
            going.push(path);
        }
    }
    for dir in ["projects", "locks"] {
        let path = global.join(dir);
        if path.is_dir() {
            going.push(path);
        }
    }

    // The PostgreSQL sloop installed, which is one directory — cluster and binaries both.
    let postgres = server::home(global);
    if owns_the_server && postgres.is_dir() {
        going.push(postgres);
    }

    let backups = global.join("backups");
    let kept = if backups.is_dir() {
        vec![backups]
    } else {
        Vec::new()
    };

    Ok(What {
        global: global.to_path_buf(),
        server: record,
        owns_the_server,
        going,
        kept,
    })
}

/// Say what is about to happen, before anything does.
fn announce(what: &What) {
    crate::say!("{}", style::heading("This will remove:"));

    match (&what.server, what.owns_the_server) {
        (Some(server), true) => {
            crate::say!(
                "  {} {}",
                style::label("PostgreSQL"),
                style::paint(&format!(
                    "the whole server sloop installed, on port {}",
                    server.port
                ))
            );
            crate::say!(
                "  {}",
                style::dim(
                    "sloop installed it, so sloop takes it away. `sloop setup` \
                            downloads and installs it again."
                )
            );
        }
        (Some(server), false) => {
            crate::say!(
                "  {} {}",
                style::label("Database"),
                style::paint(&format!(
                    "{TYPE_THIS} and its role, on the PostgreSQL already at port {}",
                    server.port
                ))
            );
            crate::say!(
                "  {}",
                style::dim(
                    "that server was here before sloop and stays exactly as it is — \
                            no other database on it is touched"
                )
            );
        }
        (None, _) => crate::say!("  {}", style::dim("no server — this machine is not set up")),
    }

    for path in &what.going {
        crate::say!("  {} {}", style::label("Gone"), path.display());
    }

    if what.kept.is_empty() {
        return;
    }

    crate::say!("");
    crate::say!("{}", style::heading("This will keep:"));
    for path in &what.kept {
        crate::say!(
            "  {} {}",
            style::label("Backups"),
            style::paint(&path.display().to_string())
        );
    }
    crate::say!(
        "  {}",
        style::dim(
            "a backup is the one thing here that cannot be made again, so reset never \
                    deletes one"
        )
    );
    crate::say!(
        "  {}",
        style::dim(
            "an encrypted backup still needs its key. If the private key was never \
                    exported and this machine keeps secrets in the encrypted file, it goes \
                    with the database and those backups become unreadable."
        )
    );
}

/// Put this machine back to a fresh install.
pub fn run(locations: &Locations, consent: Consent<'_>, also_the_binary: bool) -> Outcome<Exit> {
    let global = crate::registry::adopt::global(locations)?;
    let what = survey(&global)?;

    if !what.anything() && !also_the_binary {
        crate::say!(
            "{}",
            style::dim("there is nothing to reset — this machine has no sloop state on it")
        );
        return Ok(Exit::Success);
    }

    announce(&what);
    crate::say!("");

    let destroying = Destroying {
        named: TYPE_THIS,
        noun: "database sloop keeps its state in",
        action: if also_the_binary {
            "uninstalling sloop"
        } else {
            "resetting sloop"
        },
    };
    if !consent.typed(&destroying)?.granted() {
        crate::say!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    }

    carry_out(&what)?;

    if also_the_binary {
        remove_the_binary();
    }

    crate::say!("");
    crate::say!(
        "{} {}",
        style::heading("Done."),
        style::dim(if also_the_binary {
            "sloop is off this machine."
        } else {
            "run `sloop setup` to use sloop again."
        })
    );
    Ok(Exit::Success)
}

/// Destroy what the survey found, in the order that cannot strand anything.
///
/// **The database before the files.** The files are what say where the database is, so
/// removing them first would leave a `sloop_database` nothing could find and nothing could
/// clean up — which is the one way a reset could make more mess than it cleared.
fn carry_out(what: &What) -> Outcome<()> {
    if let Some(server) = &what.server {
        if what.owns_the_server {
            stop_the_cluster(server);
        } else {
            drop_sloops_own(server, &what.global)?;
        }
    }

    for path in &what.going {
        let outcome = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        outcome.map_err(|error| {
            Failure::usage(format!("could not remove {}: {error}", path.display()))
                .hint("remove it by hand — everything else has already gone")
        })?;
        crate::say!("  {} {}", style::label("Removed"), path.display());
    }

    Ok(())
}

/// Stop a cluster sloop made, so its directory can actually be deleted.
///
/// Best effort: a cluster that is already stopped, or whose `pg_ctl` has gone with a
/// half-removed PostgreSQL, is not a reason to refuse to finish. The directory goes either
/// way, and on Windows a running postmaster is what would stop it — which is why this is
/// tried first rather than skipped.
fn stop_the_cluster(server: &server::Server) {
    let Some(data) = &server.data else {
        return;
    };

    let stopped = Command::new(server.program("pg_ctl"))
        .arg("-D")
        .arg(data)
        .args(["-m", "immediate", "-w", "stop"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if stopped.is_ok_and(|status| status.success()) {
        crate::say!("  {} {}", style::label("Stopped"), data.display());
    }
}

/// Drop sloop's own database and role from a server that is not sloop's.
///
/// **Two statements and nothing else.** This is somebody else's PostgreSQL with somebody
/// else's databases on it; the only things here that are sloop's are the two it created.
fn drop_sloops_own(server: &server::Server, global: &Path) -> Outcome<()> {
    let Some(password) = superuser_password(global, server) else {
        return Err(Failure::new(
            Exit::Connect,
            format!(
                "{} cannot be opened, so {TYPE_THIS} cannot be dropped from it",
                server.url("postgres")
            ),
        )
        .hint(
            "nothing has been removed. Restore the password this machine keeps for that \
               server, or drop the database and its role by hand",
        ));
    };

    let own = server::own::Own::sloops();

    // `DROP DATABASE` cannot run inside a transaction, so these are two statements rather
    // than a script — and `IF EXISTS` because a half-finished earlier reset is a state to
    // finish rather than to fail on.
    for statement in [
        format!("DROP DATABASE IF EXISTS \"{}\";", own.database),
        format!("DROP ROLE IF EXISTS \"{}\";", own.role),
    ] {
        server::make::run_sql(server, Some(&password), &statement)?;
    }

    crate::say!(
        "  {} {} and {}",
        style::label("Dropped"),
        own.database,
        own.role
    );
    Ok(())
}

/// The superuser password this machine keeps for that server, or `None`.
fn superuser_password(global: &Path, server: &server::Server) -> Option<crate::secret::Secret> {
    server::record::superuser_password(global, server)
        .ok()
        .flatten()
}

/// Take sloop itself off the machine.
///
/// **The directory the installer made, and the `PATH` entry that points at it.** Both halves
/// live in [`crate::pathentry`], which is `R21`'s: the installer chose the directory and
/// wrote the line, so the installer's module is where what to undo is described. What is
/// here is the half the owner asked `uninstall` to share with `reset`, so the two cannot
/// drift into removing different things.
fn remove_the_binary() {
    if let Some(installed) = crate::pathentry::installed() {
        // **Never the whole directory on Unix.** `~/.local/bin` is somebody's own bin
        // directory with somebody's own programs in it; only the one file sloop put there
        // goes. On Windows `Programs\sloop` is wholly the installer's and goes whole.
        match &installed {
            crate::pathentry::Installed::Tree(path) if path.is_dir() => {
                said_or_told(std::fs::remove_dir_all(path), path);
            }
            crate::pathentry::Installed::File(path) if path.exists() => {
                said_or_told(std::fs::remove_file(path), path);
            }
            _ => {}
        }
    }

    let removal = crate::pathentry::remove();
    for touched in &removal.touched {
        crate::say!("  {} {}", style::label("PATH"), touched.said());
    }
    for trouble in &removal.trouble {
        crate::note!(
            "{}",
            style::dim(&format!("{trouble} — take it out by hand"))
        );
    }

    if !removal.touched.is_empty() {
        crate::say!(
            "  {}",
            style::dim("open shells still have the old PATH; the next one will not")
        );
    }
}

/// Say what went, or say what did not and carry on.
///
/// **A running binary cannot always delete itself**, which is exactly the case on Windows —
/// so this is reported rather than fatal: everything else has already gone, and telling
/// somebody one file is left is better than telling them the uninstall failed.
fn said_or_told(outcome: std::io::Result<()>, path: &Path) {
    match outcome {
        Ok(()) => crate::say!("  {} {}", style::label("Removed"), path.display()),
        Err(error) => crate::note!(
            "{}",
            style::dim(&format!(
                "{} is still there: {error} — delete it by hand",
                path.display()
            ))
        ),
    }
}
