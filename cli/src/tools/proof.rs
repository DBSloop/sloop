//! How a downloaded archive is proved to be the one that was asked for.
//!
//! **Three engines, three answers, because the three projects publish three different
//! things.** This is not a preference — it is what is actually on the other end, checked
//! against the real endpoints:
//!
//! ```text
//! PostgreSQL   EnterpriseDB publishes nothing beside the Windows archive, so the SHA-256
//!              is one sloop holds itself. `releases::PINNED`, and `ci/pinned-releases.sh`
//!              re-downloads every row to catch a hash that moved.
//! MariaDB      downloads.mariadb.org's REST API gives a sha256sum per file, so the version
//!              really is resolved at request time and really is verified.
//! MySQL        Oracle publishes MD5 on an HTML page and a detached GPG signature. The
//!              signature is what sloop checks.
//! ```
//!
//! **Why not the MD5.** MD5 is broken for collisions, so checking it can honestly say *"the
//! file arrived intact"* and cannot say *"the file was not tampered with"*. `releases`
//! already refused to print a sentence with nothing behind it, about this exact trade, and
//! this is the same refusal one engine along.
//!
//! **Nothing here links a crypto library for the signature.** `gpg` is a program, shelled out
//! to exactly as `curl` is for the download itself — so the graph is unchanged and the claim
//! survives `cargo tree`. A machine with no `gpg` is told so by name rather than quietly
//! dropped to something weaker: what is being protected here is the guarantee, and rule 0d
//! does not license weakening that.

#[cfg(test)]
#[path = "proof_tests.rs"]
mod tests;

use std::path::Path;
use std::process::{Command, Stdio};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// One public key sloop will accept a signature from.
///
/// **The fingerprint and the key together, because either alone is useless.** The key is what
/// `gpg` needs in order to check anything at all; the fingerprint is what says *which* key,
/// so that a good signature by somebody else's key is refused exactly as a wrong hash is. A
/// test asks `gpg` for each carried key's own fingerprint and fails if the two have drifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    /// The long fingerprint, as `gpg` prints it with the spaces taken out.
    pub fingerprint: &'static str,
    /// Whose it is, for the sentence that says what was checked against what.
    pub named: &'static str,
    /// The key itself, ASCII-armoured, as the publisher publishes it.
    pub armored: &'static str,
}

/// What sloop will accept as proof that an archive is the one it asked for.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proof {
    /// A SHA-256 this build of sloop carries, with the exact size beside it.
    ///
    /// The size is checked first and it is not decoration: a redirect to an error page that
    /// arrived with a `200` is caught by the length before a byte is hashed.
    Pinned {
        /// Exact length of the archive.
        bytes: u64,
        /// Lower-case hex SHA-256.
        sha256: String,
    },
    /// A SHA-256 the publisher gives for that exact file.
    Published {
        /// Lower-case hex SHA-256, as the publisher wrote it.
        sha256: String,
        /// Where it came from, for the sentence that says what was checked against what.
        from: String,
    },
    /// A detached GPG signature the publisher gives beside the archive.
    Signed {
        /// Where the `.asc` is.
        signature_url: String,
        /// Whose signature it may be.
        ///
        /// **A list because publishers rotate keys**, and an archive signed with last year's
        /// is still that publisher's archive. Any one of them is enough and **nothing else
        /// is**: a good signature by a key that is not on this list is refused exactly as a
        /// wrong hash is, which is the whole attack this prevents.
        keys: &'static [Key],
    },
}

impl Proof {
    /// What this proof is, in the words the run prints as it happens.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Pinned { .. } => "the SHA-256 this sloop was built with".to_owned(),
            Self::Published { from, .. } => format!("the SHA-256 {from} publishes"),
            Self::Signed { keys, .. } => format!(
                "{}'s GPG signature",
                keys.first().map_or("the publisher", |key| key.named)
            ),
        }
    }

    /// Whether this machine can check this proof at all.
    ///
    /// Asked **before** the download, not after: a machine that cannot verify a MySQL archive
    /// should be told before it spends four hundred megabytes finding out.
    #[must_use]
    pub fn checkable_here(&self) -> bool {
        match self {
            Self::Pinned { .. } | Self::Published { .. } => true,
            Self::Signed { .. } => gpg().is_some(),
        }
    }
}

