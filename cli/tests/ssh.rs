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

// ---------------------------------------------------------------------------------------
// the whole path, through the real binary — R19e
// ---------------------------------------------------------------------------------------

mod support;

use support::Sandbox;

/// The rig, when there is one.
///
/// `SLOOP_SSH_TEST` is `user|host|port|identity|passphrase-or-empty`, naming a server with a
/// PostgreSQL bound to `127.0.0.1` inside it and nothing published but port 22 — so a
/// forward is the only way to the database. The database behind it is `orders`, owned by
/// `postgres` with the password `rigpassword`, holding three rows in `items`. Without the
/// variable every test here says so and passes, which is every CI runner.
struct Rig {
    user: String,
    host: String,
    port: String,
    identity: String,
    passphrase: String,
}

impl Rig {
    fn named() -> Option<Self> {
        let named = std::env::var("SLOOP_SSH_TEST").ok()?;
        let parts: Vec<&str> = named.split('|').collect();
        assert!(
            parts.len() >= 5,
            "SLOOP_SSH_TEST is user|host|port|identity|passphrase-or-empty"
        );
        Some(Self {
            user: parts[0].to_owned(),
            host: parts[1].to_owned(),
            port: parts[2].to_owned(),
            identity: parts[3].to_owned(),
            passphrase: parts[4].to_owned(),
        })
    }

    /// Trust this rig's host key, in the sandbox's own home and nowhere else.
    ///
    /// **This is the operator's job and sloop does not do it.** `known_hosts` stays
    /// OpenSSH's — nothing in sloop writes to it, reads it or passes an option about it —
    /// so a test of the happy path has to arrange what a person arranges by connecting
    /// once. It lands inside the sandbox, which is the home `ssh` is handed, so the
    /// developer's own file is untouched.
    fn trust_the_host_key(&self, sandbox: &Sandbox) {
        let ssh_dir = sandbox.home().join(".ssh");
        std::fs::create_dir_all(&ssh_dir).expect("a .ssh in the sandbox");

        let scanned = Command::new("ssh-keyscan")
            .args(["-p", &self.port, &self.host])
            .stdin(Stdio::null())
            .output()
            .expect("ssh-keyscan should run");
        assert!(
            scanned.status.success(),
            "ssh-keyscan could not reach the rig: {}",
            String::from_utf8_lossy(&scanned.stderr)
        );

        std::fs::write(ssh_dir.join("known_hosts"), &scanned.stdout).expect("known_hosts");
    }

    /// Register the database behind it, at the address the **server** sees.
    fn register(&self, sandbox: &Sandbox, name: &str) -> support::Run {
        sandbox.sloop(&[
            "db",
            "add",
            name,
            "--url",
            "postgres://postgres@127.0.0.1:5432/orders",
            "--ssh-host",
            &self.host,
            "--ssh-port",
            &self.port,
            "--ssh-user",
            &self.user,
            "--ssh-identity",
            &self.identity,
            "--password-from",
            if cfg!(windows) {
                "cmd /c echo rigpassword"
            } else {
                "echo rigpassword"
            },
        ])
    }
}

/// **The whole of `R19e`, through the real binary.** A database whose port is closed to
/// everything outside its own server is registered, tested and backed up from here with
/// nothing but a working `ssh` — and the backup has the rig's three rows in it.
///
/// This is the test the entry's "Done when" is written from. It needs the rig's host key to
/// be one `ssh` already trusts, which is the operator's business and not sloop's: `known_hosts`
/// stays OpenSSH's, and the test below proves what happens when it is not trusted.
#[test]
fn a_database_only_its_own_server_can_reach_is_registered_tested_and_backed_up() {
    let Some(rig) = Rig::named() else {
        eprintln!("skipping: SLOOP_SSH_TEST names no server to reach");
        return;
    };
    if !rig.passphrase.is_empty() {
        eprintln!("skipping: this test wants the key with no passphrase");
        return;
    }

    let sandbox = Sandbox::new("ssh-end-to-end");
    rig.trust_the_host_key(&sandbox);
    rig.register(&sandbox, "prod").expect_code(0);

    // The connection, over a forward this run opened.
    let reached = sandbox.sloop(&["db", "test", "prod"]);
    reached
        .expect_code(0)
        .expect_said(&format!("through {}@{}", rig.user, rig.host))
        // **The rig's PostgreSQL, not this machine's.** The rig is 15 and nothing published
        // but port 22, so a version line at all means the forward carried the connection.
        .expect_said("postgres 15")
        // And what secured it is said truthfully: the client dialled 127.0.0.1, so no
        // certificate could be matched against the name it was issued for.
        .expect_said("encrypted by SSH");

    // And a real backup of it, with the rig's three rows counted through the tunnel.
    sandbox.sloop(&["key", "export"]).expect_code(0);
    let taken = sandbox.sloop(&["backup", "prod"]);
    taken.expect_code(0).expect_said("backed up to");

    let listed = sandbox.sloop(&["backups", "list", "prod", "--json"]);
    listed.expect_code(0);
    assert!(
        listed.stdout().contains("\"rows\": 3"),
        "the backup did not come from the rig's database:\n{}",
        listed.stdout()
    );
}

