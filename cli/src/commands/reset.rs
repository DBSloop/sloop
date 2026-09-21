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
//! **And the passwords go with the state they belong to.** A reset that left sloop's own
//! credentials in the OS keyring was not a reset: the next `sloop setup` builds a new cluster
//! with new passwords, and a stale entry under the same key is a machine that authenticates
//! against a server that no longer exists. That is not hypothetical — it is what the owner
//! hit: *"password authentication failed for user"* the role sloop owns, on a machine that had been
//! reset. See "A reset leaves nothing behind" in `docs/OWNER-DECISIONS.md`.
//!
//! **Except the backup key, and that is the one exception in the whole command.** The private
//! half of the `age` keypair lives in the keyring under `backup-key:<public>`, and it is the
//! only thing standing between a kept backup and an unreadable one. Deleting it while keeping
//! the backups would be deleting the backups the slow way.
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
use crate::secret::Route;
use crate::server::{self, Origin};
use crate::service::mechanism::Mechanism;
use crate::service::{elevation, manage};
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
    /// Every secret this machine keeps for sloop, by the name it is filed under.
    ///
    /// **Never the backup key.** See the header: it is the one thing here whose absence
    /// cannot be undone by running Setup again.
    pub passwords: Vec<Kept>,
    /// True when the registry could not be read, so the list above may be short.
    pub some_unknown: bool,
    /// The background service, when this machine has one registered.
    ///
    /// **Part of "everything sloop put here", and the only part that needs more than this
    /// account has.** A reset that left it registered would leave the machine running sloop
    /// at boot against a store that no longer exists.
    pub service: Option<Mechanism>,
    /// Every database server `sloop server install` put on this machine.
    ///
    /// Kept whole rather than as paths, because each one has to be stopped before its
    /// directory can be removed — and stopping one needs its port, its binaries and its
    /// superuser.
    pub installed: Vec<crate::install::Installed>,
    /// Every `backups/` that stays, so the sentence about them names them.
    pub kept: Vec<PathBuf>,
}

/// One secret sloop keeps, and where it keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// What it is filed under.
    pub key: String,
    /// Which store it is in.
    pub route: Route,
    /// What it opens, for the line that names it.
    pub what: String,
}

impl What {
    /// Has this machine anything to reset?
    #[must_use]
    pub fn anything(&self) -> bool {
        self.server.is_some()
            || !self.going.is_empty()
            || !self.passwords.is_empty()
            || self.service.is_some()
            || !self.installed.is_empty()
    }
}

/// Work out what a reset would do.
pub fn survey(global: &Path) -> Outcome<What> {
    let service =
        Mechanism::of_this_machine().filter(|mechanism| manage::state(*mechanism).installed());
    surveying(global, service)
}

