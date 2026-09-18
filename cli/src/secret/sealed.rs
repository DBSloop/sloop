//! The encrypted file, for a Linux machine with no keyring running.
//!
//! Argon2id turns a passphrase into a key; XChaCha20-Poly1305 encrypts the whole store
//! under it. The header carries the Argon2 parameters so a file written today still opens
//! after the defaults are raised, and the header is fed to the cipher as associated data,
//! so editing the parameters to something cheap makes the file fail to open rather than
//! open faster.
//!
//! The plaintext is a length-prefixed list of name/password pairs and not a text format.
//! Nothing inside needs escaping, so a password made entirely of quotes and backslashes
//! round-trips byte for byte — which is the whole reason a text format was not used.

use std::io::IsTerminal;
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, Zeroizing};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;

/// Recognises the file, so a wrong path fails as "not a sloop file" rather than as
/// "wrong passphrase".
const MAGIC: &[u8; 8] = b"SLOOPSEC";

/// Bumped only when the layout below changes.
const VERSION: u8 = 1;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const HEADER_LEN: usize = MAGIC.len() + 1 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN;

/// The variable that supplies the passphrase when there is no terminal to ask at.
pub const PASSPHRASE_VAR: &str = "SLOOP_PASSPHRASE";

/// The same, as a path to read it out of, which is what a service is given.
///
/// **`R24` wrote this into all three unit files and nothing read it, which `R25` found.** A
/// systemd unit is world-readable — `systemctl cat sloop` prints it to anybody with a shell —
/// so the passphrase cannot be *in* the definition; what goes in is the path to a file only
/// the service account can open. That was built, and then the daemon looked for
/// [`PASSPHRASE_VAR`], did not find it, and had no terminal to ask at. `R25` is the first
/// entry where the daemon actually opens the store, so it is the first one that could notice.
///
/// The value is a path and not a secret, which is why it may sit in a unit file at all. What
/// it points at is the secret, under permissions `service::key` sets.
pub const PASSPHRASE_FILE_VAR: &str = "SLOOP_PASSPHRASE_FILE";

/// Has this machine been told to keep secrets in the encrypted file?
///
/// The variable exists for this file and nothing else, so its presence is an answer rather
/// than a hint — `crypt::keep_somewhere` reads it to decide where a new backup key goes.
#[must_use]
pub fn is_the_machines_choice() -> bool {
    std::env::var_os(PASSPHRASE_VAR).is_some_and(|value| !value.is_empty())
        || std::env::var_os(PASSPHRASE_FILE_VAR).is_some_and(|value| !value.is_empty())
}

/// Argon2id cost. These are the crate's own defaults — 19 MiB and two passes — which is
/// the OWASP recommendation, and they are written into every file so raising them later
/// cannot orphan an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cost {
    memory_kib: u32,
    passes: u32,
    lanes: u32,
}

impl Default for Cost {
    fn default() -> Self {
        Self {
            memory_kib: argon2::Params::DEFAULT_M_COST,
            passes: argon2::Params::DEFAULT_T_COST,
            lanes: argon2::Params::DEFAULT_P_COST,
        }
    }
}

/// Somewhere a sealed store's bytes live that is not a file.
///
/// **A trait so that `secret` does not have to know what a table is.** `R19c4` moves the
/// sealed store into `sloop_database`, and the implementation of this lives over in
/// `registry::store` — which already depends on `secret`, so the dependency can only point
/// one way.
pub trait Shelf {
    /// The whole store, or empty when there has never been one.
    fn read(&self) -> Outcome<Vec<u8>>;
    /// Put the whole store back. An empty slice means there is nothing left to keep.
    fn write(&self, blob: &[u8]) -> Outcome<()>;
    /// How it reads in a message. Never a secret — it is a table name or a path.
    fn describe(&self) -> String;
}

/// Where a sealed store lives.
///
/// **Two places, and the split is not arbitrary.** `Rows` is where every registry's passwords
/// went in `R19c4`. `File` is what is left: sloop's own superuser and `sloop_db_admin`
/// passwords, which open the database and therefore cannot be kept inside it.
pub enum Vault<'a> {
    /// A file on disk.
    File(&'a Path),
    /// A row in sloop's own database.
    ///
    /// Boxed because the thing that knows how to reach one row is built on the spot — it is a
    /// store and a registry name together — and there is nowhere for a caller to keep it.
    Rows(Box<dyn Shelf + 'a>),
}

impl Vault<'static> {
    /// No store at all.
    ///
    /// **For a route that does not use one**, which is three of the four: a keyring, an
    /// environment variable and a command all fetch a password from somewhere else entirely.
    /// Reaching this is what a wrong route looks like, so it fails when it is *read* rather
    /// than when it is built — which is what an empty path used to do, and what keeps
    /// `db test` on a `${VAR}` entry working in a registry that has no sealed store at all.
    #[must_use]
    pub fn nowhere() -> Self {
        Self::File(Path::new(""))
    }
}

