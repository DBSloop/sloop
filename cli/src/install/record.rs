//! `<global>/servers.toml` — every server sloop installed, and never one of their passwords.
//!
//! **A second file rather than a second section of `server.toml`, and the separation is the
//! point.** That file holds the PostgreSQL sloop keeps its *own* state in, and `reset`
//! destroys what it names. This one holds servers somebody installed to put their *own*
//! databases in, and `reset` has never heard of it — which is `R19c6`'s rule about backups,
//! one step along: a thing the user put data into is not sloop's to throw away.
//!
//! **It cannot live in the database either**, for the reason `server.toml` cannot: a record
//! of how to open a server is no use inside a server. So it is a file, it holds paths, ports
//! and a role name, and the password field holds one of `R3`'s four routes — the word
//! `keyring`, never a secret. There is no spelling of a password this file would accept.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::{Route, Secret};

use super::{Installed, credential_key};

/// What the file is called.
pub const FILE: &str = "servers.toml";

/// Bumped only when the shape below changes in a way an older sloop could misread.
const VERSION: u32 = 1;

/// The file, exactly as TOML sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    version: u32,
    /// `[[server]]`, so that adding the second one is appending a table rather than
    /// rewriting the file into a different shape.
    #[serde(default, rename = "server", skip_serializing_if = "Vec::is_empty")]
    servers: Vec<RawServer>,
}

/// One installed server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    engine: String,
    version: String,
    home: String,
    bin: String,
    data: String,
    port: u16,
    superuser: String,
    /// A route, never a value — see [`Route::parse`], which refuses anything that looks like
    /// a password.
    password: String,
    /// When it was installed, UTC, in the shape `R8` writes every path in.
    installed_at: String,
}

#[must_use]
fn path(global: &Path) -> PathBuf {
    global.join(FILE)
}

/// The file as it stands, parsed — or an empty one on a machine that has installed nothing.
fn parse(global: &Path) -> Outcome<RawFile> {
    let file = path(global);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(RawFile {
            version: VERSION,
            servers: Vec::new(),
        });
    };

    let raw: RawFile = toml::from_str(&text).map_err(|error| {
        Failure::usage(format!("{}: {error}", file.display())).hint(
            "it is TOML, and sloop wrote it — if it has been edited by hand, that is where to \
             look",
        )
    })?;

    if raw.version > VERSION {
        return Err(Failure::new(
            Exit::Usage,
            format!("{} was written by a newer sloop", file.display()),
        )
        .hint("upgrade sloop"));
    }

    Ok(raw)
}

/// Every server sloop has installed on this machine, oldest first.
///
/// **A row naming a directory that is gone is dropped rather than reported.** Somebody who
/// deleted the directory has uninstalled that server, and a menu that kept offering to start
/// it would be arguing with them.
pub fn read(global: &Path) -> Outcome<Vec<Installed>> {
    Ok(parse(global)?
        .servers
        .iter()
        .filter_map(installed_from)
        .filter(|installed| installed.bin.is_dir())
        .collect())
}

/// The ports this machine has already given out, whether or not anything is listening on
/// them right now.
///
/// **Asked as well as binding**, because a server sloop installed and then stopped leaves its
/// port bindable — and handing that same port to the next install would collide the moment
/// somebody started the first one again.
#[must_use]
pub fn ports(global: &Path) -> Vec<u16> {
    let mut held: Vec<u16> = parse(global)
        .map(|raw| raw.servers.iter().map(|server| server.port).collect())
        .unwrap_or_default();

    // **And sloop's own state cluster, which is not in this file.** It lives on 5433 and is
    // written down in `server.toml`; a machine where it happens to be stopped would otherwise
    // be offered 5433 for a PostgreSQL somebody is about to put their databases in, and the
    // collision would turn up the next time anything ran `sloop setup`.
    if let Ok(Some(ours)) = crate::server::record::read_server(global) {
        held.push(ours.port);
    }

    held
}

/// Write down an installed server, and put its superuser password where this machine keeps
/// secrets.
///
/// **The password goes first.** A record naming a server whose password was never stored is
/// a server nobody can open, and this way round a failure leaves no record at all.
pub fn remember(global: &Path, installed: &Installed, password: &Secret) -> Outcome<()> {
    let route = crate::secret::keep_somewhere(
        credential_key(installed),
        password,
        &crate::secret::sealed::Vault::File(&global.join(crate::registry::file::SEALED_FILE)),
    )?;

    let mut raw = parse(global)?;
    raw.version = VERSION;
    raw.servers.retain(|held| !is_it(held, installed));
    raw.servers.push(RawServer {
        engine: installed.engine.scheme().to_owned(),
        version: installed.version.clone(),
        home: installed.home.display().to_string(),
        bin: installed.bin.display().to_string(),
        data: installed.data.display().to_string(),
        port: installed.port,
        superuser: installed.superuser.clone(),
        password: route.as_field(),
        installed_at: crate::backup::stamp::Stamp::now().utc_path(),
    });

    save(global, &raw)
}

/// The superuser password of an installed server, off whichever route its record names.
pub fn password_for(global: &Path, installed: &Installed) -> Outcome<Secret> {
    let raw = parse(global)?;
    let held = raw
        .servers
        .iter()
        .find(|held| is_it(held, installed))
        .ok_or_else(|| {
            Failure::new(
                Exit::Usage,
                format!(
                    "{} names no {}",
                    path(global).display(),
                    installed.describe()
                ),
            )
            .hint("`sloop server list` shows the servers sloop installed on this machine")
        })?;

    Ok(crate::secret::resolve(
        &Route::parse(&held.password)?,
        &crate::secret::Lookup {
            key: &credential_key(installed),
            vault: &crate::secret::sealed::Vault::File(
                &global.join(crate::registry::file::SEALED_FILE),
            ),
        },
    )?
    .secret)
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

/// Is this row that server? Engine and version together, which is what the directory under
/// `servers/` is named after and therefore what makes one install distinct from another.
fn is_it(held: &RawServer, installed: &Installed) -> bool {
    held.engine == installed.engine.scheme() && held.version == installed.version
}

/// One row, as the rest of the code sees it.
///
/// `None` for a row naming an engine this build does not speak, which is what a record
/// written by a later sloop looks like from here: skipped, not a failure, because the other
/// rows are still perfectly readable.
fn installed_from(raw: &RawServer) -> Option<Installed> {
    let engine = Engine::ALL
        .into_iter()
        .find(|engine| engine.scheme() == raw.engine)?;

    Some(Installed {
        engine,
        version: raw.version.clone(),
        home: PathBuf::from(&raw.home),
        bin: PathBuf::from(&raw.bin),
        data: PathBuf::from(&raw.data),
        port: raw.port,
        superuser: raw.superuser.clone(),
    })
}
