//! Installing a database server, for somebody who wants one and does not want to go and
//! find out how.
//!
//! **`R19d`, and the owner's reason for it is the test of whether it is any good:** *"this
//! will help user installing a db in just few steps instead of finding it's website and
//! installing instructions"*. So the shape is engine, then version, then one confirmation —
//! and everything after that is sloop's problem, including the parts a person doing it by
//! hand gets wrong: which archive is the right one, whether it arrived intact, where to put
//! it, which port is free, how the cluster is initialised, and how it is closed behind a
//! password before anything else on the machine can reach it.
//!
//! **This is not the PostgreSQL in [`crate::server`], and the difference matters.** That one
//! is sloop's own state, on 5433, written down in `server.toml`, and `reset` destroys it.
//! These hold *the user's* databases. They are written down in `servers.toml`, `reset` never
//! looks at them — it names what it deletes one by one, which is what makes that true by
//! construction rather than by intention — and the rule is `R19c6`'s rule about backups one
//! step along: a thing the user put data into is not sloop's to throw away.
//!
//! **It inherits the network rule with nothing softened.** The download is the system's own
//! `curl`, exactly as `R6`'s is; the archive is proved before a byte of it is unpacked, by
//! whichever of [`crate::tools::proof`]'s three kinds of proof that publisher actually gives;
//! and `cargo tree` shows no HTTP client, because there is still no HTTP client.

pub mod mysql;
pub mod port;
pub mod record;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::path::{Path, PathBuf};

use crate::engine::Engine;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;
use crate::server::{LOOPBACK, Origin, Server};
use crate::style;
use crate::tools::catalogue::Build;
use crate::tools::{acquire, proof};

/// Where every server sloop installed lives.
///
/// One directory under the global store, so that what sloop has put on this machine is one
/// place somebody can look at, and so the record beside it is never the only way to find
/// them again.
#[must_use]
pub fn home(global: &Path) -> PathBuf {
    global.join("servers")
}

/// Where one of them lives: `<global>/servers/postgres-18.6`.
///
/// **Keyed by version as well as engine**, because installing 8.4 beside 9.5 is a thing
/// somebody testing an upgrade will want, and a layout that made the second one overwrite
/// the first would take a working server away without saying so.
#[must_use]
pub fn home_for(global: &Path, engine: Engine, version: &str) -> PathBuf {
    home(global).join(format!("{}-{version}", engine.scheme()))
}

/// Where an archive is downloaded to before it has been proved.
///
/// Beside the installs rather than inside one: nothing that has failed its check is ever in
/// a directory a later run would treat as an installed server.
#[must_use]
fn workspace(global: &Path) -> PathBuf {
    home(global).join("download")
}

/// A server sloop installed, as it is written down and as it is reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Which engine.
    pub engine: Engine,
    /// The version, as the project writes it.
    pub version: String,
    /// The directory the whole thing is under.
    pub home: PathBuf,
    /// Where its programs are.
    pub bin: PathBuf,
    /// Where its data is.
    pub data: PathBuf,
    /// The port it listens on, which is the engine's usual one unless that was taken.
    pub port: u16,
    /// The administrative account — `postgres`, or `root`.
    pub superuser: String,
}

impl Installed {
    /// How it reads in a report and on a screen.
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{} {}", self.engine.proper_name(), self.version)
    }

    /// How a connection to it is spelled, with no password anywhere in it.
    #[must_use]
    pub fn url(&self) -> String {
        format!(
            "{}://{}@{LOOPBACK}:{}/",
            self.engine.scheme(),
            self.superuser,
            self.port
        )
    }

    /// As a [`Server`], for the PostgreSQL code that already knows how to drive one.
    ///
    /// [`Origin::Sloops`] because sloop made this cluster and may start and stop it. It is
    /// not the cluster `server.toml` names — see this module's header for why those two are
    /// kept apart.
    #[must_use]
    pub fn as_server(&self) -> Server {
        Server {
            bin: self.bin.clone(),
            data: Some(self.data.clone()),
            port: self.port,
            superuser: self.superuser.clone(),
            origin: Origin::Sloops,
        }
    }
}