impl Vault<'_> {
    /// The bytes as they stand, or empty where there is no store yet.
    fn bytes(&self) -> Outcome<Vec<u8>> {
        match self {
            Self::File(path) if !path.is_file() => Ok(Vec::new()),
            Self::File(path) => std::fs::read(path).map_err(|error| {
                Failure::new(
                    Exit::Usage,
                    format!("could not read {}: {error}", path.display()),
                )
            }),
            Self::Rows(rows) => rows.read(),
        }
    }

    /// Put the whole store back.
    fn replace(&self, blob: &[u8]) -> Outcome<()> {
        match self {
            Self::File(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        Failure::new(
                            Exit::Usage,
                            format!("could not create {}: {error}", parent.display()),
                        )
                    })?;
                }
                std::fs::write(path, blob).map_err(|error| {
                    Failure::new(
                        Exit::Usage,
                        format!("could not write {}: {error}", path.display()),
                    )
                })
            }
            Self::Rows(rows) => rows.write(blob),
        }
    }

    /// How it reads in a message.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::Rows(rows) => rows.describe(),
        }
    }
}

/// Read the password stored under `key`.
pub fn get(vault: &Vault<'_>, key: &str) -> Outcome<Secret> {
    let bytes = vault.bytes()?;
    if bytes.is_empty() {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "there is no encrypted password store at {}",
                vault.describe()
            ),
        )
        .hint("run `sloop db add` and choose the encrypted file, or use another route"));
    }

    let passphrase = passphrase(false)?;
    let plaintext = open(&bytes, &passphrase)?;

    entries(&plaintext)?
        .into_iter()
        .find(|(name, _)| name == key)
        .map(|(_, secret)| secret)
        .ok_or_else(|| {
            Failure::new(
                Exit::Usage,
                format!("the encrypted store has no password for {key}"),
            )
            .hint("run `sloop db add` for it")
        })
}

/// Store a password under `key`, rewriting the store.
pub fn put(vault: &Vault<'_>, key: &str, secret: &Secret) -> Outcome<()> {
    rewrite(vault, key, Some(secret))
}

/// Take a password out of the store.
///
/// A key that is not in there is not a failure: `db edit` calls this to clear the old key
/// after moving a password, and a record whose password was never stored has nothing to
/// clear. Reporting that as an error would make a successful edit look like a failed one.
pub fn forget(vault: &Vault<'_>, key: &str) -> Outcome<()> {
    if vault.bytes()?.is_empty() {
        return Ok(());
    }
    rewrite(vault, key, None)
}

/// Read the whole store, replace or drop one entry, and write it back.
fn rewrite(vault: &Vault<'_>, key: &str, secret: Option<&Secret>) -> Outcome<()> {
    let passphrase = passphrase(true)?;

    let bytes = vault.bytes()?;
    let mut stored: Vec<(String, Secret)> = if bytes.is_empty() {
        Vec::new()
    } else {
        entries(&open(&bytes, &passphrase)?)?
    };

    stored.retain(|(name, _)| name != key);
    if let Some(secret) = secret {
        stored.push((key.to_owned(), secret.clone()));
    }

    let plaintext = encode(&stored);
    let sealed = seal(&plaintext, &passphrase)?;

    vault.replace(&sealed)
}

