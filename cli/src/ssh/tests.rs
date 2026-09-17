//! What sloop runs, and — the half that matters — what its askpass helper refuses to answer.
//!
//! **The prompts below are real.** They were captured from an OpenSSH 10.2 connection to a
//! real server before this module was written, which is how the host key case was found at
//! all: with `SSH_ASKPASS_REQUIRE=force`, OpenSSH routes *every* prompt to the helper, and a
//! helper that answered the host key question would silently trust an unknown host.

use std::path::PathBuf;

use super::askpass::is_asking_for_a_secret;
use super::{DEFAULT_PORT, Server, Through};
use crate::secret::Route;

/// A server, as plainly as one can be written.
fn server() -> Server {
    Server {
        host: "db.example.test".to_owned(),
        port: DEFAULT_PORT,
        user: Some("deploy".to_owned()),
        identity: None,
    }
}

/// The two prompts OpenSSH really sends to an askpass helper when it wants a secret.
const ASKING_FOR_A_SECRET: &[&str] = &[
    "Enter passphrase for key 'key_pass': ",
    "Enter passphrase for key '/home/me/.ssh/id_ed25519': ",
    "deploy@db.example.test's password: ",
    "Password: ",
];

/// And the ones it sends that are **not** a request for a secret. Answering any of these is
/// the mistake this whole guard exists to prevent.
const NOT_ASKING_FOR_A_SECRET: &[&str] = &[
    "The authenticity of host '[127.0.0.1]:2222 ([127.0.0.1]:2222)' can't be established.\n\
     ED25519 key fingerprint is: SHA256:DVeB7mWFtZ5eqcKRkw0ODukrgPSc6FaebA3vSJVpCas\n\
     This key is not known by any other names.\n\
     Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
    "Please type 'yes', 'no' or the fingerprint: ",
    "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! Add correct host key in known_hosts",
    "",
    "   ",
];

/// **A passphrase prompt is answered and a host key question never is.**
#[test]
fn only_a_request_for_a_secret_is_ever_answered() {
    for prompt in ASKING_FOR_A_SECRET {
        assert!(
            is_asking_for_a_secret(prompt),
            "this asks for a secret and was refused: {prompt}"
        );
    }

    for prompt in NOT_ASKING_FOR_A_SECRET {
        assert!(
            !is_asking_for_a_secret(prompt),
            "this is not a request for a secret and would have been answered: {prompt}"
        );
    }
}

/// **The trap in the real prompt, spelled out.** The host key question contains the word
/// "fingerprint", and a naive rule of "does it mention a password" would still refuse it —
/// but a prompt that mentioned *both* would slip through a rule written the other way round.
#[test]
fn a_prompt_that_mentions_both_is_refused_rather_than_answered() {
    let both = "The authenticity of host 'x' can't be established. Enter passphrase? \
                Are you sure you want to continue connecting (yes/no)? ";
    assert!(
        !is_asking_for_a_secret(both),
        "when in doubt this has to refuse, because the cost of the two mistakes is not equal"
    );
}

/// The forward, the failure mode, and the keepalive — the three that decide whether a tunnel
/// is real, silent or stale.
#[test]
fn the_arguments_bind_one_forward_on_loopback_and_fail_loudly() {
    let through = Through {
        server: server(),
        secret: None,
    };
    let arguments = through.arguments(54321, "127.0.0.1", 5432);

    assert!(arguments.contains(&"-N".to_owned()), "{arguments:?}");
    assert!(
        arguments.contains(&"ExitOnForwardFailure=yes".to_owned()),
        "a forward that could not be bound has to be a failure: {arguments:?}"
    );
    assert!(
        arguments.contains(&"ServerAliveInterval=30".to_owned()),
        "a held connection has to notice a server that went away: {arguments:?}"
    );
    assert!(
        arguments.contains(&"127.0.0.1:54321:127.0.0.1:5432".to_owned()),
        "the forward is bound on loopback and nowhere else: {arguments:?}"
    );
    assert_eq!(
        arguments.last().map(String::as_str),
        Some("deploy@db.example.test"),
        "the destination is last, the way ssh takes it"
    );
}

