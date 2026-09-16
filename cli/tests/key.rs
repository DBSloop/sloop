//! `sloop key export`, `sloop key import`, and the one moment sloop insists on being read.
//!
//! **Every key here lives inside the sandbox.** The sandbox sets `SLOOP_PASSPHRASE`, which
//! tells sloop this machine keeps secrets in the Argon2id file rather than the OS keyring —
//! so a test run cannot leave a key in the developer's own Credential Manager, which no
//! temporary directory can clean up.
//!
//! The format itself, and what an encrypted dump decrypts to, are in `crypt::tests` and
//! `commands::backup::cluster_tests`.

mod support;

use support::Sandbox;

/// What the registry says about encryption, if anything.
fn registry(sandbox: &Sandbox) -> String {
    std::fs::read_to_string(sandbox.global_dir().join("registry.toml")).unwrap_or_default()
}

/// The key on standard output, with nothing else mixed into it.
fn exported(sandbox: &Sandbox) -> String {
    let run = sandbox.sloop(&["key", "export"]);
    run.expect_code(0);

    let printed = run.stdout();
    let key = printed.trim();
    assert!(
        key.starts_with("AGE-SECRET-KEY-1"),
        "standard output is not just the key: {printed:?}"
    );
    assert_eq!(printed.lines().count(), 1, "more than the key: {printed:?}");
    key.to_owned()
}

/// A redirect gets the key and nothing else; a person gets the explanation.
///
/// That split is the whole interface: `sloop key export > backup-key.txt` has to produce a
/// file age itself would accept, and the warning about what losing it costs has to be
/// somewhere the redirect cannot swallow.
#[test]
fn export_puts_the_key_on_stdout_and_the_warning_on_stderr() {
    let sandbox = Sandbox::new("key-export");
    let run = sandbox.sloop(&["key", "export"]);

    run.expect_code(0);

    let key = run.stdout();
    assert!(key.starts_with("AGE-SECRET-KEY-1"), "{key:?}");
    assert_eq!(key.lines().count(), 1, "{key:?}");

    let explaining = run.stderr();
    assert!(
        explaining.contains("age1"),
        "the public key is not in the explanation: {explaining}"
    );
    assert!(
        explaining.contains("password manager") || explaining.contains("safe"),
        "nothing said where to keep it: {explaining}"
    );
    assert!(
        !explaining.contains("AGE-SECRET-KEY-1"),
        "the private key is on stderr as well as stdout: {explaining}"
    );

    // The public half is in the registry, the private half is not.
    let file = registry(&sandbox);
    assert!(file.contains("[encryption]"), "{file}");
    assert!(file.contains("public-key = \"age1"), "{file}");
    assert!(file.contains("key-kept = \"exported\""), "{file}");
    assert!(
        !file.contains("AGE-SECRET-KEY"),
        "the private key is in the registry: {file}"
    );
}

/// Exporting twice gives the same key. A second call that quietly made a new one would make
/// every backup taken in between unreadable.
#[test]
fn exporting_twice_exports_the_same_key() {
    let sandbox = Sandbox::new("key-twice");

    let first = exported(&sandbox);
    let second = exported(&sandbox);
    assert_eq!(first, second, "the key changed underneath the backups");
}

/// The round trip somebody does when they move to a new machine.
#[test]
fn a_key_exported_here_imports_there() {
    let here = Sandbox::new("key-from");
    let key = exported(&here);
    let public = registry(&here)
        .lines()
        .find(|line| line.starts_with("public-key"))
        .expect("the registry names the public key")
        .to_owned();

    let there = Sandbox::new("key-to");
    let run = there
        .command(there.work(), &["key", "import"])
        .stdin(format!("{key}\n").as_bytes())
        .run();

    run.expect_code(0).expect_said("imported");
    // Never echoed, on either stream.
    run.expect_silent_about(&key);

    let file = registry(&there);
    assert!(
        file.contains(public.trim()),
        "the imported key is not the one that arrived:\n{file}"
    );
    assert!(file.contains("key-kept = \"exported\""), "{file}");

    // And the machine it landed on exports the same key back out.
    assert_eq!(exported(&there), key);
}