/// The account name an installed server's superuser password is filed under.
///
/// Derived from the connection rather than invented, the way every other key in this tool
/// is: two servers of the same engine on different ports keep different passwords, and
/// neither can read the other's by accident.
#[must_use]
fn credential_key(installed: &Installed) -> String {
    format!(
        "sloop-installed:{}:{}@{LOOPBACK}:{}",
        installed.engine.scheme(),
        installed.superuser,
        installed.port
    )
}

/// Download the build somebody chose, prove it, unpack it, bring it up, and write it down.
///
/// **The order is the whole of the safety.** Nothing is unpacked before [`proof::holds`]
/// returns `Ok`, nothing is started before it has been unpacked, and nothing is written down
/// before it has answered a connection — so a run that fails anywhere leaves no record of a
/// server that is not there.
pub fn run(global: &Path, build: &Build) -> Outcome<Installed> {
    let into = home_for(global, build.engine, &build.version);
    already_there(global, build, &into)?;

    // **Asked before the download, not after.** A machine with no `gpg` cannot check a MySQL
    // archive, and finding that out is worth four hundred megabytes of somebody's line only
    // if it is found out afterwards.
    if !build.proof.checkable_here() {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "{} is proved by {}, and this machine has no gpg to check it with",
                build.describe(),
                build.proof.describe()
            ),
        )
        .hint(
            "install gpg — `winget install GnuPG.GnuPG`, `apt install gnupg`, `brew install \
             gnupg` — or install that server yourself",
        ));
    }

    let port = port::choose(build.engine.default_port(), &record::ports(global))?;

    let archive = fetch(global, build)?;
    unpack(&archive, &into)?;
    // A third of a gigabyte is not worth keeping for what came out of it, and neither is the
    // detached signature beside it.
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_file(archive.with_extension("asc"));

    let installed = Installed {
        engine: build.engine,
        version: build.version.clone(),
        bin: into.join("bin"),
        data: into.join("data"),
        home: into,
        port,
        superuser: superuser_of(build.engine).to_owned(),
    };

    let password = raise(&installed)?;
    record::remember(global, &installed, &password)?;

    // The archive is gone, so the directory it came down into has nothing left to hold.
    // `remove_dir` and not `remove_dir_all`: it goes only if it is empty, so a half-finished
    // download from another run is never swept up by a run that happened to succeed.
    let _ = std::fs::remove_dir(workspace(global));

    Ok(installed)
}

/// The administrative account each family creates its server with.
///
/// The conventional name in both cases, so that anything a person does with this server by
/// hand afterwards behaves the way every tutorial says it will.
#[must_use]
const fn superuser_of(engine: Engine) -> &'static str {
    match engine {
        Engine::Postgres => crate::server::SUPERUSER,
        Engine::Mysql | Engine::Mariadb => "root",
    }
}

/// Bring the server up and close it behind a generated password.
///
/// PostgreSQL's sequence is [`crate::server::make::raise`], unchanged and not copied. The
/// MySQL family's is nothing like it — a different bootstrap program, a config file rather
/// than command-line options, and a root account that starts with no password at all — which
/// is why it is its own module rather than a branch inside this one.
fn raise(installed: &Installed) -> Outcome<Secret> {
    match installed.engine {
        Engine::Postgres => {
            crate::server::make::raise(installed.as_server()).map(|ready| ready.password)
        }
        Engine::Mysql | Engine::Mariadb => {
            let password = Secret::new(crate::secret::generated_password()?);
            mysql::raise(installed, &password)?;
            Ok(password)
        }
    }
}

/// Is this one already here? The cheap half of [`already_there`], for a caller that wants to
/// refuse before it has printed a screen about an install that is not going to happen.
pub fn already_installed(global: &Path, build: &Build) -> Outcome<()> {
    already_there(
        global,
        build,
        &home_for(global, build.engine, &build.version),
    )
}

