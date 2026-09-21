//! The four routes, each carrying the same deliberately awful password.
//!
//! The password below has a backslash, a redirect, a dollar, a hash, both kinds of quote,
//! a semicolon, a backtick, a pipe, a percent and a space in it. Every one of those means
//! something to a shell, to TOML, or to Windows `cmd`, and the whole point of this module
//! is that none of them means anything to sloop.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{Lookup, Route, Secret, resolve};

/// Everything that has ever broken a password field, in one string.
const NASTY: &str =
    r#"p@ss\word >out $HOME #hash "double" ;semi 'single' `tick` |pipe %PATH% &amp !bang end"#;

/// The same, with the trailing space that a copy-and-paste brings along.
const NASTY_TRAILING: &str = r#"p@ss\word "double" 'single' end "#;

fn vault(sealed: &Path) -> super::sealed::Vault<'_> {
    super::sealed::Vault::File(sealed)
}

fn lookup<'a>(vault: &'a super::sealed::Vault<'a>) -> Lookup<'a> {
    Lookup {
        key: "postgres://app@db.internal:5432/app",
        vault,
    }
}

// ---------------------------------------------------------------- the secret itself

#[test]
fn a_secret_never_prints_itself() {
    let secret = Secret::new(NASTY.to_owned());

    let shown = format!("{secret:?}");
    assert_eq!(shown, "Secret(<redacted>)");
    assert!(!shown.contains("p@ss"), "Debug leaked the password");

    // There is deliberately no `Display`, so `println!("{secret}")` does not compile and
    // a password cannot reach a log by someone reaching for the obvious thing.
}

#[test]
fn the_notes_describe_the_password_without_saying_it() {
    for value in [NASTY, NASTY_TRAILING, ""] {
        let secret = Secret::new(value.to_owned());
        for note in secret.notes() {
            // Every string contains the empty one, so only a real value is worth
            // searching for.
            if !value.is_empty() {
                assert!(!note.contains(value), "a note quoted the password: {note}");
            }
            assert!(!note.contains("p@ss"), "a note quoted the password: {note}");
        }
    }
}

#[test]
fn trailing_whitespace_is_reported_and_never_removed() {
    let kept = Secret::new(NASTY_TRAILING.to_owned());

    assert_eq!(kept.expose(), NASTY_TRAILING, "the space was stripped");
    assert!(
        kept.notes().iter().any(|note| note.contains("whitespace")),
        "nothing warned about the trailing space"
    );

    // A password with nothing odd about it gets no notes at all.
    assert!(Secret::new(NASTY.to_owned()).notes().is_empty());

    // Empty is worth saying out loud: it usually means a variable was not set.
    assert!(!Secret::new(String::new()).notes().is_empty());
}

// ---------------------------------------------------------------- routes as written

#[test]
fn every_route_survives_being_written_down_and_read_back() {
    let routes = [
        Route::Keyring,
        Route::EncryptedFile,
        Route::Environment("PGPASSWORD".to_owned()),
        Route::Command("op read op://vault/db/password".to_owned()),
    ];

    for route in routes {
        let written = route.as_field();
        assert_eq!(Route::parse(&written).unwrap(), route, "{written}");
    }
}

/// The rule that makes rule 3 enforceable: there is no way to spell a password that a
/// registry would accept.
#[test]
fn a_plaintext_password_is_refused_rather_than_stored() {
    for attempt in [
        NASTY,
        "hunter2",
        "",
        "correct horse battery staple",
        "$PGPASSWORD",
        "${}",
        "${TWO WORDS}",
        "keyring ",
        "command:",
        "file",
        "encrypted_file",
    ] {
        let failure = Route::parse(attempt).unwrap_err();
        assert_eq!(failure.exit().code(), 2, "{attempt:?} was accepted");
    }
}

#[test]
fn the_refusal_never_suggests_writing_a_password() {
    let failure = Route::parse("hunter2").unwrap_err();
    assert!(
        failure.message().contains("hunter2"),
        "it should name what it refused"
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("never written")),
        "the hint has to say why"
    );
}

#[test]
fn the_flag_outranks_whatever_the_registry_says() {
    let from_file = Route::Keyring;

    assert_eq!(
        from_file.overridden_by(Some("op read op://vault/db/password")),
        Route::Command("op read op://vault/db/password".to_owned())
    );
    // An empty flag is not a flag.
    assert_eq!(from_file.overridden_by(Some("")), Route::Keyring);
    assert_eq!(from_file.overridden_by(Some("   ")), Route::Keyring);
    assert_eq!(from_file.overridden_by(None), Route::Keyring);
}

