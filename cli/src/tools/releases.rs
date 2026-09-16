//! Which PostgreSQL build sloop will fetch on Windows, and how it knows it got that one.
//!
//! **The hash is pinned, and that is a decision with a reason.** The written plan said
//! "version resolved from the index at request time, checksum verified", and the two turn
//! out to pull against each other: PostgreSQL's own index says which versions exist, but
//! the Windows binaries come from EnterpriseDB, which publishes no checksum beside them —
//! a `.sha256` next to the archive is a 403. So a version resolved at request time could
//! only ever be verified by trusting whatever answered, and "checksum verified" would be a
//! sentence with nothing behind it.
//!
//! What is here instead: a table of releases whose SHA-256 was taken by hand from the real
//! download, and a refusal to install anything that does not match one of them. The index
//! is still read — it is what says whether the pinned major is still supported upstream,
//! and what lets sloop mention a newer PostgreSQL rather than pretend it is the latest —
//! but it decides nothing on its own, so a slow or unreachable postgresql.org cannot stop
//! an install that is otherwise perfectly verifiable.
//!
//! **Going stale is graceful, not broken.** A pinned `pg_dump` dumps every server up to
//! its own major, so the day PostgreSQL 19 ships, this table still gets someone a working
//! backup of everything they had yesterday. `ci/pinned-releases.sh` is what stops it going
//! quietly wrong: it re-downloads every row and fails if a hash has moved, and says so when
//! the index has a supported major no row covers.

/// Where EnterpriseDB keeps the Windows builds. The official ones — postgresql.org links
/// here for Windows, because the project itself publishes no Windows binaries.
const DOWNLOAD_HOST: &str = "https://get.enterprisedb.com/postgresql";

/// PostgreSQL's own machine-readable list of majors, what the latest minor of each is, and
/// which are still supported. Advice, never a gate — see the module comment.
pub const INDEX_URL: &str = "https://www.postgresql.org/versions.json";

/// A Windows build this sloop can prove it received intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    /// `18` in 18.6.
    pub major: u32,
    /// `6` in 18.6.
    pub minor: u32,
    /// EnterpriseDB rebuilds a release without changing its version, and the archives
    /// differ. The build is part of the identity, not decoration.
    pub build: u32,
    /// The exact size of the archive, checked before a byte of it is hashed. A wrong size
    /// is the cheap way to catch a redirect to an error page that arrived with a 200.
    pub bytes: u64,
    /// Lower-case hex SHA-256 of the archive.
    pub sha256: &'static str,
}

impl Release {
    /// Where to get it.
    #[must_use]
    pub fn url(&self) -> String {
        format!("{DOWNLOAD_HOST}/{}", self.file_name())
    }

    /// What the archive is called.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!(
            "postgresql-{}.{}-{}-windows-x64-binaries.zip",
            self.major, self.minor, self.build
        )
    }
}

impl std::fmt::Display for Release {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "PostgreSQL {}.{}", self.major, self.minor)
    }
}

/// The releases this build of sloop will install, newest first.
///
/// **Adding one is a row**, and the hash is the whole point of it, so it is taken from the
/// real download and never from a web page. `ci/pinned-releases.sh` re-checks every row.
pub const PINNED: &[Release] = &[Release {
    major: 18,
    minor: 6,
    build: 3,
    bytes: 344_414_106,
    sha256: "59f8ce701c63c2ed623c665a5e51b3ef6f2e37ccf837b68ffeed0742d0ae6abd",
}];

/// The newest release sloop can install.
#[must_use]
pub fn newest() -> &'static Release {
    PINNED
        .iter()
        .max_by_key(|release| (release.major, release.minor, release.build))
        .expect("there is always at least one pinned release")
}

/// A major, and whether upstream still supports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Major {
    /// `18`.
    pub number: u32,
    /// The newest minor upstream has published for it.
    pub latest_minor: u32,
    /// False once it has reached end of life.
    pub supported: bool,
}

/// Read PostgreSQL's version index.
///
/// Hand-read rather than deserialised, and that is the cheaper trade here: the whole of
/// what is wanted is three fields per entry, against a JSON dependency that would sit in
/// the graph of a tool whose headline claim is about what is *not* in its graph. Anything
/// that does not parse is simply not returned — the index is advice, and advice that
/// arrives malformed is the same as advice that does not arrive.
#[must_use]
pub fn read_index(json: &str) -> Vec<Major> {
    let mut majors = Vec::new();

    for object in json.split('{').skip(1) {
        let object = &object[..object.find('}').unwrap_or(object.len())];

        let Some(number) = field(object, "major").and_then(|value| {
            // `"major": "9.6"` for the old ones: everything before the dot is the major,
            // and a major with a dot in it is long past anything sloop would install.
            value
                .split('.')
                .next()
                .and_then(|digits| digits.parse::<u32>().ok())
        }) else {
            continue;
        };

        majors.push(Major {
            number,
            latest_minor: field(object, "latestMinor")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            supported: field(object, "supported").is_some_and(|value| value == "true"),
        });
    }

    majors
}

/// One `"name": value` out of a flat JSON object, quotes stripped.
fn field<'a>(object: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("\"{name}\"");
    let after = object.split_once(&key)?.1.split_once(':')?.1.trim();
    let value = after.split(',').next()?.trim().trim_end_matches('}').trim();
    Some(value.trim_matches('"'))
}

/// What the index says that is worth telling somebody mid-install.
///
/// Only ever a sentence. A newer PostgreSQL than sloop has a hash for is a thing worth
/// knowing and never a reason to refuse: the pinned build still dumps every server up to
/// its own major, which is every server that existed when it was pinned.
#[must_use]
pub fn what_the_index_adds(majors: &[Major], installing: &Release) -> Option<String> {
    if majors.is_empty() {
        return None;
    }

    let newest_supported = majors
        .iter()
        .filter(|major| major.supported)
        .map(|major| major.number)
        .max()?;

    let ours = majors
        .iter()
        .find(|major| major.number == installing.major)?;

    // End of life first, and before "something newer exists", because both are true of a
    // pin that has fallen far enough behind and only one of them is the serious half.
    if !ours.supported {
        return Some(format!(
            "postgresql.org no longer supports PostgreSQL {}, and lists {newest_supported} \
             as the newest that is. This sloop installs {installing} — the build it holds a \
             checksum for — which still dumps any server up to PostgreSQL {}.",
            installing.major, installing.major
        ));
    }

    if newest_supported > installing.major {
        return Some(format!(
            "postgresql.org lists {newest_supported} as the newest supported release. This \
             sloop installs {installing}, which dumps any server up to PostgreSQL \
             {}; a newer sloop will carry a newer one.",
            installing.major
        ));
    }

    if ours.latest_minor > installing.minor {
        return Some(format!(
            "postgresql.org has {}.{} out. This sloop installs {installing} — the build it \
             holds a checksum for.",
            ours.number, ours.latest_minor
        ));
    }

    None
}
