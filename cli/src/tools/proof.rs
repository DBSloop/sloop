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

//! **Nothing in the binary reaches this module yet**, so the whole of it is allowed to sit
//! unread — the same way `secret` was allowed to in `R3`, and for the same reason. It is the
//! half of `R19d` that had to be settled before anything else could be built: which of three
//! things each publisher actually gives, and what sloop does when that is a signature rather
//! than a hash. The catalogue that picks a version and the screen that offers one are what
//! will call it. Until then its reader is `proof_tests`, which writes real files and hashes
//! them for real.
#![allow(dead_code)]

#[cfg(test)]
#[path = "proof_tests.rs"]
mod tests;

use std::path::Path;
use std::process::{Command, Stdio};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

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
        /// Whose signature it has to be, so a *valid* signature by somebody else is still a
        /// refusal. A good signature from the wrong key is the whole attack this prevents.
        key: &'static str,
        /// What that key is called, for the sentence.
        key_named: &'static str,
    },
}

impl Proof {
    /// What this proof is, in the words the run prints as it happens.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Pinned { .. } => "the SHA-256 this sloop was built with".to_owned(),
            Self::Published { from, .. } => format!("the SHA-256 {from} publishes"),
            Self::Signed { key_named, .. } => format!("{key_named}'s GPG signature"),
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
            key,
            key_named,
        } => signature_holds(archive, signature_url, key, key_named, what),
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

/// Check a detached signature with the system `gpg`.
///
/// **A good signature is not enough; it has to be the right key.** `gpg --verify` exits zero
/// for any signature it can check, including one made by a key somebody added five minutes
/// ago — so the long key id is required to appear in what it said, and a signature by
/// anybody else is refused exactly as a wrong hash is.
fn signature_holds(
    archive: &Path,
    signature_url: &str,
    key: &str,
    key_named: &str,
    what: &str,
) -> Outcome<()> {
    let Some(gpg) = gpg() else {
        // The same sentence `checkable_here` lets a caller avoid reaching this at all. Said
        // again here because this is reachable on its own, and a caller that forgot to ask
        // should get the useful refusal rather than a confusing one.
        return Err(not_checkable_here(what));
    };

    let signature = archive.with_extension("asc");
    super::acquire::download(signature_url, &signature)?;

    let said = Command::new(&gpg)
        .arg("--verify")
        .arg(&signature)
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| Failure::usage(format!("could not run gpg: {error}")))?;

    // `gpg` says everything on stderr, success included.
    let told = format!(
        "{}{}",
        String::from_utf8_lossy(&said.stdout),
        String::from_utf8_lossy(&said.stderr)
    );

    if !said.status.success() {
        return Err(wrong(what, "gpg would not verify it").hint(format!(
            "gpg said: {}\n\nThe archive is still at {} — it has not been unpacked",
            told.trim(),
            archive.display()
        )));
    }

    // The key, not just *a* key. `gpg` prints the long id with spaces on some versions and
    // without on others, so both spellings are looked for.
    let spaced = key
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ");

    if !told.contains(key) && !told.contains(&spaced) {
        return Err(wrong(
            what,
            &format!("it is signed, but not by {key_named} ({key})"),
        )
        .hint(format!(
            "gpg said: {}\n\nA good signature by the wrong key is exactly what this check \
             exists to catch. Nothing has been unpacked",
            told.trim()
        )));
    }

    Ok(())
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