/// Importing over a different key is refused. The backups already taken can only be read
/// with the key they were written for.
#[test]
fn a_different_key_is_not_allowed_to_replace_the_one_in_use() {
    let sandbox = Sandbox::new("key-replace");
    let mine = exported(&sandbox);

    let somebody_elses = exported(&Sandbox::new("key-other"));
    assert_ne!(mine, somebody_elses);

    let run = sandbox
        .command(sandbox.work(), &["key", "import"])
        .stdin(format!("{somebody_elses}\n").as_bytes())
        .run();

    run.expect_code(2)
        .expect_said("already encrypting to")
        .expect_said("different key");

    // Importing the key it already has is fine, and is what moving a machine looks like.
    sandbox
        .command(sandbox.work(), &["key", "import"])
        .stdin(format!("{mine}\n").as_bytes())
        .run()
        .expect_code(0);
    assert_eq!(exported(&sandbox), mine, "the key changed");
}

#[test]
fn what_is_not_a_key_is_refused_without_being_echoed() {
    let sandbox = Sandbox::new("key-junk");

    for attempt in ["hunter2", "AGE-SECRET-KEY-1NOPE", "age1alsonot"] {
        let run = sandbox
            .command(sandbox.work(), &["key", "import"])
            .stdin(format!("{attempt}\n").as_bytes())
            .run();
        run.expect_code(2).expect_said("not an age private key");
    }

    // Nothing at all on standard input says so, rather than waiting for something.
    sandbox
        .command(sandbox.work(), &["key", "import"])
        .stdin(b"")
        .run()
        .expect_code(2)
        .expect_said("nothing arrived on standard input");

    assert!(
        !registry(&sandbox).contains("[encryption]"),
        "a refused import wrote a key anyway"
    );
}

/// R11's other half: the first encrypted backup refuses until the key has been copied
/// somewhere, and without a terminal it says which command does that rather than asking.
#[test]
fn the_first_backup_will_not_run_until_the_key_has_been_copied() {
    let sandbox = Sandbox::new("key-gate");
    sandbox
        .sloop(&[
            "db",
            "add",
            "orders",
            "--url",
            "postgres://app@127.0.0.1:1/orders",
            "--env",
            "PW",
        ])
        .expect_code(0);

    let refused = sandbox
        .command(sandbox.work(), &["backup", "orders"])
        .env("PW", "whatever")
        .run();

    refused
        .expect_code(2)
        .expect_said("has not been copied anywhere yet")
        .expect_said("no terminal")
        .expect_said("sloop key export");

    // It said what losing the key costs, in those words, once.
    refused
        .expect_said("this registry has a new backup key")
        .expect_said("unreadable");

    // The key was created and kept, so the same key is what gets exported next.
    let file = registry(&sandbox);
    assert!(file.contains("[encryption]"), "{file}");
    assert!(
        !file.contains("key-kept"),
        "it recorded a copy nobody has made: {file}"
    );
    assert!(
        sandbox.global_dir().join("secrets.sealed").is_file(),
        "the private key was not kept in the sandbox's own file"
    );
    assert!(
        !std::fs::read_to_string(sandbox.global_dir().join("secrets.sealed"))
            .unwrap_or_default()
            .contains("AGE-SECRET-KEY"),
        "the private key is sitting in the file in the clear"
    );

    // Nothing was dumped: the refusal came before any connection was opened.
    assert!(!sandbox.global_dir().join("backups").exists());

    // After exporting, the same command gets as far as the server.
    exported(&sandbox);
    sandbox
        .command(sandbox.work(), &["backup", "orders"])
        .env("PW", "whatever")
        .run()
        .expect_code(3);
}

/// `key --help` says which stream carries what, because that is the whole usage.
#[test]
fn key_help_says_which_stream_the_key_comes_out_on() {
    let sandbox = Sandbox::new("key-help");

    sandbox
        .sloop(&["key", "export", "--help"])
        .expect_code(0)
        .expect_said("standard output");
    sandbox
        .sloop(&["key", "import", "--help"])
        .expect_code(0)
        .expect_said("standard input");
}