/// Encrypt `plaintext` under `passphrase`.
fn seal(plaintext: &[u8], passphrase: &Zeroizing<String>) -> Outcome<Vec<u8>> {
    let cost = Cost::default();
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    random(&mut salt)?;
    random(&mut nonce)?;

    let header = header(cost, &salt, &nonce);
    let key = derive(passphrase, &salt, cost)?;

    let cipher = XChaCha20Poly1305::new(&Key::from(key));
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &header,
            },
        )
        .map_err(|_| Failure::new(Exit::Failure, "could not encrypt the password file"))?;

    let mut out = header;
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a file produced by [`seal`].
fn open(bytes: &[u8], passphrase: &Zeroizing<String>) -> Outcome<Zeroizing<Vec<u8>>> {
    let not_ours = || {
        Failure::new(Exit::Usage, "that is not a sloop encrypted password file")
            .hint("the file starts with the wrong bytes, so it was written by something else")
    };

    if bytes.len() < HEADER_LEN || &bytes[..MAGIC.len()] != MAGIC {
        return Err(not_ours());
    }

    let mut at = MAGIC.len();
    let version = bytes[at];
    at += 1;
    if version != VERSION {
        return Err(Failure::new(
            Exit::Usage,
            format!("that file is version {version} and this sloop understands version {VERSION}"),
        )
        .hint("a newer sloop wrote it"));
    }

    let cost = Cost {
        memory_kib: u32_at(bytes, &mut at),
        passes: u32_at(bytes, &mut at),
        lanes: u32_at(bytes, &mut at),
    };
    let salt: [u8; SALT_LEN] = bytes[at..at + SALT_LEN]
        .try_into()
        .map_err(|_| not_ours())?;
    at += SALT_LEN;
    let nonce: [u8; NONCE_LEN] = bytes[at..at + NONCE_LEN]
        .try_into()
        .map_err(|_| not_ours())?;
    at += NONCE_LEN;

    let header = &bytes[..HEADER_LEN];
    let key = derive(passphrase, &salt, cost)?;

    let cipher = XChaCha20Poly1305::new(&Key::from(key));
    let plaintext = cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &bytes[at..],
                aad: header,
            },
        )
        .map_err(|_| {
            // One message for a wrong passphrase and for a tampered file, because the
            // cipher cannot tell them apart and guessing out loud would be a lie.
            Failure::new(Exit::Usage, "the encrypted password file would not open").hint(
                "either the passphrase is wrong, or the file has been altered since it was written",
            )
        })?;

    Ok(Zeroizing::new(plaintext))
}

fn header(cost: Cost, salt: &[u8; SALT_LEN], nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(VERSION);
    header.extend_from_slice(&cost.memory_kib.to_le_bytes());
    header.extend_from_slice(&cost.passes.to_le_bytes());
    header.extend_from_slice(&cost.lanes.to_le_bytes());
    header.extend_from_slice(salt);
    header.extend_from_slice(nonce);
    header
}

fn derive(passphrase: &Zeroizing<String>, salt: &[u8], cost: Cost) -> Outcome<[u8; KEY_LEN]> {
    let params = argon2::Params::new(cost.memory_kib, cost.passes, cost.lanes, Some(KEY_LEN))
        .map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("the encrypted file asks for Argon2 settings that are not valid: {error}"),
            )
        })?;

    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    // Wiped by `Zeroizing` on the way out; the copy handed to the cipher lives only as
    // long as the call that uses it.
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut_slice())
        .map_err(|error| Failure::new(Exit::Failure, format!("could not derive a key: {error}")))?;

    Ok(*key)
}

/// Length-prefixed pairs. No separator, so no byte is special and nothing needs escaping.
fn encode(entries: &[(String, Secret)]) -> Zeroizing<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(
        &u32::try_from(entries.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    for (name, secret) in entries {
        push_field(&mut out, name.as_bytes());
        push_field(&mut out, secret.expose().as_bytes());
    }
    Zeroizing::new(out)
}

fn push_field(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
}

fn entries(plaintext: &[u8]) -> Outcome<Vec<(String, Secret)>> {
    let damaged = || {
        Failure::new(
            Exit::Usage,
            "the encrypted password file decrypted but did not make sense",
        )
    };

    let mut at = 0usize;
    let count = take_u32(plaintext, &mut at).ok_or_else(damaged)?;
    let mut out = Vec::new();

    for _ in 0..count {
        let name = take_field(plaintext, &mut at).ok_or_else(damaged)?;
        let value = take_field(plaintext, &mut at).ok_or_else(damaged)?;
        let name = String::from_utf8(name).map_err(|_| damaged())?;
        let mut value = String::from_utf8(value).map_err(|_| damaged())?;
        out.push((name, Secret::new(value.clone())));
        value.zeroize();
    }

    Ok(out)
}

fn take_u32(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(*at..*at + 4)?.try_into().ok()?;
    *at += 4;
    Some(u32::from_le_bytes(slice))
}

fn take_field(bytes: &[u8], at: &mut usize) -> Option<Vec<u8>> {
    let len = take_u32(bytes, at)? as usize;
    let field = bytes.get(*at..*at + len)?.to_vec();
    *at += len;
    Some(field)
}

/// Read `u32` from a header that has already been length-checked.
fn u32_at(bytes: &[u8], at: &mut usize) -> u32 {
    let slice: [u8; 4] = bytes[*at..*at + 4].try_into().unwrap_or([0; 4]);
    *at += 4;
    u32::from_le_bytes(slice)
}

fn random(buffer: &mut [u8]) -> Outcome<()> {
    getrandom::fill(buffer).map_err(|error| {
        Failure::new(
            Exit::Failure,
            format!("the operating system would not provide random bytes: {error}"),
        )
    })
}

/// Get the passphrase that unlocks the file.
///
/// The variable first, so a scheduled run works. Otherwise a prompt — but only when there
/// is a terminal to prompt at. Without one it exits 2 naming the variable, because a
/// scheduled run that stops to ask a question is the worst thing this tool can do.
fn passphrase(confirm: bool) -> Outcome<Zeroizing<String>> {
    if let Ok(value) = std::env::var(PASSPHRASE_VAR)
        && !value.is_empty()
    {
        return Ok(Zeroizing::new(value));
    }

    // **The file next, which is how a service is told.** A variable is inherited by every
    // child a process starts and shows up in `/proc/<pid>/environ`; a path does neither, and
    // the file behind it is readable only by the account the service runs as. See
    // [`PASSPHRASE_FILE_VAR`].
    if let Some(from_a_file) = from_the_file()? {
        return Ok(from_a_file);
    }

    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            "the encrypted password file needs a passphrase, and there is no terminal to ask at",
        )
        .hint(format!(
            "set {PASSPHRASE_VAR} for unattended runs, or {PASSPHRASE_FILE_VAR} to the path \
             of a file holding it"
        )));
    }

    let first = Zeroizing::new(
        rpassword::prompt_password("Passphrase for the encrypted password file: ").map_err(
            |error| {
                Failure::new(
                    Exit::Usage,
                    format!("could not read the passphrase: {error}"),
                )
            },
        )?,
    );

    if confirm {
        let again = Zeroizing::new(rpassword::prompt_password("Again: ").map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("could not read the passphrase: {error}"),
            )
        })?);
        if first.as_str() != again.as_str() {
            return Err(Failure::new(
                Exit::Usage,
                "the two passphrases were different",
            ));
        }
    }

    if first.is_empty() {
        return Err(Failure::new(
            Exit::Usage,
            "an empty passphrase encrypts nothing",
        ));
    }

    Ok(first)
}

