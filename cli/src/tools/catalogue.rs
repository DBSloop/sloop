//! What the menu can offer to install: which engines, which versions, and where each build
//! comes from.
//!
//! **The list of engines is longer than the list sloop speaks, on purpose.** `R19d` asks for
//! *"the ones sloop does not yet speak, named as not yet supported"* — because a menu that
//! silently omits MongoDB reads as a tool that has never heard of it, and one that says *"not
//! yet"* reads as a tool that knows exactly where it stands. The `engine` table `R19c3` built
//! has a `supported` column for the same reason.
//!
//! **Where a version comes from is per engine, and it is not a style choice** — it is what
//! each project actually publishes, which `proof` documents in full:
//!
//! ```text
//! PostgreSQL   the pinned table, because EnterpriseDB publishes no checksum at all
//! MariaDB      downloads.mariadb.org's REST API, which gives a sha256 per file — so the
//!              versions offered are the ones that really exist, resolved at request time
//! MySQL        a list of series this build knows, verified by Oracle's GPG signature
//! ```
//!
//! **Why MySQL's versions are a list and its hashes are not.** Oracle publishes no index a
//! program can read — the download page is `?id=556213`, an opaque number per file — so the
//! *versions* have to be carried. What is **not** carried is the integrity: the signature
//! covers that, and it is checked against Oracle's own key. A version that has aged out
//! simply stops resolving, and the run says so rather than installing something else.
//!
//! **The MariaDB index is read with `serde_json`, which is already in the graph.** An earlier
//! pass read it by hand, on the reasoning that three fields were not worth a dependency —
//! and running it against the real endpoint showed the document is not the shape that
//! assumed: the SHA-256 is nested under `checksum` beside an MD5 and a SHA-1, and the
//! download URL comes *after* that object rather than before it. A parser that cannot read
//! the thing it parses is not a saving. `serde_json` has been in this graph since `R9`'s
//! manifests and nothing in it can open a socket.
//!
//! **Nothing here opens a socket.** Reading the MariaDB index is done through a [`Reader`]
//! the caller hands in — which in the running program is `curl`, the same way the download
//! is, and in the tests is a table of the real shapes the real endpoints return. So the two
//! index readers are exercised against reality without this module ever being the thing that
//! reaches the network.

#[cfg(test)]
#[path = "catalogue_tests.rs"]
mod tests;

use std::fmt::Write as _;

use serde::Deserialize;

use crate::failure::Outcome;

use crate::engine::Engine;

use super::proof::Proof;

/// One row of the engine screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listed {
    /// How it is written for a person.
    pub name: &'static str,
    /// What it is, in the phrase under the name.
    pub blurb: &'static str,
    /// Which engine sloop speaks, or `None` for one it does not.
    pub engine: Option<Engine>,
}

impl Listed {
    /// Can sloop install and then actually use this one?
    #[must_use]
    pub const fn supported(&self) -> bool {
        self.engine.is_some()
    }
}

/// Every engine the screen lists, supported first and the rest named as not yet.
///
/// **The unsupported rows are not a wishlist.** They are the four somebody most often turns
/// up expecting, so that the answer is on the screen rather than found out by searching for
/// a menu item that is not there.
pub const ENGINES: &[Listed] = &[
    Listed {
        name: "PostgreSQL",
        blurb: "what sloop keeps its own state in",
        engine: Some(Engine::Postgres),
    },
    Listed {
        name: "MySQL",
        blurb: "Oracle's, verified by their GPG signature",
        engine: Some(Engine::Mysql),
    },
    Listed {
        name: "MariaDB",
        blurb: "a fork of MySQL with its own dump program",
        engine: Some(Engine::Mariadb),
    },
    Listed {
        name: "MongoDB",
        blurb: "not yet — sloop speaks SQL engines only",
        engine: None,
    },
    Listed {
        name: "SQL Server",
        blurb: "not yet",
        engine: None,
    },
    Listed {
        name: "SQLite",
        blurb: "not yet — a file rather than a server, so most of sloop would not apply",
        engine: None,
    },
    Listed {
        name: "Oracle Database",
        blurb: "not yet",
        engine: None,
    },
];

/// What this machine is, in the words each project puts in its file names.
///
/// `None` on a platform no project publishes an archive for, which is the honest answer for
/// 32-bit Windows and for anything exotic: a menu that offered an install that cannot work
/// would be worse than one that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// `winx64`
    Windows,
    /// `linux-glibc2.28-x86_64`
    Linux,
    /// `macos*-arm64` or `macos*-x86_64`
    MacOs,
}