// ---------------------------------------------------------------- the encrypted file

#[test]
fn a_nasty_password_round_trips_through_the_encrypted_file() {
    let entries = vec![
        ("one".to_owned(), Secret::new(NASTY.to_owned())),
        ("two".to_owned(), Secret::new(NASTY_TRAILING.to_owned())),
        ("empty".to_owned(), Secret::new(String::new())),
    ];

    let back = super::sealed::round_trip(&entries, "a passphrase with spaces").unwrap();

    assert_eq!(back.len(), 3);
    assert_eq!(back[0].1.expose(), NASTY);
    assert_eq!(
        back[1].1.expose(),
        NASTY_TRAILING,
        "the trailing space was lost"
    );
    assert_eq!(back[2].1.expose(), "");
}

#[test]
fn the_encrypted_file_gives_nothing_away_to_a_reader() {
    let entries = vec![("one".to_owned(), Secret::new(NASTY.to_owned()))];
    let sealed = super::sealed::seal_for_test(&entries, "passphrase").unwrap();

    // Neither the password nor the name it is filed under survives as readable bytes.
    assert!(
        !contains(&sealed, NASTY.as_bytes()),
        "the password is in the file in the clear"
    );
    assert!(
        !contains(&sealed, b"one"),
        "the key is in the file in the clear"
    );
}

#[test]
fn the_encrypted_file_refuses_the_wrong_passphrase() {
    let entries = vec![("one".to_owned(), Secret::new(NASTY.to_owned()))];
    let sealed = super::sealed::seal_for_test(&entries, "the right one").unwrap();

    let failure = super::sealed::open_for_test(&sealed, "the wrong one").unwrap_err();
    assert_eq!(failure.exit().code(), 2);
    assert!(!failure.message().contains("p@ss"));
}

/// The header is fed to the cipher as associated data, so weakening the Argon2 cost by
/// editing the file makes it refuse to open rather than open faster.
#[test]
fn the_encrypted_file_notices_an_edited_header() {
    let entries = vec![("one".to_owned(), Secret::new(NASTY.to_owned()))];
    let sealed = super::sealed::seal_for_test(&entries, "passphrase").unwrap();

    // The memory cost sits straight after the magic and the version byte.
    let mut tampered = sealed.clone();
    tampered[9] = tampered[9].wrapping_add(1);
    assert!(super::sealed::open_for_test(&tampered, "passphrase").is_err());

    // And so does a flipped bit anywhere in the ciphertext.
    let mut flipped = sealed;
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;
    assert!(super::sealed::open_for_test(&flipped, "passphrase").is_err());
}

/// Rule 4 where it actually bites, and `R32` in the same test on purpose.
///
/// **One test, because what it is asserting about is process-wide.** The passphrase a run
/// has already been given is remembered for that process, so a second test setting it would
/// decide whether the first one still sees a machine with nothing to go on. The two halves
/// have to run in this order, and the only way to guarantee an order is to be one test.
#[test]
fn it_asks_once_a_run_and_never_again() {
    assert!(
        std::env::var_os("SLOOP_PASSPHRASE").is_none(),
        "this test needs SLOOP_PASSPHRASE unset"
    );

    // Before anything has been asked: a test process has no terminal, which is exactly the
    // situation a scheduled job is in, so it names the variable rather than stopping.
    let failure = super::sealed::passphrase_without_a_terminal().unwrap_err();

    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("SLOOP_PASSPHRASE")),
        "it has to name the way out: {failure:?}"
    );

    // `R32`: once a run has been given one, every later lookup uses it. This is the whole
    // fix, and the shape of the bug is what it rules out -- `Store::open` unseals twice and
    // the menu opens the store more than once, which was seven prompts before one screen.
    super::sealed::remember_for_test("the one this run was given");

    let again = super::sealed::passphrase_without_a_terminal()
        .expect("a run that has already been asked does not need a terminal to be asked again");
    assert_eq!(&*again, "the one this run was given");
}

#[test]
fn asking_the_encrypted_file_for_a_file_that_is_not_there_says_which() {
    let missing = std::env::temp_dir().join("sloop-no-such-sealed-file");
    let failure = super::sealed::get(&vault(&missing), "anything").unwrap_err();

    assert_eq!(failure.exit().code(), 2);
    assert!(failure.message().contains("sloop-no-such-sealed-file"));
}

