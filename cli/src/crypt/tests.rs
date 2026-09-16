//! The keypair and the file format, with no database and no keyring involved.
//!
//! **Nothing here touches the machine's credential store.** Key storage is exercised
//! through the real binary in `tests/key.rs`, where a sandbox and `SLOOP_PASSPHRASE` keep
//! it inside a temporary directory — a test that wrote to the developer's own Credential
//! Manager would be the defect this project already fixed once.

use std::io::Read as _;

use super::{PrivateKey, PublicKey, opened, sealed_name, sealed_to};

/// A temporary directory of this test's own.
fn scratch(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sloop-crypt-{}-{label}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&path).expect("a temporary directory");
    path
}

/// A key written down and read back is the same key, and the halves belong together.
#[test]
fn a_keypair_survives_being_written_down() {
    let private = PrivateKey::generate();
    let public = private.public();

    let written = private.secret();
    assert!(
        written.expose().starts_with("AGE-SECRET-KEY-1"),
        "an exported key has to be one age itself would take"
    );
    assert!(
        public.to_string().starts_with("age1"),
        "{public} is not an age recipient"
    );

    let back = PrivateKey::parse(written.expose()).expect("reading the key back");
    assert_eq!(back.public(), public, "the halves came apart");
    assert_eq!(
        PublicKey::parse(&public.to_string()).expect("reading the public half"),
        public
    );

    // Two keys are two keys.
    assert_ne!(PrivateKey::generate().public(), public);
}

/// The private key never prints itself, the same rule `Secret` has.
#[test]
fn a_private_key_does_not_print_itself() {
    let private = PrivateKey::generate();
    let shown = format!("{private:?}");

    assert_eq!(shown, "PrivateKey(<redacted>)");
    assert!(
        !shown.contains("AGE-SECRET-KEY"),
        "Debug leaked the key: {shown}"
    );
}

/// What somebody typed wrong is refused with a sentence, and the refusal does not quote the
/// secret back at them.
#[test]
fn a_key_that_is_not_a_key_is_refused() {
    for attempt in [
        "",
        "hunter2",
        "age1",
        "AGE-SECRET-KEY-1",
        "AGE-SECRET-KEY-1NOTREALLYAKEYATALL",
    ] {
        let failure = PrivateKey::parse(attempt).expect_err("{attempt} was accepted");
        assert_eq!(failure.exit().code(), 2, "{attempt}");
        assert!(
            !failure.message().contains(attempt) || attempt.is_empty(),
            "the refusal quoted the key back: {}",
            failure.message()
        );
    }

    // The public half is refused just as clearly, and *is* quoted, because it is not secret.
    let failure = PublicKey::parse("not-a-key").expect_err("it is not a key");
    assert!(failure.message().contains("not-a-key"), "{failure:?}");
}

/// The round trip that a backup depends on: bytes in, an age file on disk, the same bytes
/// back out. Bigger than one chunk, because the format tags each chunk separately and a
/// single-chunk test would never exercise the boundary.
#[test]
fn a_sealed_file_comes_back_byte_for_byte() {
    let scratch = scratch("round-trip");
    let private = PrivateKey::generate();

    // Over 64 KiB, and deliberately not a multiple of it.
    let plain: Vec<u8> = (0..200_000_u32).map(|byte| (byte % 251) as u8).collect();
    let path = sealed_name(&scratch.join("dump"));

    sealed_to(&path, &private.public(), |sink| {
        sink.write_all(&plain).expect("writing the plaintext");
        Ok(())
    })
    .expect("sealing it");

    assert_eq!(
        path.file_name().map(std::ffi::OsStr::to_string_lossy),
        Some("dump.age".into())
    );

    // It is a real age file, which is the whole reason the format was chosen.
    let raw = std::fs::read(&path).expect("reading it back");
    assert!(
        raw.starts_with(b"age-encryption.org/v1\n"),
        "not an age file: {:?}",
        String::from_utf8_lossy(&raw[..raw.len().min(32)])
    );
    assert_ne!(raw, plain, "the file is not encrypted at all");

    let mut back = Vec::new();
    opened(&path, &private)
        .expect("opening it")
        .read_to_end(&mut back)
        .expect("reading it");
    assert_eq!(back, plain, "the plaintext came back changed");

    let _ = std::fs::remove_dir_all(&scratch);
}

/// The other key cannot read it, and is told why rather than handed an error type.
#[test]
fn another_key_cannot_open_it_and_the_message_says_what_to_do() {
    let scratch = scratch("wrong-key");
    let mine = PrivateKey::generate();
    let somebody_elses = PrivateKey::generate();
    let path = sealed_name(&scratch.join("dump"));

    sealed_to(&path, &mine.public(), |sink| {
        sink.write_all(b"the only copy of anything")
            .expect("writing");
        Ok(())
    })
    .expect("sealing it");

    let failure = opened(&path, &somebody_elses)
        .err()
        .expect("the wrong key opened it");
    assert_eq!(failure.exit().code(), 5, "a restore failure is code 5");
    assert!(
        failure
            .message()
            .contains("no key on this machine can open it"),
        "{failure:?}"
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("key import")),
        "the message has to say what to do: {failure:?}"
    );

    // And the right key still works, so the test above proves something.
    assert!(opened(&path, &mine).is_ok());

    let _ = std::fs::remove_dir_all(&scratch);
}

/// A plain dump handed to the decryptor explains itself instead of panicking, which is
/// exactly what `restore` will hand it the first time somebody points it at an old backup.
#[test]
fn a_file_that_is_not_encrypted_is_explained_rather_than_a_panic() {
    let scratch = scratch("plain");
    let path = scratch.join("dump");
    std::fs::write(&path, b"PGDMP\x00\x00 not an age file at all").expect("writing a dump");

    let failure = opened(&path, &PrivateKey::generate())
        .err()
        .expect("it is not an age file");
    assert_eq!(failure.exit().code(), 5);
    assert!(failure.message().contains("not an age file"), "{failure:?}");

    // And a file that is not there at all is its own sentence.
    let missing = opened(&scratch.join("nothing"), &PrivateKey::generate());
    let missing = missing.err().expect("there is no such file");
    assert_eq!(missing.exit().code(), 5);

    let _ = std::fs::remove_dir_all(&scratch);
}

/// A sealing that fails partway leaves nothing behind. A half-written encrypted dump is
/// unreadable *and* looks like a backup, which is the worst of both.
#[test]
fn a_sealing_that_fails_leaves_no_file() {
    let scratch = scratch("failed");
    let path = sealed_name(&scratch.join("dump"));

    let failure = sealed_to(&path, &PrivateKey::generate().public(), |_| {
        Err(crate::failure::Failure::new(
            crate::exit::Exit::Dump,
            "the dump program fell over",
        ))
    })
    .expect_err("the closure failed");

    assert_eq!(failure.exit().code(), 4);
    assert!(!path.exists(), "{} was left behind", path.display());

    let _ = std::fs::remove_dir_all(&scratch);
}