/// **`BatchMode` is set exactly when there is no secret to send, and that is not a detail.**
/// It stops `ssh` asking anything of its own — rule 4 — but it also disables the askpass
/// helper, so setting it unconditionally would make a configured passphrase unusable.
#[test]
fn batch_mode_is_on_without_a_secret_and_off_with_one() {
    let agentless = Through {
        server: server(),
        secret: None,
    };
    assert!(
        agentless
            .arguments(1, "127.0.0.1", 5432)
            .contains(&"BatchMode=yes".to_owned()),
        "with nothing to send, ssh must never stop to ask"
    );

    let with_a_passphrase = Through {
        server: server(),
        secret: Some(Route::Keyring),
    };
    assert!(
        !with_a_passphrase
            .arguments(1, "127.0.0.1", 5432)
            .contains(&"BatchMode=yes".to_owned()),
        "BatchMode disables askpass, which is the only way the passphrase could arrive"
    );
}

/// `-p` only when it is not 22, and `-i` only when a key was named — so a machine whose
/// `~/.ssh/config` already answers both is not overridden by sloop's defaults.
#[test]
fn nothing_is_passed_that_the_users_own_ssh_config_should_decide() {
    let plain = Through {
        server: server(),
        secret: None,
    };
    let arguments = plain.arguments(1, "127.0.0.1", 5432);
    assert!(!arguments.contains(&"-p".to_owned()), "{arguments:?}");
    assert!(!arguments.contains(&"-i".to_owned()), "{arguments:?}");

    let spelled_out = Through {
        server: Server {
            port: 2222,
            identity: Some(PathBuf::from("/home/me/.ssh/id_rig")),
            ..server()
        },
        secret: None,
    };
    let arguments = spelled_out.arguments(1, "127.0.0.1", 5432);
    assert!(arguments.contains(&"2222".to_owned()), "{arguments:?}");
    assert!(
        arguments.contains(&"/home/me/.ssh/id_rig".to_owned()),
        "{arguments:?}"
    );
}

/// A destination with no user is `host`, which is what leaves `~/.ssh/config` in charge.
#[test]
fn a_server_with_no_user_named_leaves_it_to_ssh() {
    let anyone = Server {
        user: None,
        ..server()
    };
    assert_eq!(anyone.destination(), "db.example.test");
    assert_eq!(anyone.describe(), "db.example.test");

    let elsewhere = Server {
        port: 2222,
        ..server()
    };
    assert_eq!(elsewhere.describe(), "deploy@db.example.test:2222");
}

/// **Two databases on one server share one login, and that is the owner's whole point.**
/// Two entries naming different keys do not, because they are two different logins.
#[test]
fn databases_on_one_server_share_a_connection_and_different_keys_do_not() {
    let one = server();
    let another = server();
    assert_eq!(one.credential_key(), another.credential_key());

    let other_user = Server {
        user: Some("someone-else".to_owned()),
        ..server()
    };
    assert_ne!(one.credential_key(), other_user.credential_key());

    let other_key = Server {
        identity: Some(PathBuf::from("/home/me/.ssh/id_other")),
        ..server()
    };
    assert_ne!(one.credential_key(), other_key.credential_key());

    let other_port = Server {
        port: 2222,
        ..server()
    };
    assert_ne!(one.credential_key(), other_port.credential_key());
}

/// **It is also a keyring account name, so it has to read like one.** Somebody opening
/// Credential Manager should be able to tell what a row is for without decoding it.
#[test]
fn the_login_key_reads_as_something_a_person_can_place() {
    assert_eq!(server().credential_key(), "ssh://deploy@db.example.test:22");

    let anyone = Server {
        user: None,
        ..server()
    };
    assert_eq!(anyone.credential_key(), "ssh://db.example.test:22");

    let with_a_key = Server {
        identity: Some(PathBuf::from("/home/me/.ssh/id_ed25519")),
        ..server()
    };
    assert_eq!(
        with_a_key.credential_key(),
        "ssh://deploy@db.example.test:22#/home/me/.ssh/id_ed25519"
    );

    // The port is always in it, so passing `--ssh-port 22` and leaving it out file the
    // passphrase under the same name rather than under two.
    let said_so = Server {
        port: DEFAULT_PORT,
        ..server()
    };
    assert_eq!(said_so.credential_key(), server().credential_key());
}

/// What a listing says about a tunnel, and what it cannot say.
#[test]
fn how_a_tunnel_reads_carries_no_secret() {
    let agent = Through {
        server: server(),
        secret: None,
    };
    assert_eq!(
        agent.describe(),
        "over deploy@db.example.test, from the agent"
    );

    let kept = Through {
        server: server(),
        secret: Some(Route::Keyring),
    };
    assert_eq!(
        kept.describe(),
        "over deploy@db.example.test, unlocked from the OS keyring"
    );

    // A `command:` route is a command line, which is a direction and not a value — the same
    // property that lets a password route be printed.
    let from_a_manager = Through {
        server: server(),
        secret: Some(Route::Command("op read op://vault/ssh/pw".to_owned())),
    };
    assert!(from_a_manager.describe().contains("op read"));
}

