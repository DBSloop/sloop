//! `manifest.json` — everything about a backup except the backup.
//!
//! **A dump on its own is a file of unknown provenance.** The manifest is what turns it
//! into a backup: which server it came from and which version that server was, how big the
//! dump is and what it hashes to, how long it took, what was in it table by table, and
//! exactly when it was taken — in UTC, in the local clock, and with the offset between them
//! so both survive being read on another machine.
//!
//! **It is written last, and that is load-bearing.** The dump is written, then checksummed,
//! then this file appears. So a directory with a `manifest.json` in it is a finished
//! backup, and one without is a run that died halfway — which is the distinction `R12` has
//! to make in order to report a half-written backup rather than silently counting it, and
//! the reason `backup` refuses to overwrite a directory that already has one.
//!
//! **Nothing in here is a secret.** `source` is the same connection string a server's own
//! log shows, with no password in it, because the whole file has to be something a person
//! can paste into a public issue when a restore goes wrong.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::engine::{Engine, ServerInfo, TableCount};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use super::stamp::{Local, Stamp};

/// The file's name, inside a backup's directory.
pub const FILE: &str = "manifest.json";

/// Bumped only when the shape below changes in a way an older sloop could misread.
const VERSION: u32 = 1;

/// How much is read at a time when hashing a dump. A dump is measured in gigabytes and
/// nothing is gained by holding one in memory.
const CHUNK: usize = 64 * 1024;

/// One backup, described.
///
/// The field order is the order it is written in — `serde_json` keeps it — so the file
/// opens with what it is and what it came from rather than with a wall of row counts.
///
/// `PartialEq` and no `Eq`, because two of the fields are durations in seconds and a
/// float has no total equality. Nothing compares two manifests for identity; the tests
/// compare one against what was written, which is what `PartialEq` is for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The shape of this file. `version`, the same word the registry file uses for the
    /// same thing, and not to be confused with `server.version` a few lines down.
    pub version: u32,
    /// The sloop that wrote it.
    pub sloop: String,
    /// The name the database is registered under — what somebody goes looking for.
    pub label: String,
    /// Which engine.
    pub engine: Engine,
    /// The connection, exactly as a server's log would show it. Never a password.
    pub source: String,
    /// The database's own name on the server.
    pub database: String,
    /// What answered.
    pub server: Server,
    /// When, said three ways so that none of them has to be recomputed later.
    pub taken: Taken,
    /// The whole backup: counting, dumping and hashing.
    pub took_seconds: f64,
    /// `pg_dump` or `mysqldump` alone, which is the number worth comparing week to week.
    pub dump_seconds: f64,
    /// The file beside this one.
    pub dump: Dump,
    /// Every row in the source, added up.
    pub rows: u64,
    /// Exact `count(*)` per table, never an estimate.
    pub tables: Vec<Count>,
}

/// What answered when sloop connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    /// `17.9`, `8.4`, `11.4`.
    pub version: String,
    /// Whether the connection that took this backup was encrypted.
    pub tls: bool,
}

/// The moment, three ways.
///
/// **All three, because each answers a question the others cannot.** `utc` matches the
/// directory name and sorts. `local` is what the person's clock said, which is what they
/// remember. `offset` is what makes `local` mean anything on a machine somewhere else —
/// without it, a backup carried to another country reads as a lie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Taken {
    /// `2026-09-16T03:15:00Z`.
    pub utc: String,
    /// `2026-09-16T08:45:00+05:30`.
    pub local: String,
    /// `+05:30`.
    pub offset: String,
    /// The same offset in seconds, for anything that has to do arithmetic on it.
    pub offset_seconds: i32,
    /// Seconds since the epoch, for anything that would otherwise parse the strings above.
    pub unix: i64,
}

/// The dump file itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dump {
    /// Its name in this directory. A name rather than a path, so moving the directory
    /// somewhere else does not turn the manifest into a set of broken references.
    pub file: String,
    /// Its size, as it sits on the disk — the encrypted size, when it is encrypted.
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the file as stored, so an intact backup can be told from a
    /// corrupted one without holding the key.
    pub sha256: String,
    /// How it is encrypted, when it is. Absent means the dump is plaintext.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<Encrypted>,
}

/// What a restore has to be able to undo.
///
/// The recipient is recorded because it is the only thing that can tell somebody *which*
/// key they need, on a machine that has several or none. It is a public key: writing it down
/// gives nothing away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Encrypted {
    /// `age`, and there is no second format planned.
    pub format: String,
    /// The public key it was encrypted to.
    pub recipient: String,
}

/// One table, and exactly how many rows were in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Count {
    /// The schema it lives in. MySQL has none and puts the database name here.
    pub schema: String,
    /// The table's own name.
    pub name: String,
    /// `count(*)`.
    pub rows: u64,
}

impl Manifest {
    /// Describe a backup that has just been taken.
    pub fn of(described: Described<'_>) -> Self {
        let local = described.taken.local();

        Self {
            version: VERSION,
            sloop: env!("CARGO_PKG_VERSION").to_owned(),
            label: described.label.to_owned(),
            engine: described.server.engine,
            source: described.source.to_owned(),
            database: described.database.to_owned(),
            server: Server {
                version: described.server.version.to_string(),
                tls: described.server.tls,
            },
            taken: Taken {
                utc: described.taken.utc_iso(),
                local: local.iso(),
                offset: local.offset_label(),
                offset_seconds: local.offset_seconds(),
                unix: described.taken.unix_seconds(),
            },
            took_seconds: seconds(described.took),
            dump_seconds: seconds(described.dump_took),
            dump: Dump {
                file: described.dump_file.to_owned(),
                bytes: described.bytes,
                sha256: described.sha256,
                encryption: described.sealed_to.map(|recipient| Encrypted {
                    format: crate::crypt::SUFFIX.to_owned(),
                    recipient: recipient.to_string(),
                }),
            },
            rows: described.counts.iter().map(|count| count.rows).sum(),
            tables: described
                .counts
                .iter()
                .map(|count| Count {
                    schema: count.table.schema.clone(),
                    name: count.table.name.clone(),
                    rows: count.rows,
                })
                .collect(),
        }
    }