/// The same, told what the machine runs rather than asking it.
///
/// **Split out so the answer can be written down.** What a reset does to a *store* is decided
/// by files under it; whether there is a service to remove is decided by the machine, and a
/// test that consulted the real service control manager would pass or fail depending on
/// whether the developer happened to have sloop installed on the machine running it.
fn surveying(global: &Path, service: Option<Mechanism>) -> Outcome<What> {
    let record = server::record::read_server(global)?;
    let owns_the_server = record
        .as_ref()
        .is_some_and(|server| server.origin == Origin::Sloops);

    // Everything in the global store that is sloop's own state. Named one by one rather than
    // "the whole directory", because `backups/` lives in there too and must not be swept up
    // by a glob somebody adds to later.
    let mut going = Vec::new();

    // **The service's key file, even when no service is registered.** `R34`: a `service
    // install` that failed half way leaves one behind, and nothing else on this path would
    // ever look at it.
    let key_file = crate::service::key::path();
    if key_file.exists() {
        going.push(key_file);
    }

    for name in [
        server::record::FILE,
        crate::registry::file::SEALED_FILE,
        crate::registry::file::SERVICE_SEALED_FILE,
        crate::registry::file::FILE,
        crate::install::record::FILE,
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

    // **And every database server `sloop server install` put here.** These are not sloop's
    // own state cluster: they are MySQL, MariaDB or another PostgreSQL that sloop downloaded
    // and installed because somebody asked it to. A reset that left them behind would leave
    // running servers with their data and a `servers.toml` describing them, on a machine
    // whose whole point was that sloop had been taken off it.
    let installed = crate::install::record::read(global).unwrap_or_default();
    let servers = crate::install::home(global);
    if servers.is_dir() {
        going.push(servers);
    }

    let backups = global.join("backups");
    let kept = if backups.is_dir() {
        vec![backups]
    } else {
        Vec::new()
    };

    let (passwords, some_unknown) = secrets(global, record.as_ref(), &installed);

    Ok(What {
        global: global.to_path_buf(),
        server: record,
        owns_the_server,
        going,
        kept,
        passwords,
        some_unknown,
        service,
        installed,
    })
}

/// Every secret this machine keeps for sloop, and whether that list is complete.
///
/// **Two of them come off the disk and the rest come out of the registry**, which is the
/// difference that matters when something has already gone wrong. `server.toml` names the
/// server, and the superuser's key and `sloop_db_admin`'s are derived from it — so the two
/// that strand a machine are found even when nothing else can be. The registered databases'
/// own passwords are in the registry, which lives in the database this is about to destroy;
/// if it will not open, they are reported as unknown rather than silently skipped.
fn secrets(
    global: &Path,
    record: Option<&server::Server>,
    installed: &[crate::install::Installed],
) -> (Vec<Kept>, bool) {
    let mut found: Vec<Kept> = Vec::new();

    if let Some(server) = record {
        found.push(Kept {
            key: server::record::credential_key_of(server),
            route: Route::Keyring,
            what: format!("{}, the superuser of sloop's PostgreSQL", server.superuser),
        });

        if let Ok(Some(own)) = server::record::recorded_own(global) {
            let route = server::record::database_route(global)
                .ok()
                .flatten()
                .unwrap_or(Route::Keyring);
            found.push(Kept {
                key: own.credential_key(server),
                route,
                what: format!("{}, which owns sloop's own database", own.role),
            });
        }
    }

    // **Every server `sloop server install` put here, and its superuser.** These are not in
    // the registry — they are in `servers.toml`, which is on disk — so they are found whether
    // or not the state database opens.
    for one in installed {
        found.push(Kept {
            key: crate::install::credential_key(one),
            route: Route::Keyring,
            what: format!(
                "{}, the superuser of the {} sloop installed",
                one.superuser, one.engine
            ),
        });
    }

    // Everything the registry recorded. Best effort on purpose: a machine whose state
    // database will not open is exactly the machine somebody is resetting.
    let mut unknown = false;
    match registered(global) {
        Ok(more) => found.extend(more),
        Err(()) => unknown = true,
    }

    // A key filed twice is a key deleted twice, which is noise on the screen and a second
    // `NoEntry` nobody needs to read.
    found.dedup_by(|one, other| one.key == other.key);
    (found, unknown)
}

/// The passwords the registry recorded for the databases sloop knows about.
///
/// `Err(())` when the registry could not be read at all, which the caller reports rather
/// than swallows.
fn registered(global: &Path) -> Result<Vec<Kept>, ()> {
    // **The global registry and nothing else.** A project registry lives beside somebody's
    // code and is not sloop's to reach into from here; `Resolution::global_only` is the same
    // answer the background service uses, and for the same reason — there is no working
    // directory that means anything at this point.
    let registries =
        crate::registry::Registries::open(crate::registry::Resolution::global_only(), global)
            .map_err(|_| ())?;

    // **Both secrets a database can have.** Its own password, and — where it is reached over
    // SSH — the passphrase of the key that opens the tunnel. Two entries, two keys, and a
    // reset that forgot only the first would leave the second behind.
    let mut found = Vec::new();
    for (_, name, database) in registries.all() {
        found.push(Kept {
            key: database.credential_key(),
            route: database.password.clone(),
            what: format!("the password for {name}"),
        });

        if let Some(through) = database.reach.through()
            && let Some(route) = through.secret.clone()
        {
            found.push(Kept {
                key: through.server.credential_key(),
                route,
                what: format!("the SSH key passphrase for {name}"),
            });
        }
    }
    Ok(found)
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

    if let Some(mechanism) = what.service {
        crate::say!(
            "  {} {}",
            style::label("Service"),
            style::paint(&format!(
                "the background service, registered with {}",
                mechanism.spoken()
            ))
        );
    }

    for one in &what.installed {
        crate::say!(
            "  {} {}",
            style::label("Server"),
            style::paint(&format!(
                "{} on port {}, which sloop installed",
                one.describe(),
                one.port
            ))
        );
    }

    for path in &what.going {
        crate::say!("  {} {}", style::label("Gone"), path.display());
    }

    for kept in &what.passwords {
        crate::say!(
            "  {} {}",
            style::label("Password"),
            style::paint(&kept.what)
        );
    }
    if !what.passwords.is_empty() {
        crate::say!(
            "  {}",
            style::dim(
                "these are forgotten wherever this machine keeps them, so the next \
                        `sloop setup` starts with nothing left over"
            )
        );
    }
    if what.some_unknown {
        crate::say!(
            "  {}",
            style::dim(
                "the registry could not be read, so a password it recorded for one of your \
                        own databases may be left behind — `sloop db remove <name>` forgets one"
            )
        );
    }

    announce_what_stays(what);
}

/// The other half of the list: what a reset deliberately does not touch.
///
/// Its own function because [`announce`] is a list of sections and this is the section that
/// is about a promise rather than about a path.
fn announce_what_stays(what: &What) {
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
            "the backup key is kept too — it is the only thing here that makes a kept \
                    backup readable, so forgetting it would be deleting the backups slowly."
        )
    );
    crate::say!(
        "  {}",
        style::dim(
            "a machine that keeps its secrets in the encrypted file is the exception: \
                    that file lives in the registry and goes with it, so export the key \
                    first if those backups matter."
        )
    );
}

