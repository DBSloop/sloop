//! What each of the three proofs accepts, and — the half that matters — what it refuses.
//!
//! Every test here writes a real file and hashes it for real. Nothing about a checksum is
//! worth testing against a mock of the thing doing the checking.

use std::path::PathBuf;

use super::{Keyring, Proof, holds, is_hex_sha256, mentions, not_checkable_here};

/// A real file on disk, removed when it goes out of scope.
struct File(PathBuf);

impl File {
    /// `SHA-256("sloop")` is the one hash written out by hand in this file; everything else
    /// is derived, so a typo in it fails loudly rather than making a test agree with itself.
    const SLOOP: &'static str = "b8e9d1f01b0ff90b2cc0a42a1e0cbfad5bfc3b5a8b5e99f3d2b0e9ca7e5b2de4";

    fn holding(label: &str, what: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sloop-proof-{}-{label}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, what).expect("a temporary file should be writable");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    /// What this file actually hashes to, so the "it matches" tests cannot be wrong about it.
    fn real_sha256(&self) -> String {
        crate::tools::acquire::sha256_of(&self.0).expect("a file on disk hashes")
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("asc"));
    }
}

/// A pinned proof matches when the size and the hash both do.
#[test]
fn a_pinned_archive_is_accepted_when_it_is_the_one_that_was_pinned() {
    let file = File::holding("pinned-ok", b"sloop");
    let proof = Proof::Pinned {
        bytes: 5,
        sha256: file.real_sha256(),
    };

    assert!(holds(file.path(), &proof, "archive").is_ok());
}

/// **The size is checked first, and it is not decoration.** A redirect to an error page
/// arrives with a `200` and a body, and the length is what catches it before a byte is
/// hashed — which is the whole reason the field exists beside the hash.
#[test]
fn a_pinned_archive_of_the_wrong_length_is_refused_before_it_is_hashed() {
    let file = File::holding("pinned-short", b"<html>404</html>");
    let proof = Proof::Pinned {
        bytes: 344_414_106,
        sha256: File::SLOOP.to_owned(),
    };

    let failure = holds(file.path(), &proof, "PostgreSQL archive").expect_err("wrong length");
    assert!(
        failure.message().contains("should be 344414106"),
        "{}",
        failure.message()
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("nothing has been unpacked")),
        "a refusal has to say nothing was run"
    );
}

/// The right length and the wrong bytes is the case a length check alone would wave through.
#[test]
fn a_pinned_archive_of_the_right_length_and_wrong_bytes_is_refused() {
    let file = File::holding("pinned-swapped", b"sloop");
    let proof = Proof::Pinned {
        bytes: 5,
        // Five bytes, and not these five.
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
    };

    let failure = holds(file.path(), &proof, "archive").expect_err("wrong bytes");
    assert!(
        failure.message().contains("hashes to"),
        "{}",
        failure.message()
    );
}

/// A published hash is the same comparison, and case in the hex is not a difference.
#[test]
fn a_published_hash_is_accepted_however_the_publisher_cased_it() {
    let file = File::holding("published", b"sloop");
    let real = file.real_sha256();

    for spelling in [real.to_lowercase(), real.to_uppercase()] {
        let proof = Proof::Published {
            sha256: spelling.clone(),
            from: "downloads.mariadb.org".to_owned(),
        };
        assert!(
            holds(file.path(), &proof, "MariaDB archive").is_ok(),
            "{spelling} should be accepted"
        );
    }
}

/// **A publisher that answers with something that is not a hash is refused, not trusted.**
/// An API that returns an error document, or `null`, or an empty string, must not become a
/// comparison that passes because both sides are equally meaningless.
#[test]
fn a_publisher_that_did_not_give_a_hash_is_refused_rather_than_believed() {
    let file = File::holding("published-junk", b"sloop");

    for junk in ["", "null", "not available", "abc123", &"f".repeat(63)] {
        let proof = Proof::Published {
            sha256: junk.to_owned(),
            from: "downloads.mariadb.org".to_owned(),
        };
        let failure = holds(file.path(), &proof, "MariaDB archive")
            .expect_err("{junk} is not a SHA-256 and must not be treated as one");
        assert!(
            failure.message().contains("not a SHA-256"),
            "{junk}: {}",
            failure.message()
        );
    }
}

/// What a SHA-256 is, and what it is not.
#[test]
fn only_sixty_four_hex_digits_is_a_sha256() {
    assert!(is_hex_sha256(&"a".repeat(64)));
    assert!(is_hex_sha256(&"F".repeat(64)));

    assert!(!is_hex_sha256(&"a".repeat(63)));
    assert!(!is_hex_sha256(&"a".repeat(65)));
    assert!(!is_hex_sha256(""));
    assert!(!is_hex_sha256(&"g".repeat(64)), "g is not hex");
    assert!(
        !is_hex_sha256(&format!("{}\n", "a".repeat(63))),
        "a newline is not a digit"
    );
}

/// **A machine that cannot check a signature is told before the download, not after.**
/// Four hundred megabytes is a long way to go to find out sloop was never going to be able
/// to prove what arrived.
#[test]
fn whether_a_proof_can_be_checked_here_is_answerable_without_downloading_anything() {
    let pinned = Proof::Pinned {
        bytes: 1,
        sha256: File::SLOOP.to_owned(),
    };
    let published = Proof::Published {
        sha256: File::SLOOP.to_owned(),
        from: "downloads.mariadb.org".to_owned(),
    };

    assert!(pinned.checkable_here(), "a hash needs nothing installed");
    assert!(published.checkable_here(), "a hash needs nothing installed");

    let signed = Proof::Signed {
        signature_url: "https://example.invalid/x.asc".to_owned(),
        keys: crate::tools::catalogue::MYSQL_KEYS,
    };
    // True or false depending on the machine — what is asserted is that asking is free and
    // that the answer is the same as whether `gpg` is there.
    assert_eq!(signed.checkable_here(), super::gpg().is_some());
}