    /// When it was taken, on the clock of the machine that took it.
    #[must_use]
    pub fn taken_locally(&self) -> Local {
        Local::at(
            Stamp::from_unix_seconds(self.taken.unix),
            self.taken.offset_seconds,
        )
    }

    /// Write it into a backup's directory.
    ///
    /// A plain write rather than the write-and-rename the registry uses: this directory
    /// was created by this run, is named after a second that has already passed, and has
    /// no reader until the file is complete. What a half-written one means is settled —
    /// it means the run died, and the next reader says so.
    pub fn write(&self, directory: &Path) -> Outcome<PathBuf> {
        let path = path_in(directory);

        let mut text = serde_json::to_string_pretty(self).map_err(|error| {
            Failure::new(
                Exit::Dump,
                format!("could not describe the backup just taken: {error}"),
            )
        })?;
        // Every other file this project writes ends in one, and a file that does not is a
        // file that looks truncated to anything reading it in a terminal.
        text.push('\n');

        std::fs::write(&path, text.as_bytes()).map_err(|error| {
            Failure::new(
                Exit::Dump,
                format!("could not write {}: {error}", path.display()),
            )
            .hint("the dump itself is there; without this file it is not a backup sloop can read")
        })?;

        Ok(path)
    }

    /// Read one back.
    ///
    /// `R12` lists and prunes by what these say, and `R13` restores what one describes;
    /// today the tests are what read it back, which is how the shape stays honest — a
    /// file that cannot be parsed into the struct that wrote it is a format nobody has
    /// checked.
    #[allow(dead_code)]
    pub fn read(path: &Path) -> Outcome<Self> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("could not read {}: {error}", path.display()),
            )
        })?;

        let manifest: Self = serde_json::from_str(&text).map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("{} is not a backup manifest: {error}", path.display()),
            )
            .hint("sloop wrote it — if it has been edited by hand, that is where to look")
        })?;

        if manifest.version > VERSION {
            return Err(Failure::new(
                Exit::Usage,
                format!(
                    "{} says version {} and this sloop understands version {VERSION}",
                    path.display(),
                    manifest.version
                ),
            )
            .hint("a newer sloop wrote it"));
        }

        Ok(manifest)
    }
}

/// Everything [`Manifest::of`] needs, named rather than positional.
///
/// Nine arguments in a row is nine chances to pass the duration where the dump's duration
/// goes and never find out.
pub struct Described<'a> {
    /// The name the database is registered under.
    pub label: &'a str,
    /// The connection string, with no password in it.
    pub source: &'a str,
    /// The database's name on the server.
    pub database: &'a str,
    /// What the server said about itself.
    pub server: &'a ServerInfo,
    /// When the backup started.
    pub taken: Stamp,
    /// How long all of it took.
    pub took: std::time::Duration,
    /// How long the dump program alone took.
    pub dump_took: std::time::Duration,
    /// The dump's file name inside the directory.
    pub dump_file: &'a str,
    /// Its size.
    pub bytes: u64,
    /// Its checksum.
    pub sha256: String,
    /// The key it was encrypted to, when it was.
    pub sealed_to: Option<&'a crate::crypt::PublicKey>,
    /// The source's exact row counts.
    pub counts: &'a [TableCount],
}

/// Where the manifest goes, inside a backup's directory.
#[must_use]
pub fn path_in(directory: &Path) -> PathBuf {
    directory.join(FILE)
}

/// Is there a finished backup in this directory?
///
/// The manifest is written last, so its presence is the only thing that distinguishes a
/// complete backup from a run that was killed with a half-written dump on disk.
#[must_use]
pub fn completed(directory: &Path) -> bool {
    path_in(directory).is_file()
}

/// The SHA-256 of a file, in lowercase hex.
///
/// Streamed. A dump is measured in gigabytes and reading one into memory to hash it would
/// be a backup tool falling over on exactly the databases worth backing up.
pub fn checksum(file: &Path) -> Outcome<String> {
    use std::io::Read as _;

    let mut handle = std::fs::File::open(file).map_err(|error| {
        Failure::new(
            Exit::Dump,
            format!("could not read {} to check it: {error}", file.display()),
        )
    })?;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        let read = handle.read(&mut buffer).map_err(|error| {
            Failure::new(
                Exit::Dump,
                format!("could not read {}: {error}", file.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hex(&hasher.finalize()))
}

/// Bytes as lowercase hex.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut text, byte| {
        // Writing to a String cannot fail, and the alternative is a `format!` per byte.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// A duration as seconds, to the millisecond.
///
/// Rounded on the way in rather than at every display, so the number in the file is the
/// number that gets printed and nobody has to wonder which of two roundings they are
/// looking at.
fn seconds(took: std::time::Duration) -> f64 {
    (took.as_secs_f64() * 1_000.0).round() / 1_000.0
}