/// The passphrase's *route* plays no part in which connection is shared: it opens the same
/// key, so two entries that differ only in where the passphrase is kept are one login.
#[test]
fn where_the_passphrase_is_kept_does_not_split_a_connection() {
    let keyring = Through {
        server: server(),
        secret: Some(Route::Keyring),
    };
    let from_a_command = Through {
        server: server(),
        secret: Some(Route::Command("op read op://vault/ssh/pw".to_owned())),
    };

    assert_eq!(
        keyring.server.credential_key(),
        from_a_command.server.credential_key()
    );
}

/// **A real tunnel to a real server, when there is one.**
///
/// `SLOOP_SSH_TEST` names it: `user@host`, the port, the key, its passphrase route and the
/// database behind it, separated by `|`. A machine with a throwaway server set up can prove
/// the whole path — spawn `ssh`, bind a forward, and find the far side answering — and a
/// machine without one says so and passes, which is every CI runner.
#[test]
fn a_forward_really_opens_and_the_far_side_answers() {
    let Ok(named) = std::env::var("SLOOP_SSH_TEST") else {
        eprintln!("skipping: SLOOP_SSH_TEST names no server to reach");
        return;
    };

    let parts: Vec<&str> = named.split('|').collect();
    assert!(
        parts.len() >= 5,
        "SLOOP_SSH_TEST is user|host|port|identity|passphrase-or-empty"
    );
    let passphrase = parts[4];

    let through = Through {
        server: Server {
            host: parts[1].to_owned(),
            port: parts[2].parse().expect("a port"),
            user: Some(parts[0].to_owned()),
            identity: Some(PathBuf::from(parts[3])),
        },
        secret: (!passphrase.is_empty()).then_some(Route::Keyring),
    };

    let secret =
        (!passphrase.is_empty()).then(|| crate::secret::Secret::new(passphrase.to_owned()));

    // **The real sloop binary, not this test harness.** `current_exe()` here is the test
    // binary in `target/debug/deps`, and `ssh` pointed at that would get a test runner rather
    // than an askpass helper — which is precisely how the first run of this failed.
    let me = std::env::current_exe().expect("this test binary");
    let sloop = me
        .parent()
        .and_then(std::path::Path::parent)
        .expect("target/debug")
        .join(if cfg!(windows) { "sloop.exe" } else { "sloop" });
    assert!(sloop.is_file(), "{} is not there", sloop.display());

    let mut held = super::tunnel::Tunnels::with_helper(sloop);
    let tunnel = held
        .to(&through, "127.0.0.1", 5432, secret.as_ref())
        .expect("the forward should open");
    let port = tunnel.local_port();

    assert!(
        !super::tunnel::is_bindable(port),
        "if this port is still free then nothing is listening on the forward"
    );

    // **And the database on the far side answers through it**, which is the only thing that
    // actually proves a tunnel rather than a bound port. `psql` connects to `127.0.0.1:<local>`
    // and knows nothing about SSH — the whole design in one assertion.
    if let Some(psql) =
        crate::tools::acquire::on_path_at(if cfg!(windows) { "psql.exe" } else { "psql" })
    {
        let said = std::process::Command::new(psql)
            .args(["-h", super::LOOPBACK])
            .args(["-p", &port.to_string()])
            .args(["-U", "postgres", "-d", "orders"])
            .args(["--tuples-only", "--no-align", "--no-password"])
            .args(["-c", "SELECT count(*) FROM items"])
            .env("PGPASSWORD", "rigpassword")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("psql should run");

        assert!(
            said.status.success(),
            "the far side did not answer: {}",
            String::from_utf8_lossy(&said.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&said.stdout).trim(),
            "3",
            "the rig's table has three rows in it"
        );
    } else {
        eprintln!("note: no psql here, so only the forward itself was checked");
    }

    // **And the same server twice is one login**, which is the owner's whole point.
    let again = held
        .to(&through, "127.0.0.1", 5432, secret.as_ref())
        .expect("the second ask should reuse the first");
    assert_eq!(again.local_port(), port);
    assert_eq!(held.count(), 1, "two asks, one connection");

    // Closed when it goes out of scope, by the type system rather than by remembering.
    drop(held);
    for _ in 0..50 {
        if super::tunnel::is_bindable(port) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("the forward on {port} was still up after the tunnel was dropped");
}
