//! Backup encryption: an `age` keypair, and the asymmetry that makes it work unattended.
//!
//! **The public key sits in the registry; the private key does not.** That is the whole
//! design. A scheduled backup needs the public key and nothing else — no passphrase, no
//! keyring unlock, no secret of any kind in the environment — so encryption costs a cron
//! job nothing. Only a restore needs the private key, and a restore has a person attached
//! to it.
//!
//! **`age`, and the real implementation of it.** `dump.age` is an actual age file: anybody
//! holding the private key can read it with the standard `age` tool, on a machine that has
//! never heard of sloop, in ten years. That escape hatch is the reason the format was
//! chosen, and it is a property of the *file* — which is why this module drives the
//! reference implementation rather than composing the format out of primitives. The cost is
//! a longer dependency graph; the alternative was hand-writing the one thing in this tool
//! whose silent failure mode is a backup nobody can open.
//!
//! **The private key is stored the same two ways a password is** — the OS keyring, or the
//! Argon2id-encrypted file on a machine with no keyring — and it is filed under the public
//! key rather than under a path. A project that gets moved, renamed or copied still finds
//! its own key, and a registry carried to another machine says exactly which key is missing.
//!
//! **Nothing here writes a secret anywhere a person did not ask for it.** The private key
//! leaves this module in two places only: into the keyring or the sealed file, and out of
//! `sloop key export`, which is a person asking for it by name.

#[cfg(test)]
mod tests;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use age::secrecy::ExposeSecret as _;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::{self, Lookup, Route, Secret};

/// The extension an encrypted dump carries.
///
/// `dump.age` rather than `dump.enc`: the name says which format it is, and that is the
/// difference between a file somebody can open and a file somebody has to guess at.
pub const SUFFIX: &str = "age";

/// The public half. Safe to print, safe to commit, safe to lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey(age::x25519::Recipient);

impl PublicKey {
    /// Read one as it is written down: `age1` and fifty-odd characters.
    pub fn parse(text: &str) -> Outcome<Self> {
        age::x25519::Recipient::from_str(text.trim())
            .map(Self)
            .map_err(|why| {
                Failure::usage(format!("{text} is not an age public key: {why}"))
                    .hint("it is the `age1…` half — `sloop key export` prints the other one")
            })
    }
}

impl std::fmt::Display for PublicKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The half that can read a backup, and the half whose loss is unrecoverable.
///
/// No `Display` and no `Debug` that says anything, for the same reason [`Secret`] has
/// neither: a key that can be formatted is a key that reaches a log eventually.
#[derive(Clone)]
pub struct PrivateKey(age::x25519::Identity);

impl PrivateKey {
    /// A new keypair, from the operating system's randomness.
    #[must_use]
    pub fn generate() -> Self {
        Self(age::x25519::Identity::generate())
    }

    /// Read one back: `AGE-SECRET-KEY-1…`.
    pub fn parse(text: &str) -> Outcome<Self> {
        age::x25519::Identity::from_str(text.trim())
            .map(Self)
            .map_err(|why| {
                // The text itself is never quoted back — it is the secret.
                Failure::usage(format!("that is not an age private key: {why}"))
                    .hint("it is the `AGE-SECRET-KEY-1…` line that `sloop key export` printed")
            })
    }

    /// The public half, derived rather than stored.
    #[must_use]
    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.to_public())
    }

    /// The key as it is written down, wrapped so it cannot be printed by accident.
    #[must_use]
    pub fn secret(&self) -> Secret {
        Secret::new(self.0.to_string().expose_secret().to_owned())
    }
}

impl std::fmt::Debug for PrivateKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PrivateKey(<redacted>)")
    }
}

/// Where a private key is kept, and where to look for it.
pub struct Store<'a> {
    /// The keyring, or the Argon2id file. Nothing else: the other two password routes fetch
    /// a secret somebody else manages, and this is a secret sloop generated.
    pub route: &'a Route,
    /// The encrypted store belonging to the registry that names the key — a row in
    /// sloop's own database since `R19c4`, a file for the two passwords that open it.
    pub vault: &'a secret::sealed::Vault<'a>,
}