/// Put this machine back to a fresh install.
pub fn run(locations: &Locations, consent: Consent<'_>, also_the_binary: bool) -> Outcome<Exit> {
    let global = crate::registry::adopt::global(locations)?;
    let what = survey(&global)?;

    // **Before the list, before the question, before anything.** The owner's instruction:
    // *"even for reset and uninstall if it needs elevated administrator rights, then ask it
    // first without proceeding"*. Removing a service is the one thing here that needs more
    // than this account has, so a run that cannot do it says so while the machine is still
    // whole — rather than deleting the cluster and then discovering it cannot finish. Same
    // rule as `R24a`, one command earlier.
    if let Some(mechanism) = what.service {
        elevation::require(mechanism)?;
    }

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

    carry_out(locations, &what)?;

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
fn carry_out(locations: &Locations, what: &What) -> Outcome<()> {
    // **The service before the database it reads.** It wakes on a timer and opens the store
    // every round; leaving it running while the cluster underneath it is deleted is asking
    // for a round that lands halfway through.
    if let Some(mechanism) = what.service {
        super::service::uninstall(locations, mechanism);
    }

    if let Some(server) = &what.server {
        if what.owns_the_server {
            stop_the_cluster(server);
        } else {
            drop_sloops_own(server, &what.global)?;
        }
    }

    // **Every server `sloop server install` put here, stopped before its directory goes.**
    // On Windows a running postmaster or mysqld holds its own data files open, so a delete
    // that did not stop it first is a delete that fails halfway and leaves a tree behind.
    for one in &what.installed {
        stop_installed(&what.global, one);
    }

    for path in &what.going {
        let outcome = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        // **Already gone is done, not failed.** Two steps on this path can own the same
        // file: `service uninstall` removes the key file, and so does the sweep below it for
        // the case where no service was ever registered. Treating the second one as an error
        // aborted the whole reset and left the cluster, the record and both sealed files
        // exactly where they were — a half-removed machine, from a command that had already
        // said what it was going to do.
        match outcome {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            outcome => outcome.map_err(|error| {
                Failure::usage(format!("could not remove {}: {error}", path.display()))
                    .hint("remove it by hand — everything else has already gone")
            })?,
        }
        crate::say!("  {} {}", style::label("Removed"), path.display());
    }

    prune_empty_directories(what);
    forget_the_passwords(what);

    Ok(())
}

/// Take the directories away too, once nothing sloop put in them is left.
///
/// **`R34`: "it should clean totally" means the folders as well.** Every path above is a file
/// or a directory sloop made, and removing them one by one leaves `/etc/sloop` holding
/// nothing and the store holding nothing -- two empty directories that read, to anybody
/// looking, as sloop still being on the machine.
///
/// **Only when empty, and that is the whole safety of it.** `backups/` lives in the store and
/// is never deleted, so a store that still has one is a store that stays. `remove_dir`
/// refuses a directory with anything in it, so the check and the removal are the same call
/// and nothing can slip between them.
fn prune_empty_directories(what: &What) {
    for directory in [
        crate::service::key::path().parent().map(Path::to_path_buf),
        Some(what.global.clone()),
    ]
    .into_iter()
    .flatten()
    {
        if std::fs::remove_dir(&directory).is_ok() {
            crate::say!("  {} {}", style::label("Removed"), directory.display());
        }
    }
}

/// Forget every secret the survey named.
///
/// **Best effort, one at a time, and never fatal.** Everything above this line has already
/// happened — the cluster is gone and the files are gone — so a keyring that will not answer
/// is a line to read, not a reason to fail a reset that has already succeeded. An entry that
/// was never there is not an error either: `os_keyring::delete` says so itself.
///
/// The encrypted-file route is not touched here because it has already gone: the sealed file
/// is one of the paths in `going`, so the secrets inside it went with it.
fn forget_the_passwords(what: &What) {
    let mut forgotten = 0;
    let mut never_ours = 0;

    for kept in &what.passwords {
        match &kept.route {
            Route::Keyring => match crate::secret::os_keyring::delete(&kept.key) {
                Ok(()) => forgotten += 1,
                Err(failure) => crate::say!(
                    "  {} {}",
                    style::label("Left"),
                    style::dim(&format!("{}: {}", kept.what, failure.message()))
                ),
            },
            // The encrypted file is one of the paths already removed above, so everything
            // inside it went with it. Counted, because it did go.
            Route::EncryptedFile => forgotten += 1,
            // **Nothing to delete, and saying so is the point.** `${VAR}` and a password
            // command point at a secret somebody else keeps — sloop never held a copy, so
            // there is no copy of it here to remove and none of sloop's business to try.
            Route::Environment(_) | Route::Command(_) => never_ours += 1,
        }
    }

    if forgotten > 0 {
        crate::say!(
            "  {} {}",
            style::label("Forgotten"),
            plural(forgotten, "password")
        );
    }
    if never_ours > 0 {
        crate::say!(
            "  {} {}",
            style::label("Left"),
            style::dim(&format!(
                "{} sloop never held — they come from an environment variable or a command, \
                 and the secret itself was always somebody else's",
                plural(never_ours, "password")
            ))
        );
    }
}

/// `1 password`, `2 passwords`.
fn plural(count: usize, what: &str) -> String {
    if count == 1 {
        format!("{count} {what}")
    } else {
        format!("{count} {what}s")
    }
}

/// Hand `pg_ctl` the account a root run gave the cluster to.
///
/// **`R31`: root cannot stop a cluster either.** `pg_ctl` refuses uid 0 for exactly the same
/// reason `initdb` does, so a reset run by the account that set the machine up would leave
/// the postmaster running and then delete the directory underneath it. Best effort, like
/// everything else on this path: a machine with no service account has nothing to drop to,
/// and the stop is attempted as whoever is running.
fn as_the_owner(command: &mut Command) {
    if !crate::account::is_root() {
        return;
    }
    if let Ok(service) = crate::account::service() {
        crate::account::run_as(command, &service);
    }
}

/// Stop a server `sloop server install` put here, so its directory can actually be deleted.
///
/// **Best effort, and every branch of it.** One that is already stopped, one whose binaries
/// have gone with a half-removed install, and one whose password cannot be read back are all
/// the same thing here: the directory goes either way, and the line that follows says so if
/// it could not.
fn stop_installed(global: &Path, one: &crate::install::Installed) {
    let stopped = match one.engine {
        // PostgreSQL ships a supervisor that knows how to ask a cluster to close its files.
        crate::engine::Engine::Postgres => {
            let mut command = Command::new(one.bin.join("pg_ctl"));
            command
                .arg("-D")
                .arg(&one.data)
                .args(["stop", "-m", "fast", "-w"])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            as_the_owner(&mut command);
            command.status().is_ok_and(|status| status.success())
        }

        // The MySQL family has no supervisor: the server is asked to shut down over its own
        // protocol, as the superuser, which means the password. `MYSQL_PWD` on this child and
        // nothing wider — rule 3, and the same route `install::mysql` uses.
        crate::engine::Engine::Mysql | crate::engine::Engine::Mariadb => {
            let admin = if one.engine == crate::engine::Engine::Mariadb {
                "mariadb-admin"
            } else {
                "mysqladmin"
            };
            let Ok(password) = crate::install::record::password_for(global, one) else {
                return said_still_running(one);
            };
            Command::new(one.bin.join(admin))
                .args(["--protocol=tcp", "--host=127.0.0.1"])
                .arg(format!("--port={}", one.port))
                .arg(format!("--user={}", one.superuser))
                .arg("shutdown")
                .env("MYSQL_PWD", password.expose())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }
    };

    if stopped {
        crate::say!("  {} {}", style::label("Stopped"), one.describe());
    } else {
        said_still_running(one);
    }
}

/// Say a server would not stop, without making it fatal.
///
/// The directory is removed next either way, and on Windows that is what will fail if the
/// server really is still up — with a line naming the path, which is more use than this one.
fn said_still_running(one: &crate::install::Installed) {
    crate::say!(
        "  {} {}",
        style::label("Left"),
        style::dim(&format!(
            "{} would not stop — if its directory will not delete, stop it and try again",
            one.describe()
        ))
    );
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

    let mut command = Command::new(server.program("pg_ctl"));
    command
        .arg("-D")
        .arg(data)
        .args(["-m", "immediate", "-w", "stop"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    as_the_owner(&mut command);

    let stopped = command.status();

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