/// The one refusal for a machine with no `gpg`, so both places say it the same way.
fn not_checkable_here(what: &str) -> Failure {
    Failure::new(
        Exit::Usage,
        format!(
            "{what} ships a GPG signature and no checksum, and this machine has no gpg to \
             check it with"
        ),
    )
    .hint(
        "install gpg — `winget install GnuPG.GnuPG`, `apt install gnupg`, `brew install \
         gnupg` — or install that server yourself. sloop will not run an archive it cannot \
         prove it received intact",
    )
}

/// Check an archive against its proof, and refuse if it does not hold.
///
/// **Nothing is unpacked before this returns `Ok`.** An archive that fails is left on disk
/// for somebody to look at and never handed to an extractor, which is the one ordering that
/// matters in this whole module.
pub fn holds(archive: &Path, proof: &Proof, what: &str) -> Outcome<()> {
    match proof {
        Proof::Pinned { bytes, sha256 } => {
            let found = size_of(archive)?;
            if found != *bytes {
                return Err(wrong(
                    what,
                    &format!("it is {found} bytes and should be {bytes}"),
                ));
            }
            compare(archive, sha256, what)
        }
        Proof::Published { sha256, from } => {
            if !is_hex_sha256(sha256) {
                return Err(wrong(
                    what,
                    &format!("{from} gave something that is not a SHA-256"),
                ));
            }
            compare(archive, sha256, what)
        }
        Proof::Signed {
            signature_url,
            keys,
        } => signature_holds(archive, signature_url, keys, what),
    }
}

/// Hash the file and compare, case-insensitively on the hex and nothing else.
fn compare(archive: &Path, expected: &str, what: &str) -> Outcome<()> {
    let found = super::acquire::sha256_of(archive)?;
    if found.eq_ignore_ascii_case(expected) {
        return Ok(());
    }

    Err(wrong(
        what,
        &format!("it hashes to {found} and should be {expected}"),
    ))
}