impl Store<'_> {
    /// What the key is filed under.
    ///
    /// The public key, not a path. A project that moves keeps its key; a registry copied to
    /// another machine names exactly which key is missing there.
    fn key_for(public: &PublicKey) -> String {
        format!("backup-key:{public}")
    }

    /// Put a private key away.
    pub fn keep(&self, private: &PrivateKey) -> Outcome<()> {
        let name = Self::key_for(&private.public());
        let secret = private.secret();

        match self.route {
            Route::Keyring => secret::os_keyring::set(&name, &secret),
            Route::EncryptedFile => secret::sealed::put(self.vault, &name, &secret),
            other => Err(unsupported(other)),
        }
    }

    /// Fetch the private key for a public one.
    pub fn fetch(&self, public: &PublicKey) -> Outcome<PrivateKey> {
        let name = Self::key_for(public);

        let secret = match self.route {
            Route::Keyring | Route::EncryptedFile => secret::resolve(
                self.route,
                &Lookup {
                    key: &name,
                    vault: self.vault,
                },
            )
            .map_err(|failure| {
                failure.hint(format!(
                    "this backup was encrypted to {public}, and that key is not on this \
                     machine. `sloop key import` takes the one you exported"
                ))
            })?,
            other => return Err(unsupported(other)),
        };

        let private = PrivateKey::parse(secret.secret.expose())?;
        if private.public() != *public {
            return Err(Failure::new(
                Exit::Usage,
                format!("the stored key is not the one {public} needs"),
            )
            .hint("`sloop key import` replaces it with the right one"));
        }

        Ok(private)
    }
}

/// Put a new key wherever this machine can keep one.
///
/// **`SLOOP_PASSPHRASE` decides, where it is set.** That variable exists for one purpose —
/// the Argon2id file — so a machine that has it has already said where it keeps secrets, and
/// putting the key in a keyring it deliberately does not use would be ignoring the answer.
/// Everywhere else the keyring comes first, because it needs no configuration and is there
/// on every desktop.
///
/// **A machine that can keep neither is not a failure, it is a machine without encryption.**
/// The caller says so and takes a plain dump, because a backup that did not happen is worse
/// than a backup that is not encrypted — see `commands::backup`.
pub fn keep_somewhere(private: &PrivateKey, vault: &secret::sealed::Vault<'_>) -> Outcome<Route> {
    let mut first = None;

    for route in preference() {
        let store = Store {
            route: &route,
            vault,
        };
        match store.keep(private) {
            Ok(()) => return Ok(route),
            Err(failure) => first = first.or(Some(failure)),
        }
    }

    Err(first
        .unwrap_or_else(|| Failure::usage("there is nowhere on this machine to keep a backup key")))
}

/// The two storage routes, in the order this machine should be asked about them.
fn preference() -> [Route; 2] {
    secret::preferred_routes()
}

/// A route that cannot hold a key sloop generated.
fn unsupported(route: &Route) -> Failure {
    Failure::usage(format!(
        "a backup key cannot live in {} — that route fetches a secret something else keeps",
        route.describe()
    ))
    .hint("keyring, or encrypted-file on a machine with no keyring")
}

/// Write an encrypted file, with `fill` producing the plaintext.
///
/// **A closure rather than a writer, because the format has to be finished.** An age file
/// ends with a tag over its last chunk; a writer dropped without being closed leaves a file
/// that decrypts to nothing and says nothing about why. Handing the plaintext side to a
/// closure means the finishing cannot be forgotten.
///
/// The file is removed if anything fails, because a half-written encrypted dump is worse
/// than none: it is unreadable *and* it looks like a backup.
pub fn sealed_to<F>(path: &Path, recipient: &PublicKey, fill: F) -> Outcome<()>
where
    F: FnOnce(&mut dyn Write) -> Outcome<()>,
{
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            Failure::new(
                Exit::Dump,
                format!("could not create {}: {error}", parent.display()),
            )
        })?;
    }

    let existed = path.exists();
    let outcome = write_sealed(path, recipient, fill);

    if outcome.is_err() && !existed {
        let _ = std::fs::remove_file(path);
    }
    outcome
}

