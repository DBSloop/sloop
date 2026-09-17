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
//! **Nothing here opens a socket.** Reading the MariaDB index is `curl`, the same way the
//! download is, and everything else is a table.

//! **Nothing in the binary reaches this module yet**, for the same reason `proof` does not:
//! the screen that walks engine → version → confirm is the next piece of `R19d` and is what
//! will call both. Until then its reader is `catalogue_tests`, which feeds the two index
//! readers the real shape of what the real endpoints return.
#![allow(dead_code)]

#[cfg(test)]
#[path = "catalogue_tests.rs"]
mod tests;

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
/// carries a `sha256sum`, so a version can be resolved now and still be proved — which is
/// exactly what `R19d` asked for and what the other two cannot give.
pub const MARIADB_INDEX: &str = "https://downloads.mariadb.org/rest-api/mariadb/";

/// Pull the `file_name`, `sha256sum` and `file_download_url` out of one release's index.
///
/// Hand-read for the reason `releases::read_index` gives: three fields against a JSON
/// dependency in the graph of a tool whose headline claim is about what is *not* in its
/// graph. Anything that does not parse is simply not offered — and a file with no usable
/// hash is dropped rather than offered unverified, which is the whole point.
#[must_use]
pub fn read_mariadb_files(json: &str, platform: Platform, version: &str) -> Vec<Build> {
    let mut builds = Vec::new();

    for object in json.split('{').skip(1) {
        let object = &object[..object.find('}').unwrap_or(object.len())];

        let Some(file_name) = field(object, "file_name") else {
            continue;
        };
        // The binary tarball or zip, never a source tarball, a debug build or a directory.
        if !is_a_server_archive(&file_name, platform) {
            continue;
        }

        let Some(sha256) =
            field(object, "sha256sum").filter(|hash| super::proof::is_hex_sha256(hash))
        else {
            continue;
        };
        let Some(url) = field(object, "file_download_url") else {
            continue;
        };

        builds.push(Build {
            engine: Engine::Mariadb,
            version: version.to_owned(),
            file_name,
            url,
            proof: Proof::Published {
                sha256,
                from: "downloads.mariadb.org".to_owned(),
            },
        });
    }

    builds
}

/// Is this the binary server archive for this platform?
fn is_a_server_archive(file_name: &str, platform: Platform) -> bool {
    if file_name.contains("debug") || file_name.contains("-src") || file_name.ends_with('/') {
        return false;
    }

    match platform {
        Platform::Windows => file_name.ends_with("winx64.zip"),
        Platform::Linux => file_name.ends_with("x86_64.tar.gz") && file_name.contains("linux"),
        Platform::MacOs => file_name.ends_with(".tar.gz") && file_name.contains("macos"),
    }
}

/// Every version the index lists, newest first.
#[must_use]
pub fn read_mariadb_versions(json: &str) -> Vec<String> {
    let mut versions: Vec<String> = Vec::new();

    for object in json.split('{').skip(1) {
        let object = &object[..object.find('}').unwrap_or(object.len())];
        // Stable releases only: an alpha or an RC is not what somebody picking from a menu
        // means by "the versions available".
        if field(object, "release_status").is_some_and(|status| status != "Stable") {
            continue;
        }
        if let Some(id) = field(object, "release_id").filter(|id| !versions.contains(id)) {
            versions.push(id);
        }
    }

    versions.sort_by(|left, right| newest_first(left, right));
    versions
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

/// One `"name": value` out of a flat JSON object, quotes stripped.
fn field(object: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\"");
    let after = object.split_once(&key)?.1.split_once(':')?.1.trim();
    let value = after.split(',').next()?.trim().trim_end_matches('}').trim();
    let value = value.trim_matches('"');
    (!value.is_empty() && value != "null").then(|| value.to_owned())
}