/// Is this a lower- or upper-case hex SHA-256 and nothing else?
#[must_use]
pub fn is_hex_sha256(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// The size of a file that is supposed to be there.
fn size_of(archive: &Path) -> Outcome<u64> {
    std::fs::metadata(archive)
        .map(|found| found.len())
        .map_err(|error| Failure::usage(format!("could not read {}: {error}", archive.display())))
}

/// Check a detached signature with the system `gpg`, against sloop's own keyring and nobody
/// else's.
///
/// **A good signature is not enough; it has to be the right key.** `gpg --verify` exits zero
/// for any signature it can check, including one made by a key somebody added five minutes
/// ago — so the keyring it checks against is built here, from the keys this build carries,
/// and the fingerprint is still required to appear in what it said.
///
/// **In a directory of its own, thrown away afterwards.** The first real run of this created
/// `~/.gnupg` on a machine that had never used `gpg`, which is sloop reaching into somebody's
/// home directory to do its own bookkeeping — and worse, it would mean the check depended on
/// whatever else was in there. `--homedir` on a temporary directory makes the keyring exactly
/// the carried keys and nothing else, which is the property the refusal below rests on.
fn signature_holds(archive: &Path, signature_url: &str, keys: &[Key], what: &str) -> Outcome<()> {
    let Some(gpg) = gpg() else {
        // The same sentence `checkable_here` lets a caller avoid reaching this at all. Said
        // again here because this is reachable on its own, and a caller that forgot to ask
        // should get the useful refusal rather than a confusing one.
        return Err(not_checkable_here(what));
    };

    let signature = archive.with_extension("asc");
    super::acquire::download(signature_url, &signature)?;

    let keyring = Keyring::beside(archive, &gpg, keys)?;
    let said = keyring.verify(&gpg, &signature, archive)?;

    if !said.ok {
        return Err(wrong(what, "gpg would not verify it").hint(format!(
            "gpg said: {}\n\nNothing has been unpacked and nothing has been run",
            said.told.trim()
        )));
    }

    // **One of the listed keys, not just *a* key.** The keyring holds only these, so this is
    // belt and braces — and it is cheap belt and braces against the day somebody adds a key
    // to the ring for a different reason.
    if keys.iter().any(|key| mentions(&said.told, key.fingerprint)) {
        return Ok(());
    }

    let expected = keys
        .iter()
        .map(|key| format!("{} ({})", key.named, key.fingerprint))
        .collect::<Vec<_>>()
        .join(", or ");

    Err(
        wrong(what, &format!("it is signed, but not by {expected}")).hint(format!(
            "gpg said: {}\n\nA good signature by the wrong key is exactly what this check \
             exists to catch. Nothing has been unpacked",
            said.told.trim()
        )),
    )
}

/// A `gpg` home directory holding exactly the keys this build carries, deleted on the way out.
///
/// **It sits beside the archive, and that is a path-handling decision rather than a tidiness
/// one.** The `gpg` on a Windows machine is as likely to be the MSYS build that Git for
/// Windows ships as a native one, and the MSYS build does not read a `C:\` path as absolute —
/// it prepends the working directory and then cannot find its own keyring. The first real run
/// of this failed exactly there. Keeping the keyring next to the archive means every path
/// handed to `gpg` is a bare name in its own working directory, which no build of it can
/// misread.
struct Keyring {
    /// The directory `gpg` is run from: the one holding the archive.
    working_in: std::path::PathBuf,
    /// The keyring directory's name inside it.
    named: String,
}

/// What `gpg` said, and whether it was happy.
struct Said {
    ok: bool,
    told: String,
}

impl Keyring {
    /// Make one beside `archive` and import every carried key into it.
    fn beside(archive: &Path, gpg: &Path, keys: &[Key]) -> Outcome<Self> {
        let working_in = archive
            .parent()
            .ok_or_else(|| Failure::usage("the archive is not in a directory"))?
            .to_path_buf();
        // **Short, and that is a macOS constraint rather than a preference.** A unix socket
        // path is capped at about 104 bytes there, and `gpg` puts its agent's socket inside
        // its home directory — so a long name plus a long temporary directory is a keyring
        // `gpg` cannot open. `server::make::no_unix_socket` documents the same cap for
        // PostgreSQL, which is where this was learned the first time.
        let named = format!("sloop-gpg-{}", std::process::id());

        let home = working_in.join(&named);
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).map_err(|error| {
            Failure::usage(format!("could not create {}: {error}", home.display()))
        })?;

        // `gpg` refuses a home directory anyone else can read, on the platforms that have
        // such a notion. Best effort rather than required: it is a directory holding public
        // keys for a few seconds.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700));
        }

        let keyring = Self { working_in, named };

        for key in keys {
            let file = format!("{}.asc", key.fingerprint);
            std::fs::write(home.join(&file), key.armored)
                .map_err(|error| Failure::usage(format!("could not write {file}: {error}")))?;

            keyring.run(gpg, &["--import", &format!("{}/{file}", keyring.named)])?;
        }

        // **What the keyring holds, not what `gpg` exited with.** On macOS the import prints
        // `public key … imported` and *then* exits non-zero because it could not reach an
        // agent it does not need — so a status check refuses a keyring that is perfectly
        // correct. Asking what is in it is both more robust and a stronger check: the ring is
        // proved to hold exactly the carried keys before any signature is checked against it.
        let holding = keyring.fingerprints(gpg)?;
        for key in keys {
            if !holding.iter().any(|held| held == key.fingerprint) {
                return Err(Failure::new(
                    Exit::Failure,
                    format!("the {} key this sloop carries would not import", key.named),
                )
                .hint(format!(
                    "gpg read {holding:?} out of the keyring, and {} is not among them",
                    key.fingerprint
                )));
            }
        }

        Ok(keyring)
    }

    /// Every fingerprint the keyring holds, as `gpg` reads them back.
    fn fingerprints(&self, gpg: &Path) -> Outcome<Vec<String>> {
        let said = self.run(gpg, &["--list-keys", "--with-colons"])?;

        Ok(said
            .told
            .lines()
            .filter_map(|line| line.strip_prefix("fpr:"))
            .filter_map(|rest| rest.split(':').find(|field| !field.is_empty()))
            .map(ToOwned::to_owned)
            .collect())
    }

    /// Check a detached signature against it.
    ///
    /// Both files by name, because both are in the directory `gpg` is being run from: the
    /// signature was downloaded beside the archive, and the archive is what this is about.
    fn verify(&self, gpg: &Path, signature: &Path, archive: &Path) -> Outcome<Said> {
        let name = |path: &Path| {
            path.file_name()
                .map(|named| named.to_string_lossy().into_owned())
                .ok_or_else(|| Failure::usage("that is not a file name"))
        };

        self.run(gpg, &["--verify", &name(signature)?, &name(archive)?])
    }

    /// Run `gpg` against this keyring and nothing else.
    fn run(&self, gpg: &Path, arguments: &[&str]) -> Outcome<Said> {
        let said = Command::new(gpg)
            .current_dir(&self.working_in)
            .arg("--homedir")
            .arg(&self.named)
            .arg("--batch")
            // **No agent.** Importing a public key and checking a detached signature need no
            // secret key, so there is nothing for `gpg-agent` to do — and on macOS reaching
            // for one is what breaks this, because the socket it would open lives inside the
            // home directory and the path runs past the platform's cap.
            .arg("--no-autostart")
            // No keyserver, no auto-retrieval, no network: the keys are the ones carried and
            // there is no path by which this reaches for another.
            .args(["--keyserver-options", "no-auto-key-retrieve"])
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| Failure::usage(format!("could not run gpg: {error}")))?;

        // `gpg` says everything on stderr, success included.
        Ok(Said {
            ok: said.status.success(),
            told: format!(
                "{}{}",
                String::from_utf8_lossy(&said.stdout),
                String::from_utf8_lossy(&said.stderr)
            ),
        })
    }
}