impl Platform {
    /// What this machine is running, or `None` where nothing is published.
    #[must_use]
    pub fn here() -> Option<Self> {
        if !cfg!(target_arch = "x86_64") && !cfg!(target_arch = "aarch64") {
            return None;
        }

        if cfg!(windows) {
            // Every one of these projects publishes 64-bit Windows only.
            cfg!(target_arch = "x86_64").then_some(Self::Windows)
        } else if cfg!(target_os = "macos") {
            Some(Self::MacOs)
        } else if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else {
            None
        }
    }

    /// How MySQL spells this platform in an archive name.
    #[must_use]
    pub const fn mysql_suffix(self) -> &'static str {
        match self {
            Self::Windows => "winx64.zip",
            Self::Linux => "linux-glibc2.28-x86_64.tar.xz",
            // Oracle names the macOS build after the macOS it was built on, which moves every
            // year — so macOS is resolved from the index rather than from this table. See
            // `mysql_builds`.
            Self::MacOs => "",
        }
    }

    /// How MariaDB's REST API spells this platform in its `os` field.
    #[must_use]
    pub const fn mariadb_os(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
        }
    }
}

/// One build the menu can offer, and everything needed to get it and prove it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// Which engine.
    pub engine: Engine,
    /// `18.6`, `8.4.11`, `11.4.4` — as the project writes it.
    pub version: String,
    /// What the archive is called.
    pub file_name: String,
    /// Where to get it.
    pub url: String,
    /// How it will be proved once it is here.
    pub proof: Proof,
}

impl Build {
    /// How it reads on the version screen.
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{} {}", named(self.engine), self.version)
    }
}

/// How an engine is written for a person.
#[must_use]
pub fn named(engine: Engine) -> &'static str {
    engine.proper_name()
}

// ---------------------------------------------------------------------------------------
// PostgreSQL — the pinned table
// ---------------------------------------------------------------------------------------