/// The passphrase, out of the file [`PASSPHRASE_FILE_VAR`] names.
///
/// **A file that is named and unreadable is a failure, not a fall-through to the prompt.**
/// Somebody set that variable on purpose; carrying on to ask a question that a service has
/// nobody to answer is how rule 4 gets broken by an accident of permissions.
///
/// **The line ending comes off, and that is not cosmetic.** A passphrase file written by an
/// editor ends in a newline, and on Windows in CRLF — the same carriage return this project
/// already strips out of a config file, because the authentication failure it causes is
/// invisible.
fn from_the_file() -> Outcome<Option<Zeroizing<String>>> {
    let Some(named) = std::env::var_os(PASSPHRASE_FILE_VAR).filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let path = std::path::PathBuf::from(named);
    let held = Zeroizing::new(std::fs::read_to_string(&path).map_err(|error| {
        Failure::new(
            Exit::Usage,
            format!(
                "{PASSPHRASE_FILE_VAR} names {} and it could not be read: {error}",
                path.display()
            ),
        )
        .hint(
            "that file holds the passphrase for the encrypted password store. It has to be \
             readable by the account this is running as, and by nobody else.",
        )
    })?);

    let trimmed = Zeroizing::new(held.trim_end_matches(['\r', '\n']).to_owned());
    if trimmed.is_empty() {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "{PASSPHRASE_FILE_VAR} names {}, which is empty",
                path.display()
            ),
        )
        .hint("an empty passphrase encrypts nothing"));
    }

    Ok(Some(trimmed))
}

#[cfg(test)]
pub(crate) fn passphrase_without_a_terminal() -> Outcome<Zeroizing<String>> {
    passphrase(false)
}

#[cfg(test)]
pub(crate) fn round_trip(
    entries: &[(String, Secret)],
    passphrase: &str,
) -> Outcome<Vec<(String, Secret)>> {
    let phrase = Zeroizing::new(passphrase.to_owned());
    let sealed = seal(&encode(entries), &phrase)?;
    let opened = open(&sealed, &phrase)?;
    self::entries(&opened)
}

#[cfg(test)]
pub(crate) fn seal_for_test(entries: &[(String, Secret)], passphrase: &str) -> Outcome<Vec<u8>> {
    seal(&encode(entries), &Zeroizing::new(passphrase.to_owned()))
}

#[cfg(test)]
pub(crate) fn open_for_test(bytes: &[u8], passphrase: &str) -> Outcome<Vec<(String, Secret)>> {
    let opened = open(bytes, &Zeroizing::new(passphrase.to_owned()))?;
    entries(&opened)
}

#[cfg(test)]
pub(crate) const fn header_len() -> usize {
    HEADER_LEN
}
