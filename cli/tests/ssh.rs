//! sloop, run the way `ssh` runs it — and, where there is a server to reach, a real tunnel.
//!
//! **The askpass half needs no server and always runs.** It spawns the real binary the way
//! OpenSSH spawns an askpass helper: the prompt in `argv`, the secret on the standard input
//! it inherited, and `SLOOP_ASKPASS=1` in the environment. What comes back on standard output
//! is what `ssh` would have used as the answer, so this test is the contract.
//!
//! **The tunnel half needs a server, and skips without one.** `SLOOP_SSH_TEST` names it —
//! `user@host:port` plus the key and the database behind it — so a machine with a throwaway
//! server can prove the whole path, and CI, which has none, says so and passes.

use std::io::Write as _;
use std::process::{Command, Stdio};

/// Run the binary as `ssh` would run an askpass helper, and hand back what it printed.
fn askpass(prompt: &str, on_stdin: Option<&str>) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sloop"))
        .arg(prompt)
        .env("SLOOP_ASKPASS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary under test should be runnable");

    {
        let mut pipe = child.stdin.take().expect("its standard input");
        if let Some(secret) = on_stdin {
            let _ = writeln!(pipe, "{secret}");
        }
        // Dropped either way, so a helper that is not going to be answered sees end of file
        // rather than waiting for a line that never comes.
    }

    let finished = child.wait_with_output().expect("it should finish");
    (
        finished.status.success(),
        String::from_utf8_lossy(&finished.stdout).trim().to_owned(),
    )
}

/// **The whole point of the helper**: a passphrase goes in on the inherited pipe and comes
/// back out on standard output, where `ssh` reads it.
#[test]
fn a_passphrase_travels_on_the_pipe_and_never_in_argv_or_the_environment() {
    let (ok, said) = askpass(
        "Enter passphrase for key '/home/me/.ssh/id_ed25519': ",
        Some("correct horse battery staple"),
    );

    assert!(ok, "it should have answered");
    assert_eq!(said, "correct horse battery staple");
}

/// **And the refusal that matters.** With `SSH_ASKPASS_REQUIRE=force`, OpenSSH sends the host
/// key question to the helper too — captured verbatim from a real connection. A helper that
/// answered it would silently trust an unknown host, so this one says nothing and exits
/// non-zero, and `ssh` fails with *"Host key verification failed"*.
#[test]
fn the_host_key_question_is_never_answered_even_with_a_secret_on_the_pipe() {
    let (ok, said) = askpass(
        "The authenticity of host '[127.0.0.1]:2222 ([127.0.0.1]:2222)' can't be \
         established.\nED25519 key fingerprint is: SHA256:DVeB7mWFtZ5eqcKRkw0ODukrgPSc6Faeb\n\
         Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
        Some("yes"),
    );

    assert!(!ok, "it must not answer a host key question");
    assert!(
        said.is_empty(),
        "anything it prints is taken as the answer, so it prints nothing: {said:?}"
    );
}

/// The follow-up prompt is the same refusal, and it is worth its own case because it mentions
/// neither "passphrase" nor "password" and so has to fail the *other* half of the guard.
#[test]
fn the_second_host_key_prompt_is_refused_too() {
    let (ok, said) = askpass("Please type 'yes', 'no' or the fingerprint: ", Some("yes"));
    assert!(!ok);
    assert!(said.is_empty(), "{said:?}");
}

/// Nothing on the pipe is a run with no secret configured meeting a prompt it cannot answer.
/// Refusing is right: `ssh` then fails and says what it wanted, rather than being handed an
/// empty passphrase and reporting something less useful.
#[test]
fn a_prompt_with_nothing_to_answer_it_with_is_refused_rather_than_answered_blank() {
    let (ok, said) = askpass("Enter passphrase for key 'id_ed25519': ", None);
    assert!(!ok);
    assert!(said.is_empty(), "{said:?}");
}

/// A secret with spaces, punctuation and a trailing newline arrives exactly as it was sent.
/// A passphrase that was mangled in transit would look like a wrong passphrase, which is the
/// least debuggable failure this path could have.
#[test]
fn the_secret_arrives_byte_for_byte() {
    let awkward = r#"a b"c'd\e$f`g{h}i"#;
    let (ok, said) = askpass("Enter passphrase for key 'x': ", Some(awkward));

    assert!(ok);
    assert_eq!(said, awkward);
}