#[test]
fn something_that_is_not_one_of_our_files_says_so() {
    let failure =
        super::sealed::open_for_test(b"not a sloop file at all, really", "x").unwrap_err();
    assert!(failure.message().contains("not a sloop"));
    assert!(super::sealed::header_len() > 0);
}

// ---------------------------------------------------------------- the command

#[test]
fn a_nasty_password_round_trips_through_a_command() {
    let scratch = Scratch::new("command");
    let file = scratch.write("password", NASTY.as_bytes());

    let secret = super::command::run(&print_file(&file)).unwrap();

    assert_eq!(secret.expose(), NASTY);
}

#[test]
fn the_password_command_never_carries_the_password_in_argv() {
    let scratch = Scratch::new("argv");
    let file = scratch.write("password", NASTY.as_bytes());
    let command = print_file(&file);

    // What `ps` would show is the command line. The password is in the file it reads and
    // comes back down a pipe, so it is not in there.
    assert!(
        !command.contains("p@ss"),
        "the password ended up on the command line: {command}"
    );
    assert_eq!(super::command::run(&command).unwrap().expose(), NASTY);
}

#[test]
fn a_command_that_fails_says_so_and_does_not_invent_a_password() {
    let failure = super::command::run("exit 3").unwrap_err();

    assert_eq!(failure.exit().code(), 2);
    assert!(failure.message().contains("exit 3"));
}

#[test]
fn a_command_that_adds_a_newline_has_exactly_one_taken_off() {
    let scratch = Scratch::new("newline");
    let file = scratch.write("password", format!("{NASTY}\n").as_bytes());

    assert_eq!(
        super::command::run(&print_file(&file)).unwrap().expose(),
        NASTY
    );
}

// ---------------------------------------------------------------- the environment