/// **A key with a passphrase, supplied the way a scheduled run supplies one.**
///
/// This is the whole askpass path against a real OpenSSH: `ssh` is pointed at sloop, sloop
/// is handed the passphrase down an inherited pipe, and the forward comes up with nothing
/// typed and no terminal anywhere. The passphrase takes `--ssh-passphrase-from`, which is
/// one of `R3`'s four routes and the one that stores nothing — a keyring write would reach
/// outside the sandbox.
#[test]
fn a_key_with_a_passphrase_opens_the_forward_with_nothing_typed() {
    let Some(rig) = Rig::named() else {
        eprintln!("skipping: SLOOP_SSH_TEST names no server to reach");
        return;
    };
    if rig.passphrase.is_empty() {
        eprintln!("skipping: this test wants the key that has a passphrase");
        return;
    }

    let sandbox = Sandbox::new("ssh-passphrase");
    rig.trust_the_host_key(&sandbox);

    let prints = |what: &str| {
        if cfg!(windows) {
            format!("cmd /c echo {what}")
        } else {
            format!("echo {what}")
        }
    };
    sandbox
        .sloop(&[
            "db",
            "add",
            "prod",
            "--url",
            "postgres://postgres@127.0.0.1:5432/orders",
            "--ssh-host",
            &rig.host,
            "--ssh-port",
            &rig.port,
            "--ssh-user",
            &rig.user,
            "--ssh-identity",
            &rig.identity,
            "--ssh-passphrase-from",
            &prints(&rig.passphrase),
            "--password-from",
            &prints("rigpassword"),
        ])
        .expect_code(0);

    let reached = sandbox.sloop(&["db", "test", "prod"]);
    reached
        .expect_code(0)
        .expect_said("postgres 15")
        // The passphrase went down a pipe to one child. It is in no message, and `ps` never
        // saw it either — `ssh` was run with a path and the word `1` in its environment.
        .expect_silent_about(&rig.passphrase);
}

/// **Two operations against one server are one login**, which is the owner's whole reason
/// for the entry — *"if he wants to query or something like that then it will login after
/// each task which is not a proper apprach"*.
///
/// Two commands in two processes each open their own, so what this proves is the half a
/// test can prove from outside: the second command works, at a different local port, with no
/// second question asked. The *sharing* within one process is `ssh::tests`, where the set can
/// be counted.
#[test]
fn a_second_command_against_the_same_server_needs_nothing_more_typed() {
    let Some(rig) = Rig::named() else {
        eprintln!("skipping: SLOOP_SSH_TEST names no server to reach");
        return;
    };
    if !rig.passphrase.is_empty() {
        eprintln!("skipping: this test wants the key with no passphrase");
        return;
    }

    let sandbox = Sandbox::new("ssh-twice");
    rig.trust_the_host_key(&sandbox);
    rig.register(&sandbox, "one").expect_code(0);

    // A second database on the same server, at the same address — the case the forward set
    // is keyed for.
    sandbox
        .sloop(&[
            "db",
            "add",
            "two",
            "--url",
            "postgres://postgres@127.0.0.1:5432/orders",
            "--ssh-host",
            &rig.host,
            "--ssh-port",
            &rig.port,
            "--ssh-user",
            &rig.user,
            "--ssh-identity",
            &rig.identity,
            "--password-from",
            if cfg!(windows) {
                "cmd /c echo rigpassword"
            } else {
                "echo rigpassword"
            },
        ])
        .expect_code(0);

    // `db test` with no name tries every registered database, in one process, and both go
    // through the same login.
    sandbox.sloop(&["db", "test"]).expect_code(0);
}

/// **A host key that is not trusted fails the command and says so**, which is the entry's
/// promise that `known_hosts` stays OpenSSH's.
///
/// sloop is `ssh`'s askpass helper, and with `SSH_ASKPASS_REQUIRE=force` OpenSSH routes
/// *every* prompt to it — including this one. A helper that answered it would silently trust
/// an unknown host and nobody would ever find out, because the connection would simply start
/// working. So it refuses, `ssh` fails, and what a person sees is what they would have seen
/// without sloop.
#[test]
fn a_server_whose_host_key_is_not_trusted_fails_with_what_ssh_said() {
    let Some(rig) = Rig::named() else {
        eprintln!("skipping: SLOOP_SSH_TEST names no server to reach");
        return;
    };
    if std::env::var("SLOOP_SSH_TEST_UNTRUSTED").is_err() {
        eprintln!(
            "skipping: set SLOOP_SSH_TEST_UNTRUSTED when the rig's host key is NOT in \
             known_hosts"
        );
        return;
    }

    let sandbox = Sandbox::new("ssh-hostkey");
    // A passphrase route, so the askpass helper is what `ssh` is pointed at — which is the
    // arrangement in which answering the host key question would have been possible.
    let with_a_secret = sandbox.sloop(&[
        "db",
        "add",
        "prod",
        "--url",
        "postgres://postgres@127.0.0.1:5432/orders",
        "--ssh-host",
        &rig.host,
        "--ssh-port",
        &rig.port,
        "--ssh-user",
        &rig.user,
        "--ssh-identity",
        &rig.identity,
        "--ssh-passphrase-from",
        if cfg!(windows) {
            "cmd /c echo whatever"
        } else {
            "echo whatever"
        },
        "--password-from",
        if cfg!(windows) {
            "cmd /c echo rigpassword"
        } else {
            "echo rigpassword"
        },
    ]);
    with_a_secret.expect_code(0);

    let run = sandbox.sloop(&["db", "test", "prod"]);
    let said = format!("{}{}", run.stdout(), run.stderr());

    assert_eq!(
        run.code(),
        Some(3),
        "it has to be a connection failure:\n{said}"
    );
    assert!(
        said.contains("Host key verification failed"),
        "it has to be ssh's own words:\n{said}"
    );
}
