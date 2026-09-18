//! `<global>/server.toml` — where sloop's PostgreSQL is, and never its password.
//!
//! **This file exists because it cannot live in the database.** Everything else sloop knows
//! moves into PostgreSQL in `R19c4`; the details that *open* PostgreSQL obviously cannot.
//! So there is one file, it holds a path, a port and a role name, and the password field
//! holds one of `R3`'s four routes — the word `keyring`, not a secret. Rule 3 by
//! construction: there is no spelling of a password this file would accept.
//!
//! The password itself goes wherever this machine keeps secrets, under a key naming the
//! cluster, exactly as a registered database's does.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::{Route, Secret};

use super::{Origin, Ready, Server, make};

/// What the file is called.
pub const FILE: &str = "server.toml";

/// Bumped only when the shape below changes in a way an older sloop could misread.
///
/// `2` added the `[database]` section. A `1` written by an older sloop still reads, because
/// the section is optional and gets filled in the first time `R19c2` runs.
const VERSION: u32 = 2;

/// The account name the superuser password is filed under.
///
/// Derived from the connection rather than invented, the way a registered database's key is:
/// two sloops on one machine with clusters on different ports keep different passwords, and
/// neither can read the other's by accident.
#[must_use]
fn credential_key(server: &Server) -> String {
    format!(
        "sloop-server:{}@{}:{}",
        server.superuser,
        super::LOOPBACK,
        server.port
    )
}

/// The file, exactly as TOML sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    version: u32,
    bin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    port: u16,
    superuser: String,
    /// A route, never a value — see [`Route::parse`], which refuses anything that looks like
    /// a password.
    password: String,
    /// `this machine's` or `sloop's`, so a later run knows whether the cluster is one it may
    /// start.
    origin: String,
    /// sloop's own database on that server, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    database: Option<RawDatabase>,
}

/// Where sloop's own state lives on that server, and how to open it.
///
/// **The connection details, and nothing else.** They cannot live inside the database they
/// open, which is the whole of the exception to *"no JSON, no local text file"* — and the
/// password field is a route, so there is no spelling of a password this section accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDatabase {
    name: String,
    role: String,
    password: String,
}

#[must_use]
fn path(global: &Path) -> PathBuf {
    global.join(FILE)
}

/// The file as it stands, parsed — or `None` on a machine that has not been set up.
///
/// One reader, because there are three callers now and a second copy of "parse it and check
/// the version" is a second chance for one of them to skip the check.
fn read(global: &Path) -> Outcome<Option<RawFile>> {
    let file = path(global);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(None);
    };

    let raw: RawFile = toml::from_str(&text).map_err(|error| {
        Failure::usage(format!("{}: {error}", file.display())).hint(
            "it is TOML, and sloop wrote it — if it has been edited by hand, that is where to look",
        )
    })?;

    if raw.version > VERSION {
        return Err(Failure::new(
            Exit::Usage,
            format!("{} was written by a newer sloop", file.display()),
        )
        .hint("upgrade sloop, or delete that file to set up again"));
    }

    Ok(Some(raw))
}

/// The server a previous run settled on, **without starting it**.
///
/// [`reopen`] starts a stopped cluster, which is right for every command that is about to use
/// one and wrong for the one command that is about to delete it. `R19c6` needs to know what is
/// there before it decides what to destroy.
pub fn read_server(global: &Path) -> Outcome<Option<Server>> {
    Ok(read(global)?.as_ref().map(server_from))
}

/// The superuser password this machine keeps for that server, if it can still be read.
///
/// `None` rather than a failure when there is no record: a machine with nothing to open has
/// nothing to fail about. A password that *is* recorded and cannot be fetched is an error,
/// because that is a machine whose secret store has moved out from under it.
pub fn superuser_password(global: &Path, server: &Server) -> Outcome<Option<Secret>> {
    let Some(raw) = read(global)? else {
        return Ok(None);
    };

    let route = Route::parse(&raw.password)?;
    let resolved = crate::secret::resolve(
        &route,
        &crate::secret::Lookup {
            key: &credential_key(server),
            vault: &crate::secret::sealed::Vault::File(
                &global.join(crate::registry::file::SEALED_FILE),
            ),
        },
    )?;

    Ok(Some(resolved.secret))
}

/// The route sloop's own database's password takes, if a previous run settled one.
pub fn database_route(global: &Path) -> Outcome<Option<Route>> {
    match read(global)?.and_then(|raw| raw.database) {
        Some(database) => Ok(Some(Route::parse(&database.password)?)),
        None => Ok(None),
    }
}

/// The database a previous run settled on, by name, without resolving its password.
///
/// **Setup keeps whatever the record names.** The same principle [`reopen`] follows for the
/// server: what a previous run settled comes first, because re-deriving it risks answering
/// differently than it did then. On an ordinary machine there is no record the first time and
/// `Own::sloops` decides; after that this does, which is what makes a second Setup use the
/// database it made rather than looking for one by the name it would have chosen today.
pub fn recorded_own(global: &Path) -> Outcome<Option<super::own::Own>> {
    Ok(read(global)?
        .and_then(|raw| raw.database)
        .map(|database| super::own::Own {
            database: database.name,
            role: database.role,
        }))
}