/// Every PostgreSQL build this sloop will install, newest first.
///
/// **Windows only, and that is not an omission.** On Linux and macOS a PostgreSQL from the
/// package manager is the one the rest of the system expects, and `acquire` already offers to
/// run it — installing a second one under `~/.sloop` would leave two servers, two sets of
/// client programs and a support question nobody can answer. The archive path exists because
/// Windows has no package manager that carries PostgreSQL by default.
#[must_use]
pub fn postgres_builds() -> Vec<Build> {
    if Platform::here() != Some(Platform::Windows) {
        return Vec::new();
    }

    super::releases::PINNED
        .iter()
        .map(|release| Build {
            engine: Engine::Postgres,
            version: format!("{}.{}", release.major, release.minor),
            file_name: release.file_name(),
            url: release.url(),
            proof: Proof::Pinned {
                bytes: release.bytes,
                sha256: release.sha256.to_owned(),
            },
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// MySQL — a carried list of versions, and Oracle's signature
// ---------------------------------------------------------------------------------------

/// Where Oracle's archives really live.
///
/// **Not `dev.mysql.com/downloads/file/?id=556213`**, which is what the download page links
/// to: that number is per file, changes every release, and is published nowhere a program can
/// read. The CDN path is derivable from the version, and the detached signature sits beside
/// the archive at the same path plus `.asc` — checked against the real endpoint rather than
/// assumed.
const MYSQL_CDN: &str = "https://cdn.mysql.com/Downloads";

/// Oracle's release-engineering key, and the older ones it replaced.
///
/// **Carried rather than fetched from a keyserver.** The key *is* the trust anchor: fetching
/// it over the same network as the archive would prove nothing, and a keyserver that is down
/// would stop an install that is otherwise perfectly verifiable. Sloop imports these into a
/// keyring of its own for the check and never touches the user's.
pub const MYSQL_KEYS: &[(&str, &str)] = &[
    (
        "BCA43417C3B485DD128EC6D4B7B3B788A8D3785C",
        "MySQL Release Engineering",
    ),
    (
        "A4A9406876FCBD3C456770C88C718D3B5072E1F5",
        "MySQL Release Engineering (2022)",
    ),
];

/// The MySQL series this build knows about, newest first.
///
/// **A carried list, and only of *which versions exist*.** Oracle publishes no index a
/// program can read, so the versions have to come from somewhere; what is emphatically not
/// carried is the integrity, which is the signature's job. A row that has aged out stops
/// resolving and the run says so — it never quietly installs something else.
const MYSQL_VERSIONS: &[&str] = &["9.5.0", "8.4.11", "8.0.44"];

/// Every MySQL build this machine can be offered.
#[must_use]
pub fn mysql_builds() -> Vec<Build> {
    let Some(platform) = Platform::here() else {
        return Vec::new();
    };
    let suffix = platform.mysql_suffix();
    if suffix.is_empty() {
        // macOS: Oracle names the build after the macOS it was made on, which moves yearly.
        // Offering a guess that 404s is worse than saying the package manager is the way.
        return Vec::new();
    }

    MYSQL_VERSIONS
        .iter()
        .filter_map(|version| {
            let series = series_of(version)?;
            let file_name = format!("mysql-{version}-{suffix}");
            let url = format!("{MYSQL_CDN}/MySQL-{series}/{file_name}");

            Some(Build {
                engine: Engine::Mysql,
                version: (*version).to_owned(),
                file_name,
                url: url.clone(),
                proof: Proof::Signed {
                    signature_url: format!("{url}.asc"),
                    keys: MYSQL_KEYS,
                },
            })
        })
        .collect()
}

/// `8.4.11` is in the `MySQL-8.4` directory. `9.5.0` is in `MySQL-9.5`.
fn series_of(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    (!major.is_empty() && !minor.is_empty()).then(|| format!("{major}.{minor}"))
}

// ---------------------------------------------------------------------------------------
// MariaDB — resolved at request time, and really verified
// ---------------------------------------------------------------------------------------

/// MariaDB's own machine-readable index.
///
/// **The one engine where "versions available" means what it sounds like.** Every file entry
/// carries a SHA-256, so a version can be resolved now and still be proved — which is exactly
/// what `R19d` asked for and what the other two cannot give.
pub const MARIADB_INDEX: &str = "https://downloads.mariadb.org/rest-api/mariadb/";

/// `/rest-api/mariadb/` — the majors.
#[derive(Debug, Deserialize)]
struct Majors {
    #[serde(default)]
    major_releases: Vec<MajorRow>,
}

/// One major, as the index writes it.
///
/// The field names are the index's, not sloop's — renaming them here to please a lint about a
/// shared prefix would mean a `#[serde(rename)]` on each one saying what they are really
/// called, which is the same three words in a less obvious place.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Deserialize)]
struct MajorRow {
    release_id: String,
    #[serde(default)]
    release_status: Option<String>,
    #[serde(default)]
    release_support_type: Option<String>,
}

/// `/rest-api/mariadb/<major>/latest/` — one major's releases, keyed by version.
#[derive(Debug, Deserialize)]
struct Releases {
    #[serde(default)]
    releases: std::collections::HashMap<String, ReleaseRow>,
}

/// One release, and the files it publishes.
#[derive(Debug, Deserialize)]
struct ReleaseRow {
    release_id: String,
    #[serde(default)]
    release_status: Option<String>,
    #[serde(default)]
    files: Vec<FileRow>,
}

/// One published file.
///
/// **The hash is in two places depending on which endpoint answered**, which is not a guess:
/// the release endpoint nests it under `checksum`, beside an MD5 and a SHA-1. Both are read
/// and neither is required, because a file with no usable hash is dropped rather than offered
/// unverified.
#[derive(Debug, Deserialize)]
struct FileRow {
    file_name: String,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    checksum: Option<Checksum>,
    #[serde(default)]
    sha256sum: Option<String>,
    #[serde(default)]
    file_download_url: Option<String>,
}

/// The checksums beside one file.
#[derive(Debug, Deserialize)]
struct Checksum {
    #[serde(default)]
    sha256sum: Option<String>,
}

impl FileRow {
    /// The SHA-256, wherever this endpoint put it, and only if it really is one.
    fn sha256(&self) -> Option<&str> {
        self.checksum
            .as_ref()
            .and_then(|checksum| checksum.sha256sum.as_deref())
            .or(self.sha256sum.as_deref())
            .filter(|hash| super::proof::is_hex_sha256(hash))
    }
}

/// Every stable major the index lists, newest first, with what kind of release it is.
///
/// **Majors rather than every version ever published.** Somebody picking a server out of a
/// menu means "MariaDB 11.8", not a choice between 11.8.2 and 11.8.1 — and asking the index
/// for every major's releases to fill one screen would be a dozen requests, eleven of whose
/// answers get thrown away.
#[must_use]
pub fn read_mariadb_majors(json: &str) -> Vec<Choice> {
    let Ok(read) = serde_json::from_str::<Majors>(json) else {
        return Vec::new();
    };

    let mut majors: Vec<Choice> = read
        .major_releases
        .into_iter()
        // Stable only: an alpha or an RC is not what somebody picking from a menu means by
        // "the versions available".
        .filter(|major| major.release_status.as_deref() == Some("Stable"))
        .map(|major| Choice {
            version: major.release_id,
            note: major.release_support_type.unwrap_or_default(),
            build: None,
        })
        .collect();

    majors.sort_by(|left, right| newest_first(&left.version, &right.version));
    majors.dedup_by(|left, right| left.version == right.version);
    majors
}

/// Every release inside one major, newest first.
#[must_use]
pub fn read_mariadb_versions(json: &str) -> Vec<String> {
    let Ok(read) = serde_json::from_str::<Releases>(json) else {
        return Vec::new();
    };

    let mut versions: Vec<String> = read
        .releases
        .into_values()
        .filter(|release| {
            // Absent is fine and is what the release endpoint actually returns; anything that
            // says it is not stable is skipped.
            release.release_status.as_deref().unwrap_or("Stable") == "Stable"
        })
        .map(|release| release.release_id)
        .collect();

    versions.sort_by(|left, right| newest_first(left, right));
    versions.dedup();
    versions
}

/// The archives one release publishes for this platform, with the hash that proves each.
///
/// Anything that does not parse offers nothing, and **a file with no usable hash is dropped
/// rather than offered unverified** — which is the one line in this module the guarantee
/// rests on.
#[must_use]
pub fn read_mariadb_files(json: &str, platform: Platform, version: &str) -> Vec<Build> {
    let Ok(read) = serde_json::from_str::<Releases>(json) else {
        return Vec::new();
    };

    let Some(release) = read
        .releases
        .into_values()
        .find(|release| release.release_id == version)
    else {
        return Vec::new();
    };

    release
        .files
        .into_iter()
        .filter(|file| is_a_server_archive(file, platform))
        .filter_map(|file| {
            let sha256 = file.sha256()?.to_owned();
            let url = over_https(&file.file_download_url?);

            Some(Build {
                engine: Engine::Mariadb,
                version: version.to_owned(),
                file_name: file.file_name,
                url,
                proof: Proof::Published {
                    sha256,
                    from: "downloads.mariadb.org".to_owned(),
                },
            })
        })
        .collect()
}

/// The same address, over TLS.
///
/// **The index gives `http://` for its own downloads**, which is a real thing the real
/// endpoint really returns — and `acquire::download` refuses anything that is not https at
/// both ends of a redirect chain, deliberately. The host serves both, so the scheme is
/// corrected rather than the refusal being relaxed: the archive is proved by its hash either
/// way, and there is no reason for the bytes to cross the network in the clear on the way to
/// being proved.
fn over_https(url: &str) -> String {
    url.strip_prefix("http://")
        .map_or_else(|| url.to_owned(), |rest| format!("https://{rest}"))
}

/// Is this the binary server archive for this platform?
///
/// The name and, when the index says so, the `os` field beside it. Neither alone is enough:
/// the names carry the platform for every build that matters and the field is what
/// disambiguates the ones that do not.
fn is_a_server_archive(file: &FileRow, platform: Platform) -> bool {
    let file_name = &file.file_name;

    if file_name.contains("debug") || file_name.contains("-src") || file_name.ends_with('/') {
        return false;
    }
    if file
        .os
        .as_deref()
        .is_some_and(|named| named != platform.mariadb_os())
    {
        return false;
    }

    match platform {
        Platform::Windows => file_name.ends_with("winx64.zip"),
        Platform::Linux => file_name.ends_with("x86_64.tar.gz") && file_name.contains("linux"),
        Platform::MacOs => file_name.ends_with(".tar.gz") && file_name.contains("macos"),
    }
}

/// `11.4.4` above `10.11.2`, comparing numbers rather than text — otherwise `9` sorts above
/// `11` and the menu offers the oldest release at the top.
fn newest_first(left: &str, right: &str) -> std::cmp::Ordering {
    let parts = |text: &str| {
        text.split('.')
            .map(|piece| piece.parse::<u32>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    parts(right).cmp(&parts(left))
}

// ---------------------------------------------------------------------------------------
// Turning an engine into a list somebody can choose from
// ---------------------------------------------------------------------------------------

/// How an index is read: a URL in, the text out.
///
/// **A parameter rather than a call to `acquire::download`**, and the reason is the module
/// header's: the resolution below is worth testing against the real shapes the real endpoints
/// return, and a test that had to reach downloads.mariadb.org to do it would be a test that
/// fails when somebody's wifi does. The running program passes a closure that shells out to
/// `curl`; the tests pass one that hands back a string.
pub type Reader<'a> = &'a dyn Fn(&str) -> Outcome<String>;

/// One row of the version screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What it is called: `18.6`, `8.4.11`, or — for MariaDB — a major such as `11.8`.
    pub version: String,
    /// The line under it, when there is something worth saying. Empty otherwise.
    pub note: String,
    /// The build, when it is already known.
    ///
    /// **`None` for MariaDB, which is the one engine that resolves late.** Its index lists
    /// majors, and asking it for every major's files up front would be a dozen requests to
    /// fill a screen where eleven of the answers get thrown away.
    pub build: Option<Build>,
}

/// The versions of one engine this machine can be offered, newest first.
///
/// Empty is a real answer and not a failure: it is what a platform nothing is published for
/// looks like, and the caller says so in the words that platform deserves.
pub fn choices(engine: Engine, read: Reader<'_>) -> Outcome<Vec<Choice>> {
    let from_builds = |builds: Vec<Build>| {
        builds
            .into_iter()
            .map(|build| Choice {
                version: build.version.clone(),
                note: build.proof.describe(),
                build: Some(build),
            })
            .collect()
    };

    match engine {
        Engine::Postgres => Ok(from_builds(postgres_builds())),
        Engine::Mysql => Ok(from_builds(mysql_builds())),
        Engine::Mariadb => {
            if Platform::here().is_none() {
                return Ok(Vec::new());
            }
            Ok(read_mariadb_majors(&read(MARIADB_INDEX)?))
        }
    }
}

/// The build behind a choice, fetching the one index that resolves late.
pub fn resolve(engine: Engine, choice: &Choice, read: Reader<'_>) -> Outcome<Build> {
    if let Some(build) = &choice.build {
        return Ok(build.clone());
    }

    let platform = Platform::here().ok_or_else(|| nothing_published(engine))?;
    let json = read(&mariadb_latest(&choice.version))?;

    // The release inside that major, and then its files. Both out of the one document, which
    // is what the endpoint returns — so there is one request rather than two, and the version
    // that gets installed is the one whose files were read in the same breath.
    let version = read_mariadb_versions(&json)
        .into_iter()
        .next()
        .ok_or_else(|| {
            crate::failure::Failure::new(
                crate::exit::Exit::Failure,
                format!(
                    "downloads.mariadb.org lists no release in MariaDB {}",
                    choice.version
                ),
            )
            .hint("pick another version — nothing has been downloaded")
        })?;

    read_mariadb_files(&json, platform, &version)
        .into_iter()
        .next()
        .ok_or_else(|| {
            crate::failure::Failure::new(
                crate::exit::Exit::Failure,
                format!(
                    "downloads.mariadb.org publishes no {} archive for MariaDB {version} with a \
                     checksum beside it",
                    platform.mariadb_os()
                ),
            )
            .hint(
                "a file with no usable hash is never offered — see the release notes for that \
                 version",
            )
        })
}

/// Where one major release's newest version and its files are listed.
fn mariadb_latest(major: &str) -> String {
    format!("{MARIADB_INDEX}{major}/latest/")
}

/// The one refusal for an engine this platform has nothing published for.
fn nothing_published(engine: Engine) -> crate::failure::Failure {
    crate::failure::Failure::new(
        crate::exit::Exit::Usage,
        format!(
            "nobody publishes a {} archive for this machine",
            named(engine)
        ),
    )
    .hint("install it with this machine's own package manager instead")
}

/// What to say instead of a version list, when there is not one.
///
/// **Not a failure, and not silence either.** A Linux machine asking for PostgreSQL has a
/// perfectly good answer — its package manager — and the whole reason this screen exists is
/// that somebody should not have to go and find that out for themselves.
#[must_use]
pub fn why_nothing_is_offered(engine: Engine) -> String {
    let mut said = String::new();

    if Platform::here().is_none() {
        let _ = write!(
            said,
            "Nobody publishes a {} archive for this machine.",
            named(engine)
        );
        return said;
    }

    match engine {
        Engine::Postgres => {
            let _ = write!(
                said,
                "On this system the PostgreSQL to install is the one the rest of the system \
                 expects, which is the package manager's. Installing a second one under \
                 sloop's own directory would leave two servers and two sets of client \
                 programs."
            );
        }
        Engine::Mysql if Platform::here() == Some(Platform::MacOs) => {
            let _ = write!(
                said,
                "Oracle names its macOS build after the macOS it was built on, which moves \
                 every year, so there is no name sloop can resolve without guessing. \
                 `brew install mysql` is the answer here."
            );
        }
        Engine::Mysql | Engine::Mariadb => {
            let _ = write!(
                said,
                "Nothing is published for this machine that sloop can prove it received \
                 intact."
            );
        }
    }

    said
}