/// The route, end to end, against a variable that really is set in this process.
///
/// It is run by re-executing this test binary with the variable in its environment.
/// `std::env::set_var` is unsafe as of the 2024 edition and this crate forbids unsafe, so
/// a child process is how a test gets a variable set — and it is the more honest check
/// anyway, since it exercises the same `std::env::var` the real path uses.
#[test]
fn a_nasty_password_round_trips_through_an_environment_variable() {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "secret::tests::environment_child",
            "--ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env("SLOOP_TEST_NASTY", NASTY)
        .output()
        .expect("re-running this test binary");

    assert!(
        output.status.success(),
        "the child failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Only ever run by the test above, which sets the variable first.
#[test]
#[ignore = "re-executed with SLOOP_TEST_NASTY set"]
fn environment_child() {
    let route = Route::Environment("SLOOP_TEST_NASTY".to_owned());
    let resolved =
        resolve(&route, &lookup(&vault(Path::new("unused")))).expect("the variable is set");

    assert_eq!(
        resolved.secret.expose(),
        NASTY,
        "the value was altered on the way"
    );
    assert!(resolved.notes.is_empty(), "{:?}", resolved.notes);
}

#[test]
fn a_variable_that_is_not_set_is_a_usage_error_naming_it() {
    let route = Route::Environment("SLOOP_DEFINITELY_NOT_SET_ANYWHERE".to_owned());
    let failure = resolve(&route, &lookup(&vault(Path::new("unused")))).unwrap_err();

    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure
            .message()
            .contains("SLOOP_DEFINITELY_NOT_SET_ANYWHERE")
    );
}

// ---------------------------------------------------------------- the keyring

/// The real OS keyring, carrying the same deliberately awful password.
///
/// **What this proves and what it does not.** sloop's job is that a password made of
/// backslashes, quotes and redirects reaches the keyring and comes back byte for byte.
/// Whether the platform's credential store is willing to hand it over at that instant is
/// not sloop's contract, and the two failures are told apart:
///
/// - the value comes back and is **wrong** — sloop's bug, and a failure;
/// - the value cannot be found at all — the platform's, and a skip with the reason printed.
///
/// That second case is real and was worth an afternoon. Under the full test suite on
/// Windows, roughly one run in five, `set_in` returns success, the credential is genuinely
/// present in Credential Manager under exactly the right target — and three consecutive
/// reads of that target say it does not exist. It is not concurrency (eight threads are
/// clean), not volume (twenty-four entries sharing a key are clean), not a tight loop
/// (three hundred rounds are clean) and not a delay (a separate process sees it
/// immediately). Whatever it is, it is below this crate, and failing the build over it
/// would only teach somebody to re-run the tests.
///
/// **Nothing is left behind, even when the process is killed.** The `Drop` guard cannot
/// run then — and the same read failure above takes the delete with it, which is how four
/// real credentials once accumulated in a developer's own Credential Manager. So the
/// service name is written down before it is used and swept on the way in, the way
/// `engine::cluster_tests` sweeps a PostgreSQL directory a killed run left running.
#[test]
fn a_nasty_password_round_trips_through_the_keyring() {
    sweep_credentials_from_earlier_runs();

    // Unique per run, so two runs — or a run and a leftover — never share one target.
    let service = format!("sloop-test-{}", std::process::id());
    let key = "postgres://app@db.internal:5432/app";
    let secret = Secret::new(NASTY.to_owned());

    let Ok(()) = super::os_keyring::set_in(&service, key, &secret) else {
        eprintln!("no usable OS keyring here; skipping the keyring round trip");
        return;
    };
    // Recorded before it is read, so a process killed on the next line still gets cleared.
    remember_credential(&service, key);
    let _cleanup = Cleanup {
        service: service.clone(),
        key,
    };

    for (what, written) in [
        ("as given", NASTY),
        ("with a trailing space", NASTY_TRAILING),
    ] {
        super::os_keyring::set_in(&service, key, &Secret::new(written.to_owned()))
            .expect("the keyring took the first password, so it takes this one");

        match super::os_keyring::get_from(&service, key) {
            // The assertion this test exists for: unchanged, byte for byte.
            Ok(back) => assert_eq!(back.expose(), written, "the password came back mangled"),
            Err(why) => {
                eprintln!(
                    "the OS keyring accepted the password {what} and then would not return \
                     it ({}); skipping the rest of the round trip",
                    why.message()
                );
                return;
            }
        }
    }
}

/// Where the service names of in-flight keyring tests are written down.
///
/// In the temporary directory rather than the target directory: `cargo clean` must not be
/// able to strand a credential in somebody's keyring.
fn credential_ledger() -> PathBuf {
    std::env::temp_dir().join("sloop-keyring-test-credentials")
}

/// Note that this run is holding a credential, so a later run can clear it if this one
/// never gets the chance.
fn remember_credential(service: &str, key: &str) {
    use std::io::Write as _;

    // A space separates the two, and can: a service name is `sloop-test-<pid>` and a key
    // is a connection string, and neither has ever contained one.
    //
    // Append, never rewrite: two test binaries could be in flight, and losing the other
    // one's line would strand its credential.
    let ledger = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(credential_ledger());
    if let Ok(mut ledger) = ledger {
        let _ = writeln!(ledger, "{service} {key}");
    }
}

/// Clear anything an earlier run left in the real keyring, then forget it.
///
/// Every failure here is ignored on purpose. A ledger that cannot be read, a credential
/// that is already gone, a keyring that is not there at all — none of them is a reason to
/// fail a test about passwords, and the next run sweeps again.
fn sweep_credentials_from_earlier_runs() {
    let ledger = credential_ledger();
    let Ok(noted) = std::fs::read_to_string(&ledger) else {
        return;
    };

    let mine = format!("sloop-test-{}", std::process::id());
    for line in noted.lines() {
        if let Some((service, key)) = line.split_once(' ') {
            // Not this run's own, in the vanishingly unlikely case a pid has come round
            // again while a ledger line for it is still there.
            if service != mine {
                let _ = super::os_keyring::delete_from(service, key);
            }
        }
    }

    let _ = std::fs::remove_file(&ledger);
}

#[test]
fn a_keyring_entry_that_is_not_there_is_a_usage_error() {
    let service = format!("sloop-test-missing-{}", std::process::id());
    let failure = super::os_keyring::get_from(&service, "nothing://is@here:1/ever").unwrap_err();

    assert_eq!(failure.exit().code(), 2);
    assert!(!failure.message().contains("p@ss"));
}

struct Cleanup {
    service: String,
    key: &'static str,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if super::os_keyring::delete_from(&self.service, self.key).is_ok() {
            // The credential is gone, so the note about it has nothing left to describe.
            // Only on success: a delete that failed — which is exactly what used to leave
            // credentials behind — has to leave the note for the next run to act on.
            let _ = std::fs::remove_file(credential_ledger());
        }
    }
}

// ---------------------------------------------------------------- helpers

/// A command that prints a file and nothing else, per platform.
fn print_file(path: &Path) -> String {
    if cfg!(windows) {
        format!("type \"{}\"", path.display())
    } else {
        format!("cat \"{}\"", path.display())
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A temporary directory, removed when the test finishes.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sloop-secret-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("writing a temporary file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