/// sloop's own database and the password that opens it, as a previous run settled them.
///
/// `None` on a machine that has not been set up, which is the state `R19c4` has to turn into
/// *"run `sloop setup`"* rather than into a crash. The password comes back off whichever of
/// `R3`'s routes the record names — it is never in the record itself.
pub fn database(global: &Path) -> Outcome<Option<(super::own::Own, Secret)>> {
    let Some(raw) = read(global)? else {
        return Ok(None);
    };
    let (Some(database), server) = (raw.database.clone(), server_from(&raw)) else {
        return Ok(None);
    };

    let own = super::own::Own {
        database: database.name,
        role: database.role,
    };
    let password = crate::secret::resolve(
        &Route::parse(&database.password)?,
        &crate::secret::Lookup {
            key: &own.credential_key(&server),
            vault: &crate::secret::sealed::Vault::File(
                &global.join(crate::registry::file::SEALED_FILE),
            ),
        },
    )?
    .secret;

    Ok(Some((own, password)))
}

/// Write down sloop's own database, and put the owning role's password where this machine
/// keeps secrets.
///
/// Hands back the route it took, because the caller is about to say where the password went
/// and re-reading the file it has just written would be a second answer to a settled
/// question.
pub fn remember_database(
    global: &Path,
    server: &Server,
    own: &super::own::Own,
    password: &Secret,
) -> Outcome<Route> {
    let route = crate::secret::keep_somewhere(
        own.credential_key(server),
        password,
        &crate::secret::sealed::Vault::File(&global.join(crate::registry::file::SEALED_FILE)),
    )?;

    let mut raw = read(global)?.ok_or_else(|| {
        Failure::usage(format!(
            "{} is not there to write into",
            path(global).display()
        ))
    })?;

    raw.version = VERSION;
    raw.database = Some(RawDatabase {
        name: own.database.clone(),
        role: own.role.clone(),
        password: route.as_field(),
    });

    save(global, &raw)?;
    Ok(route)
}

/// Write the file, once, from one place.
fn save(global: &Path, raw: &RawFile) -> Outcome<()> {
    let text = toml::to_string_pretty(raw)
        .map_err(|error| Failure::usage(format!("could not write {FILE}: {error}")))?;

    std::fs::create_dir_all(global).map_err(|error| {
        Failure::usage(format!("could not create {}: {error}", global.display()))
    })?;
    std::fs::write(path(global), text)
        .map_err(|error| Failure::usage(format!("could not write {FILE}: {error}")))
}

/// Write down what Setup settled, and put the password where this machine keeps secrets.
pub fn write(global: &Path, server: &Server, password: &Secret) -> Outcome<()> {
    let route = crate::secret::keep_somewhere(
        credential_key(server),
        password,
        &crate::secret::sealed::Vault::File(&global.join(crate::registry::file::SEALED_FILE)),
    )?;

    let raw = RawFile {
        version: VERSION,
        bin: server.bin.display().to_string(),
        data: server.data.as_ref().map(|data| data.display().to_string()),
        port: server.port,
        superuser: server.superuser.clone(),
        password: route.as_field(),
        origin: match server.origin {
            Origin::Machines => "machine",
            Origin::Sloops => "sloop",
        }
        .to_owned(),
        // Kept, if a previous run had settled it. Writing the server record again — which is
        // what re-running Setup does — must not forget which database sloop's state is in.
        database: read(global).ok().flatten().and_then(|raw| raw.database),
    };

    save(global, &raw)
}

/// The server a previous run settled on, started if it had stopped.
///
/// `None` when there is no record, which is every machine before Setup. A record that names
/// a directory which is no longer there is `None` too, with a line saying so: an uninstalled
/// PostgreSQL is a reason to set up again, not a reason to fail.
pub fn reopen(global: &Path) -> Outcome<Option<Ready>> {
    let Some(raw) = read(global)? else {
        return Ok(None);
    };
    let file = path(global);
    let server = server_from(&raw);

    if !server.program("psql").is_file() {
        crate::note!(
            "{}",
            crate::style::dim(&format!(
                "{} names {}, which is not there any more — setting up again.",
                file.display(),
                raw.bin
            ))
        );
        return Ok(None);
    }

    let route = Route::parse(&raw.password)?;
    let password = crate::secret::resolve(
        &route,
        &crate::secret::Lookup {
            key: &credential_key(&server),
            vault: &crate::secret::sealed::Vault::File(
                &global.join(crate::registry::file::SEALED_FILE),
            ),
        },
    )?
    .secret;

    // A cluster sloop made is started if it had stopped; the machine's own is left exactly
    // as it is, because it is not sloop's to start.
    make::start_if_stopped(&server)?;

    Ok(Some(Ready {
        server,
        password,
        made_now: false,
    }))
}

/// The server a record describes.
///
/// One builder, because there are two readers now — [`reopen`] and [`database`] — and a
/// second copy of this is a second chance for one of them to read `origin` differently and
/// start a cluster that is not sloop's to start.
fn server_from(raw: &RawFile) -> Server {
    Server {
        bin: PathBuf::from(&raw.bin),
        data: raw.data.as_ref().map(PathBuf::from),
        port: raw.port,
        superuser: raw.superuser.clone(),
        origin: if raw.origin == "sloop" {
            Origin::Sloops
        } else {
            Origin::Machines
        },
    }
}