impl Drop for Keyring {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.working_in.join(&self.named));
    }
}

/// Does what `gpg` said name this fingerprint, however it spaced it?
///
/// **The whitespace is taken out of what it said rather than put into the fingerprint**, and
/// that is not the same thing. An earlier version built one spaced spelling — four-character
/// groups, single spaces — and `gpg` prints a *wider* gap in the middle of
/// `BCA4 3417 C3B4 85DD 128E  C6D4 …`, so the one spelling it looked for was not one `gpg`
/// produces. A build that printed only that form would have had a good signature by exactly
/// the right key refused. Removing the spacing matches every form at once.
fn mentions(told: &str, key: &str) -> bool {
    if told.contains(key) {
        return true;
    }

    let run_together: String = told
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    run_together.contains(key)
}

/// Where the system's `gpg` is, if it has one.
///
/// `gpg` and then `gpg2`, because a few distributions still ship the second name only.
#[must_use]
pub fn gpg() -> Option<std::path::PathBuf> {
    for name in ["gpg", "gpg2"] {
        let program = format!("{name}{}", if cfg!(windows) { ".exe" } else { "" });
        if let Some(found) = super::acquire::on_path_at(&program) {
            return Some(found);
        }
    }
    None
}

/// The one failure this module produces, so every refusal reads the same way.
fn wrong(what: &str, why: &str) -> Failure {
    Failure::new(
        Exit::Failure,
        format!("the {what} that arrived is not the one sloop asked for: {why}"),
    )
    .hint("nothing has been unpacked and nothing has been run")
}