/// Refuse rather than install over something.
///
/// **A directory that is already a server is never written into.** It may be running, it
/// almost certainly has data in it, and rule 5's spirit says a thing like that is not
/// destroyed on the way to doing something else. The record is checked as well as the disk,
/// because either one on its own can be the half that survived a failure.
fn already_there(global: &Path, build: &Build, into: &Path) -> Outcome<()> {
    let kept = record::read(global)?;
    let recorded = kept
        .iter()
        .find(|held| held.engine == build.engine && held.version == build.version);

    if let Some(recorded) = recorded {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "sloop already installed {} on this machine, at {}",
                recorded.describe(),
                recorded.home.display()
            ),
        )
        .hint(format!("it answers on {}", recorded.url())));
    }

    if into.exists() {
        // **A directory with no record beside it is a previous attempt that did not finish**,
        // and saying so is the difference between a refusal somebody can act on and one they
        // have to guess at. Every install that succeeds writes a record, so there is no other
        // way to reach this state — and sloop still will not delete it, because whatever is
        // in there may be a data directory, and the log beside it is what says why the last
        // attempt stopped.
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "{} is already there, and sloop has no record of finishing it",
                into.display()
            ),
        )
        .hint(format!(
            "an earlier attempt got as far as unpacking and then stopped — {} says why. \
             Delete that directory to start again; sloop will not, because it may hold a \
             database",
            into.join("error.log").display()
        )));
    }

    Ok(())
}

/// Download the archive and prove it, leaving it in the workspace beside the installs.
fn fetch(global: &Path, build: &Build) -> Outcome<PathBuf> {
    let workspace = workspace(global);
    std::fs::create_dir_all(&workspace).map_err(|error| {
        Failure::usage(format!("could not create {}: {error}", workspace.display()))
    })?;

    let archive = workspace.join(&build.file_name);
    crate::say!("  {} {}", style::label("Downloading"), build.url);
    acquire::download(&build.url, &archive)?;

    crate::say!(
        "  {} against {}",
        style::label("Checking"),
        build.proof.describe()
    );
    proof::holds(&archive, &build.proof, &build.describe()).inspect_err(|_| {
        // Nothing that failed its check is left lying about for a later run to pick up.
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_file(archive.with_extension("asc"));
    })?;
    crate::say!(
        "  {} it is the archive that was asked for",
        style::label("Proved")
    );

    Ok(archive)
}

/// Unpack the whole archive, dropping the one directory every one of these projects wraps
/// itself in.
fn unpack(archive: &Path, into: &Path) -> Outcome<()> {
    std::fs::create_dir_all(into)
        .map_err(|error| Failure::usage(format!("could not create {}: {error}", into.display())))?;

    acquire::unpack_whole(archive, into).inspect_err(|_| {
        // Half an archive is not a server, and leaving one behind would make the next run
        // refuse with "that is already there" about something that never worked.
        let _ = std::fs::remove_dir_all(into);
    })?;

    crate::say!("  {} {}", style::label("Unpacked into"), into.display());
    Ok(())
}

/// Is this server answering right now?
///
/// **Two questions, and the cheap one first.** A port nothing is listening on is a stopped
/// server, and asking that costs a bind rather than a process. A port something *is*
/// listening on could be this server or could be something else that took it while this one
/// was stopped — so that case, and only that case, actually connects and asks.
#[must_use]
pub fn is_answering(global: &Path, installed: &Installed) -> bool {
    if port::is_free(installed.port) {
        return false;
    }

    let Ok(password) = record::password_for(global, installed) else {
        return false;
    };

    match installed.engine {
        Engine::Postgres => crate::server::make::server_version(&installed.as_server(), &password)
            .is_ok_and(|version| version.is_some()),
        Engine::Mysql | Engine::Mariadb => mysql::answers(installed, &password),
    }
}