fn write_sealed<F>(path: &Path, recipient: &PublicKey, fill: F) -> Outcome<()>
where
    F: FnOnce(&mut dyn Write) -> Outcome<()>,
{
    let failed = |what: &str, why: &dyn std::fmt::Display| {
        Failure::new(Exit::Dump, format!("{what} {}: {why}", path.display()))
    };

    let handle = std::fs::File::create(path).map_err(|error| failed("could not create", &error))?;
    let out = std::io::BufWriter::with_capacity(256 * 1024, handle);

    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient.0 as &dyn age::Recipient))
            .map_err(|error| failed("could not encrypt to the key in the registry for", &error))?;

    let mut sealing = encryptor
        .wrap_output(out)
        .map_err(|error| failed("could not start writing", &error))?;

    fill(&mut sealing)?;

    // The last chunk's tag, and then the buffer underneath it. Both, in that order, or the
    // file is truncated in a way only a decryption attempt would notice.
    let mut out = sealing
        .finish()
        .map_err(|error| failed("could not finish encrypting", &error))?;
    out.flush()
        .map_err(|error| failed("could not finish writing", &error))
}

/// Read an encrypted file back.
///
/// The failures are sentences rather than an `age` error dumped on the floor: which key the
/// file wants, that the key on this machine is not it, or that the file is not an age file
/// at all. `R13`'s `restore` is what shows them to somebody; today the tests are what read
/// an encrypted dump back, which is how the round trip stays checked rather than assumed.
#[allow(dead_code)]
pub fn opened(path: &Path, private: &PrivateKey) -> Outcome<Box<dyn Read>> {
    let file = std::fs::File::open(path).map_err(|error| {
        Failure::new(
            Exit::Restore,
            format!("could not read {}: {error}", path.display()),
        )
    })?;

    let decryptor = age::Decryptor::new(std::io::BufReader::with_capacity(256 * 1024, file))
        .map_err(|error| not_readable(path, &error))?;

    let reader = decryptor
        .decrypt(std::iter::once(&private.0 as &dyn age::Identity))
        .map_err(|error| not_readable(path, &error))?;

    Ok(Box::new(reader))
}

/// Say what is wrong with an encrypted file in a sentence somebody can act on.
fn not_readable(path: &Path, error: &age::DecryptError) -> Failure {
    let (what, hint) = match error {
        age::DecryptError::NoMatchingKeys => (
            "no key on this machine can open it".to_owned(),
            "it was encrypted to a different key. `sloop key import` takes the one that \
             belongs to it, and the manifest beside the dump names which that is",
        ),
        age::DecryptError::InvalidHeader | age::DecryptError::UnknownFormat => (
            "it is not an age file".to_owned(),
            "an encrypted dump starts with `age-encryption.org/v1`. A dump this short or \
             this old may be an unencrypted one",
        ),
        age::DecryptError::DecryptionFailed => (
            "the file will not decrypt".to_owned(),
            "the bytes have changed since it was written — `sloop backups list` checks the \
             checksum in the manifest",
        ),
        other => (other.to_string(), "the file is not readable as it stands"),
    };

    Failure::new(Exit::Restore, format!("{}: {what}", path.display())).hint(hint)
}

/// The path a dump takes when it is encrypted.
#[must_use]
pub fn sealed_name(plain: &Path) -> PathBuf {
    let mut name = plain.as_os_str().to_owned();
    name.push(".");
    name.push(SUFFIX);
    PathBuf::from(name)
}
