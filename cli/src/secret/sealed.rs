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
const PASSPHRASE_VAR: &str = "SLOOP_PASSPHRASE";

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

/// Read the password stored under `key`.
pub fn get(path: &Path, key: &str) -> Outcome<Secret> {
    if !path.is_file() {
        return Err(Failure::new(
            Exit::Usage,
            format!("there is no encrypted password file at {}", path.display()),
        )
        .hint("run `sloop db add` and choose the encrypted file, or use another route"));
    }

    let bytes = std::fs::read(path).map_err(|error| {
        Failure::new(
            Exit::Usage,
            format!("could not read {}: {error}", path.display()),
        )
    })?;
    let passphrase = passphrase(false)?;
    let plaintext = open(&bytes, &passphrase)?;

    entries(&plaintext)?
        .into_iter()
        .find(|(name, _)| name == key)
        .map(|(_, secret)| secret)
        .ok_or_else(|| {
            Failure::new(
                Exit::Usage,
                format!("the encrypted file has no password for {key}"),
            )
            .hint("run `sloop db add` for it")
        })
}

/// Store a password under `key`, rewriting the file.
///
/// Waits for R7's `db add` to call it. Present now because R3 has to prove the round trip.
#[allow(dead_code)]
pub fn put(path: &Path, key: &str, secret: &Secret) -> Outcome<()> {
    let passphrase = passphrase(true)?;

    let mut stored: Vec<(String, Secret)> = if path.is_file() {
        let bytes = std::fs::read(path).map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("could not read {}: {error}", path.display()),
            )
        })?;
        entries(&open(&bytes, &passphrase)?)?
    } else {
        Vec::new()
    };

    stored.retain(|(name, _)| name != key);
    stored.push((key.to_owned(), secret.clone()));

    let plaintext = encode(&stored);
    let sealed = seal(&plaintext, &passphrase)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("could not create {}: {error}", parent.display()),
            )
        })?;
    }
    std::fs::write(path, sealed).map_err(|error| {
        Failure::new(
            Exit::Usage,
            format!("could not write {}: {error}", path.display()),
        )
    })
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
    if let Ok(value) = std::env::var(PASSPHRASE_VAR) {
        if !value.is_empty() {
            return Ok(Zeroizing::new(value));
        }
    }

    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            "the encrypted password file needs a passphrase, and there is no terminal to ask at",
        )
        .hint(format!("set {PASSPHRASE_VAR} for unattended runs")));
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