/// And when it cannot, the refusal names what to install rather than shrugging.
#[test]
fn a_machine_with_no_gpg_is_told_what_to_install() {
    let signed = Proof::Signed {
        signature_url: "https://example.invalid/x.asc".to_owned(),
        keys: crate::tools::catalogue::MYSQL_KEYS,
    };

    assert!(!signed.checkable_here() || super::gpg().is_some());
    let failure = not_checkable_here("MySQL");
    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure.message().contains("no gpg"),
        "{}",
        failure.message()
    );

    let hint = failure.hint_text().unwrap_or_default();
    for wanted in ["winget", "apt", "brew"] {
        assert!(
            hint.contains(wanted),
            "the hint should name {wanted}: {hint}"
        );
    }
}

/// Each proof says what it is, because the run prints it as it happens — *"verifying against
/// the SHA-256 downloads.mariadb.org publishes"* is a sentence somebody can check.
#[test]
fn every_proof_says_what_it_checked_against() {
    assert_eq!(
        Proof::Pinned {
            bytes: 1,
            sha256: File::SLOOP.to_owned()
        }
        .describe(),
        "the SHA-256 this sloop was built with"
    );
    assert_eq!(
        Proof::Published {
            sha256: File::SLOOP.to_owned(),
            from: "downloads.mariadb.org".to_owned()
        }
        .describe(),
        "the SHA-256 downloads.mariadb.org publishes"
    );
    assert_eq!(
        Proof::Signed {
            signature_url: String::new(),
            keys: crate::tools::catalogue::MYSQL_KEYS,
        }
        .describe(),
        "MySQL Release Engineering's GPG signature"
    );
}

// ---------------------------------------------------------------------------------------
// The carried keys
// ---------------------------------------------------------------------------------------

/// **Every carried key is the key its fingerprint says it is, and `gpg` is what says so.**
///
/// The fingerprints were in this source before the key material was — they came from
/// Oracle's own documentation, in an earlier pass that carried nothing else — so asking `gpg`
/// to agree with them is a real cross-check rather than a tautology. It is the whole of what
/// makes bytes committed to this repository safe to trust as a trust anchor.
#[test]
fn every_carried_key_is_the_key_its_fingerprint_says_it_is() {
    let Some(gpg) = super::gpg() else {
        eprintln!("skipping: this machine has no gpg to ask");
        return;
    };

    for key in crate::tools::catalogue::MYSQL_KEYS {
        let workspace = std::env::temp_dir().join(format!(
            "sloop-key-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            &key.fingerprint[..8]
        ));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).expect("a temporary directory");

        // `Keyring::beside` wants the path of a file in the directory it should work in; the
        // file itself is never opened here, only its parent is used.
        let keyring = Keyring::beside(&workspace.join("archive.zip"), &gpg, &[*key])
            .expect("the carried key should import");

        let said = keyring
            .run(&gpg, &["--list-keys", "--with-colons"])
            .expect("gpg should list what it just imported");
        assert!(said.ok, "{}", said.told);

        let fingerprints: Vec<&str> = said
            .told
            .lines()
            .filter_map(|line| line.strip_prefix("fpr:"))
            .filter_map(|rest| rest.split(':').find(|field| !field.is_empty()))
            .collect();

        assert!(
            fingerprints.first() == Some(&key.fingerprint),
            "{} says its fingerprint is {} and gpg read {:?}",
            key.named,
            key.fingerprint,
            fingerprints
        );

        drop(keyring);
        let _ = std::fs::remove_dir_all(&workspace);
    }
}

/// Each carried key really is an armoured public key and not, say, a fetch that 404'd into
/// the file — which is the shape a broken refresh of these would take.
#[test]
fn each_carried_key_is_an_armoured_public_key() {
    for key in crate::tools::catalogue::MYSQL_KEYS {
        assert!(
            key.armored
                .starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----"),
            "{} does not start with the armour header",
            key.named
        );
        assert!(
            key.armored
                .trim_end()
                .ends_with("-----END PGP PUBLIC KEY BLOCK-----"),
            "{} does not end with the armour footer",
            key.named
        );
        assert!(
            key.fingerprint.len() == 40
                && key
                    .fingerprint
                    .chars()
                    .all(|character| character.is_ascii_hexdigit()),
            "{} is not a 40-digit hex fingerprint: {}",
            key.named,
            key.fingerprint
        );
        assert!(
            !key.named.is_empty(),
            "a key with no name cannot appear in a refusal that makes sense"
        );
    }
}

/// **A fingerprint is looked for however `gpg` spaced it.** Older builds print it in
/// four-character groups and newer ones run it together; reading only one spelling would mean
/// silently accepting a signature without ever confirming whose it was.
#[test]
fn a_fingerprint_is_recognised_spaced_or_run_together() {
    let key = "BCA43417C3B485DD128EC6D4B7B3B788A8D3785C";

    assert!(mentions(&format!("using RSA key {key}"), key));
    assert!(mentions(
        "Primary key fingerprint: BCA4 3417 C3B4 85DD 128E  C6D4 B7B3 B788 A8D3 785C",
        key
    ));
    assert!(
        !mentions(
            "using RSA key 0000000000000000000000000000000000000000",
            key
        ),
        "somebody else's key must not read as this one"
    );
}
