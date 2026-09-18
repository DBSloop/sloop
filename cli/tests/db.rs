//! `sloop db …`, through the real binary, in a sandbox of its own.
//!
//! **No server and no keyring.** Both would make these tests depend on the machine — and
//! the keyring in particular is shared, real and outside the sandbox, so a test that wrote
//! to it would be writing to the developer's own credential store. So every registration
//! here uses `--env` or `--password-from`, the two routes that keep nothing, and what is
//! actually checked is the part that is entirely sloop's: what lands in the registry file,
//! what comes back out of it, and what is refused.
//!
//! The parts that need a database are in `engine::cluster_tests`.

mod support;

use support::Sandbox;

/// Read the registry a run just wrote, so a test can assert on the file rather than on
/// the message the command printed about it.
fn written(sandbox: &Sandbox) -> String {
    sandbox.registry_text()
}

/// Register one, the short way, for the tests that are about something else.
fn registered(sandbox: &Sandbox, name: &str, url: &str) {
    sandbox
        .sloop(&["db", "add", name, "--url", url, "--env", "PW"])
        .expect_code(0);
}

/// R7's "Done when", end to end: the same connection registered two ways has to come back
/// out identical. Checked on the file, because that is the thing that outlives the run.
#[test]
fn a_url_and_the_fields_it_stands_for_register_the_same_connection() {
    let sandbox = Sandbox::new("db-same");

    sandbox
        .sloop(&[
            "db",
            "add",
            "by-url",
            "--url",
            "postgres://app@db.internal:5432/orders",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(0);

    sandbox
        .sloop(&[
            "db",
            "add",
            "by-field",
            "--engine",
            "postgres",
            "--host",
            "db.internal",
            "--port",
            "5432",
            "--database",
            "orders",
            "--user",
            "app",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(0);

    // The two blocks differ only in their heading. Anything else and the two ways of
    // registering a database have drifted apart.
    let file = written(&sandbox);
    let block = |name: &str| {
        file.split(&format!("[databases.{name}]"))
            .nth(1)
            .unwrap_or_else(|| panic!("{name} is not in the registry:\n{file}"))
            .split("\n[")
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    };

    assert_eq!(block("by-url"), block("by-field"), "\n{file}");
}

/// `db list` prints a route and never a value. The one assertion in this file that is
/// about the product rather than about the plumbing.
#[test]
fn listing_shows_the_route_and_never_the_password() {
    let sandbox = Sandbox::new("db-list");

    sandbox
        .sloop(&[
            "db",
            "add",
            "orders",
            // A password in the URL, which is the one way a real secret can reach these
            // commands at all. It must not come back out of any of them.
            "--url",
            "postgres://app:hunter2@db.internal/orders",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(0);

    let run = sandbox.sloop(&["db", "list"]);
    run.expect_code(0)
        .expect_said("orders")
        .expect_said("postgres://app@db.internal:5432/orders")
        .expect_said("the environment variable PGPASSWORD")
        .expect_silent_about("hunter2");

    assert!(
        !written(&sandbox).contains("hunter2"),
        "the password reached the registry file:\n{}",
        written(&sandbox)
    );
}

/// Rule 4, on the command most likely to meet it: a registration that needs a password
/// and has no terminal to ask at must exit `2` and name the flag, not hang.
#[test]
fn without_a_terminal_a_stored_password_exits_two_and_names_the_flag() {
    let sandbox = Sandbox::new("db-no-tty");

    let run = sandbox.sloop(&[
        "db",
        "add",
        "orders",
        "--url",
        "postgres://app@db.internal/orders",
        "--keyring",
    ]);

    run.expect_code(2)
        .expect_said("no terminal")
        .expect_said("--password-stdin");

    // And nothing was written on the way to refusing.
    assert!(
        written(&sandbox).is_empty(),
        "a refused registration still wrote a registry"
    );
}

/// The escape hatch from the test above, and the shape a scheduled job uses.
///
/// `--password-from` rather than `--keyring`, because a keyring write would reach outside
/// the sandbox. What is being checked is that the run completes without a terminal.
#[test]
fn a_route_that_stores_nothing_needs_no_terminal_at_all() {
    let sandbox = Sandbox::new("db-headless");

    let helper = if cfg!(windows) {
        "cmd /c echo pw"
    } else {
        "echo pw"
    };
    sandbox
        .sloop(&[
            "db",
            "add",
            "nightly",
            "--url",
            "mysql://bk@db.internal/app",
            "--password-from",
            helper,
        ])
        .expect_code(0);

    assert!(
        written(&sandbox).contains("command:"),
        "{}",
        written(&sandbox)
    );
}

/// A password that arrives on standard input is taken exactly as it was sent: one trailing
/// newline comes off and nothing else does, because a password may legitimately end in a
/// space and trimming would register something different from what was piped.
#[test]
fn a_piped_password_is_taken_verbatim_and_is_never_echoed() {
    let sandbox = Sandbox::new("db-stdin");

    // An awkward one on purpose. It goes to the encrypted file rather than the keyring so
    // that nothing outside the sandbox is touched, and `SLOOP_PASSPHRASE` supplies the
    // passphrase that would otherwise be prompted for.
    let nasty = "pw with a space and >|$# \"quotes\" ";
    let run = sandbox
        .command(
            sandbox.work(),
            &[
                "db",
                "add",
                "sealed",
                "--url",
                "postgres://app@db.internal/orders",
                "--encrypted-file",
                "--password-stdin",
            ],
        )
        .env("SLOOP_PASSPHRASE", "a test passphrase")
        .stdin(format!("{nasty}\n").as_bytes())
        .run();

    run.expect_code(0).expect_silent_about(nasty.trim());

    // `R19c4` moved the sealed store into `sloop_database`. The bytes are the same
    // `SLOOPSEC` blob the file held, so both assertions below mean what they always meant.
    let bytes = sandbox.sealed_bytes();
    assert!(!bytes.is_empty(), "nothing was sealed");

    // The encrypted store is encrypted: the password is not sitting in it in the clear.
    assert!(
        !bytes
            .windows(nasty.len())
            .any(|window| window == nasty.as_bytes()),
        "the password is readable in the sealed store"
    );

    assert!(
        written(&sandbox).contains("encrypted-file"),
        "{}",
        written(&sandbox)
    );
}

/// Registering the same name twice is refused rather than silently overwritten, and
/// `--force` is the way to mean it.
#[test]
fn a_name_already_taken_is_refused_until_force_says_otherwise() {
    let sandbox = Sandbox::new("db-force");
    let add = |name: &str, url: &str, force: bool| {
        let mut args = vec!["db", "add", name, "--url", url, "--env", "PGPASSWORD"];
        if force {
            args.push("--force");
        }
        sandbox.sloop(&args)
    };

    add("orders", "postgres://app@one.internal/orders", false).expect_code(0);

    add("orders", "postgres://app@two.internal/orders", false)
        .expect_code(2)
        .expect_said("already registered");
    assert!(
        written(&sandbox).contains("one.internal"),
        "it was replaced"
    );

    add("orders", "postgres://app@two.internal/orders", true).expect_code(0);
    assert!(
        written(&sandbox).contains("two.internal"),
        "it was not replaced"
    );
}

/// Renaming moves the label and leaves the connection — and therefore the key the password
/// is filed under — exactly where it was.
#[test]
fn renaming_moves_the_label_and_nothing_else() {
    let sandbox = Sandbox::new("db-rename");

    sandbox
        .sloop(&[
            "db",
            "add",
            "old",
            "--url",
            "mariadb://app@db.internal:3306/shop",
            "--env",
            "PW",
        ])
        .expect_code(0);
    let before = written(&sandbox);

    sandbox
        .sloop(&["db", "rename", "old", "new"])
        .expect_code(0);
    let after = written(&sandbox);

    assert!(after.contains("[databases.new]"), "{after}");
    assert!(!after.contains("[databases.old]"), "{after}");
    assert_eq!(
        before.replace("[databases.old]", "[databases.new]"),
        after,
        "renaming changed something other than the name"
    );

    sandbox
        .sloop(&["db", "rename", "missing", "whatever"])
        .expect_code(2)
        .expect_said("no database is registered as missing");
}

/// Editing changes what was named and keeps what was not.
#[test]
fn editing_changes_only_what_was_named() {
    let sandbox = Sandbox::new("db-edit");

    sandbox
        .sloop(&[
            "db",
            "add",
            "orders",
            "--url",
            "postgres://app@db.internal:5432/orders",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(0);

    sandbox
        .sloop(&["db", "edit", "orders", "--user", "backup"])
        .expect_code(0);

    let file = written(&sandbox);
    assert!(file.contains("user = \"backup\""), "{file}");
    assert!(file.contains("host = \"db.internal\""), "{file}");
    assert!(file.contains("port = 5432"), "{file}");
    assert!(file.contains("database = \"orders\""), "{file}");

    // An edit that changes nothing says so rather than rewriting the file for no reason.
    sandbox
        .sloop(&["db", "edit", "orders", "--user", "backup"])
        .expect_code(0)
        .expect_said("already like that");

    sandbox
        .sloop(&["db", "edit", "nope", "--user", "x"])
        .expect_code(2)
        .expect_said("no database is registered as nope");
}

/// A project registry and the global store are both live, and a bare name finds the
/// nearer one — R2's rule, and R7 is the first command that can demonstrate it.
#[test]
fn a_project_entry_shadows_a_global_one_and_the_listing_says_so() {
    let sandbox = Sandbox::new("db-scopes");

    // Global first, from a directory with no project in it.
    sandbox
        .sloop(&[
            "db",
            "add",
            "orders",
            "--url",
            "postgres://app@global.internal/orders",
            "--env",
            "PW",
        ])
        .expect_code(0);

    let project = sandbox.make_dir("app");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);
    sandbox
        .sloop_in(
            &project,
            &[
                "db",
                "add",
                "orders",
                "--url",
                "postgres://app@project.internal/orders",
                "--env",
                "PW",
            ],
        )
        .expect_code(0);

    // From inside the project: both are listed, the project one first, and the global one
    // is marked as the one a bare `orders` will not reach.
    let inside = sandbox.sloop_in(&project, &["db", "list"]);
    inside
        .expect_code(0)
        .expect_said("project.internal")
        .expect_said("global.internal")
        .expect_said("shadowed");

    assert!(
        inside.stdout().find("project.internal") < inside.stdout().find("global.internal"),
        "the nearer entry should be listed first:\n{}",
        inside.stdout()
    );

    // `--global` reaches past the project to the other one.
    sandbox
        .sloop_in(&project, &["--global", "db", "list"])
        .expect_code(0)
        .expect_said("global.internal")
        .expect_silent_about("project.internal");
}

/// The qualifier is understood by the commands that take a name, and it is not part of
/// the name that gets written down.
#[test]
fn the_global_qualifier_reaches_past_a_project() {
    let sandbox = Sandbox::new("db-qualifier");

    sandbox
        .sloop(&[
            "db",
            "add",
            "orders",
            "--url",
            "postgres://app@global.internal/orders",
            "--env",
            "PW",
        ])
        .expect_code(0);

    let project = sandbox.make_dir("app");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);
    sandbox
        .sloop_in(
            &project,
            &[
                "db",
                "add",
                "orders",
                "--url",
                "postgres://app@project.internal/orders",
                "--env",
                "PW",
            ],
        )
        .expect_code(0);

    // `global:orders` edits the global one even though a nearer one exists.
    sandbox
        .sloop_in(&project, &["db", "edit", "global:orders", "--user", "far"])
        .expect_code(0);

    let global = sandbox.registry_text();
    assert!(global.contains("user = \"far\""), "{global}");
    // The qualifier is not part of the stored name.
    assert!(global.contains("[databases.orders]"), "{global}");
    assert!(!global.contains("global:orders"), "{global}");

    let project_file = sandbox.project_registry_text(&project);
    assert!(project_file.contains("user = \"app\""), "{project_file}");
}

/// Nothing registered is a sentence, not an empty screen or an error.
#[test]
fn an_empty_registry_says_what_to_do_about_it() {
    let sandbox = Sandbox::new("db-empty");
    sandbox
        .sloop(&["db", "list"])
        .expect_code(0)
        .expect_said("Nothing is registered")
        .expect_said("sloop db add");

    sandbox
        .sloop(&["db", "test"])
        .expect_code(0)
        .expect_said("Nothing is registered");
}

/// What a half-given registration says. Each one names the flag that completes it.
#[test]
fn an_incomplete_registration_names_the_flag_that_completes_it() {
    let sandbox = Sandbox::new("db-incomplete");

    let cases = [
        (vec!["db", "add", "x", "--env", "PW"], "--engine"),
        (
            vec!["db", "add", "x", "--engine", "postgres", "--env", "PW"],
            "--host",
        ),
        (
            vec![
                "db", "add", "x", "--engine", "postgres", "--host", "h", "--env", "PW",
            ],
            "--database",
        ),
        (
            vec![
                "db",
                "add",
                "x",
                "--engine",
                "postgres",
                "--host",
                "h",
                "--database",
                "d",
                "--env",
                "PW",
            ],
            "--user",
        ),
    ];

    for (args, flag) in cases {
        sandbox.sloop(&args).expect_code(2).expect_said(flag);
    }

    // And a URL that is not one.
    sandbox
        .sloop(&["db", "add", "x", "--url", "just-a-string", "--env", "PW"])
        .expect_code(2)
        .expect_said("no scheme");

    sandbox
        .sloop(&["db", "add", "x", "--url", "redis://h/0", "--env", "PW"])
        .expect_code(2)
        .expect_said("not an engine sloop knows");
}

/// There is no `--password` flag, and its absence is the product rather than an omission.
#[test]
fn there_is_no_way_to_put_a_password_in_argv() {
    let sandbox = Sandbox::new("db-no-password-flag");

    sandbox
        .sloop(&[
            "db",
            "add",
            "x",
            "--url",
            "postgres://app@h/d",
            "--password",
            "hunter2",
        ])
        .expect_code(2);

    // And the help says why, rather than leaving somebody looking for the flag.
    sandbox
        .sloop(&["db", "add", "--help"])
        .expect_code(0)
        .expect_said("password never goes in a flag");
}

// ---------------------------------------------------------------------------------------
// remove and drop
// ---------------------------------------------------------------------------------------

/// Rule 4 on the two commands that ask a question: with no terminal they exit 2 and name
/// the flag, rather than waiting for somebody who is not there.
#[test]
fn neither_remove_nor_drop_stops_to_ask_without_a_terminal() {
    let sandbox = Sandbox::new("db-no-ask");
    registered(&sandbox, "orders", "postgres://app@127.0.0.1:1/orders");

    sandbox
        .sloop(&["db", "remove", "orders"])
        .expect_code(2)
        .expect_said("no terminal")
        .expect_said("--yes");

    sandbox
        .sloop(&["db", "drop", "orders"])
        .expect_code(2)
        .expect_said("no terminal")
        .expect_said("--confirm orders");

    // And neither of them changed anything on the way to refusing.
    assert!(written(&sandbox).contains("[databases.orders]"));
}

/// `remove` provably never touches a server — the first half of R8's "Done when".
///
/// The proof is the port. `127.0.0.1:1` has nothing listening on it, so any command that
/// opened a connection would have to fail or hang; `remove` finishes, which it can only do
/// by never having tried.
#[test]
fn remove_forgets_the_record_and_never_contacts_the_server() {
    let sandbox = Sandbox::new("db-remove");
    registered(&sandbox, "orders", "postgres://app@127.0.0.1:1/orders");
    registered(&sandbox, "other", "postgres://app@127.0.0.1:1/other");

    let run = sandbox.sloop(&["db", "remove", "orders", "--yes"]);
    run.expect_code(0)
        .expect_said("forgot")
        .expect_said("not touched");

    let file = written(&sandbox);
    assert!(!file.contains("[databases.orders]"), "{file}");
    assert!(
        file.contains("[databases.other]"),
        "it removed the wrong one:\n{file}"
    );

    // A second removal of the same name is an honest "no such thing" rather than a
    // success that did nothing.
    sandbox
        .sloop(&["db", "remove", "orders", "--yes"])
        .expect_code(2)
        .expect_said("no database is registered as orders");
}

/// `drop` refuses on a typo — the second half of R8's "Done when" — and does it without
/// contacting anything, which the unreachable port proves.
#[test]
fn drop_refuses_a_typo_before_it_opens_a_connection() {
    let sandbox = Sandbox::new("db-typo");
    registered(&sandbox, "orders", "postgres://app@127.0.0.1:1/orders");

    for typo in ["order", "Orders", "orders ", "orders2", ""] {
        let run = sandbox.sloop(&["db", "drop", "orders", "--confirm", typo]);
        run.expect_code(2)
            .expect_said("the database is orders")
            .expect_said("nothing was contacted");
    }

    // Still registered, and still pointing where it did.
    assert!(written(&sandbox).contains("[databases.orders]"));
}

/// **There is no flag that skips the typing.** Not `--yes`, not `--force`, and there is no
/// longer a `--no-backup` to hide behind either: the owner settled on 2026-09-16 that a drop
/// keeps nothing, so the only way through is the database's own name.
///
/// Checked on the shape of the refusal rather than on a real drop: with nothing listening,
/// every one of these fails before a socket would have opened, which is the point.
#[test]
fn drop_needs_the_name_and_no_flag_stands_in_for_it() {
    let sandbox = Sandbox::new("db-drop-flags");
    registered(&sandbox, "orders", "postgres://app@127.0.0.1:1/orders");

    for flags in [
        vec!["db", "drop", "orders"],
        vec!["db", "drop", "orders", "--yes"],
        vec!["db", "drop", "orders", "-y"],
        vec!["db", "drop", "orders", "--force"],
        vec!["db", "drop", "orders", "-y", "--force"],
    ] {
        sandbox
            .sloop(&flags)
            .expect_code(2)
            .expect_said("--confirm orders");
    }

    // `--confirm` correct, and the password route satisfied: it gets as far as the
    // connection and fails there, which is how we know the confirmation was accepted and
    // nothing before it stopped the run.
    sandbox
        .command(
            sandbox.work(),
            &["db", "drop", "orders", "--confirm", "orders"],
        )
        .env("PW", "whatever")
        .run()
        .expect_code(3)
        .expect_said("connection");

    // Without the variable it stops earlier, on the route rather than on the name — which
    // is R3's answer and not this command's, and worth pinning so the two stay distinct.
    sandbox
        .sloop(&["db", "drop", "orders", "--confirm", "orders"])
        .expect_code(2)
        .expect_said("PW is not set");

    assert!(written(&sandbox).contains("[databases.orders]"));
}

/// `db drop --help` has to say plainly that it is not `db remove`.
#[test]
fn the_two_commands_say_which_is_which() {
    let sandbox = Sandbox::new("db-which");

    sandbox
        .sloop(&["db", "drop", "--help"])
        .expect_code(0)
        .expect_said("destroys a database on the server")
        .expect_said("not `db remove`")
        .expect_said("cannot be undone");

    sandbox
        .sloop(&["db", "remove", "--help"])
        .expect_code(0)
        .expect_said("server is not touched");
}

/// **`db create` is refused unattended unless both secrets have a source**, and the refusal
/// names every flag that is missing rather than one at a time.
#[test]
fn create_unattended_names_every_flag_it_needs() {
    let sandbox = Sandbox::new("db-create-flags");

    let run = sandbox.sloop(&["db", "create", "orders", "--engine", "postgres"]);

    run.expect_code(2)
        .expect_said("--role-password-stdin")
        .expect_said("--superuser-password-command");
}

/// An engine sloop does not know is refused before anything is asked for.
#[test]
fn create_refuses_an_engine_it_does_not_know() {
    let sandbox = Sandbox::new("db-create-engine");

    sandbox
        .sloop(&["db", "create", "orders", "--engine", "sqlite"])
        .expect_code(2)
        .expect_said("sqlite is not an engine sloop knows");
}

/// A label already registered is refused, and `--force` is named as what replaces it.
#[test]
fn create_refuses_a_name_that_is_already_registered() {
    let sandbox = Sandbox::new("db-create-taken");
    registered(&sandbox, "orders", "postgres://app@db.internal:5432/orders");

    sandbox
        .sloop(&["db", "create", "orders", "--engine", "postgres"])
        .expect_code(2)
        .expect_said("already registered")
        .expect_said("--force");
}

/// `db create --help` has to say which of the two passwords sloop keeps.
#[test]
fn create_help_says_which_password_is_kept() {
    let sandbox = Sandbox::new("db-create-help");

    sandbox
        .sloop(&["db", "create", "--help"])
        .expect_code(0)
        .expect_said("used for one")
        .expect_said("never put in a command line")
        .expect_said("keyring");
}

/// **The two names on the server are asked for, and with no terminal the flags are the only
/// way to give them.** Reopened under rule 0b: `db create` used to make a user with the
/// database's name without a word about it — *"it must ask db user name"*.
///
/// What can be checked here is the half that has to keep working unattended, and the help
/// that tells somebody the questions exist at all. The prompts themselves need a console.
#[test]
fn creating_a_database_says_it_asks_what_the_database_and_its_user_are_called() {
    let sandbox = Sandbox::new("create-names");

    sandbox
        .sloop(&["help", "db", "create"])
        .expect_code(0)
        .expect_said("sloop asks for both")
        .expect_said("which user owns it")
        .expect_said("--database and --role say them up front");

    sandbox
        .sloop(&["db", "create", "--help"])
        .expect_code(0)
        .expect_said("Asked for when left out")
        .expect_said("--role <USER>");

    // With no terminal there is nothing to ask at, so the run gets as far as the flags it
    // is missing and stops there — the same sentence it gave before the prompts arrived.
    sandbox
        .sloop(&["db", "create", "orders", "--engine", "postgres"])
        .expect_code(2)
        .expect_said("--role-password-stdin")
        .expect_said("--superuser-password-command");
}

/// **Which registry a record went into, and why.** *"why it is getting registered globally
/// where i didn't pass the --global flag explicitly?"* — because there was no project to put
/// it in, which is the documented order and is now a sentence rather than something to work
/// out from a listing.
#[test]
fn a_registration_says_why_it_went_where_it_went() {
    let sandbox = Sandbox::new("registered-why");

    // No `.sloop` anywhere above the working directory, so: the global store.
    sandbox
        .sloop(&[
            "db",
            "add",
            "orders",
            "--url",
            "postgres://a@h/d",
            "--env",
            "PW",
        ])
        .expect_code(0)
        .expect_said("in the global registry")
        .expect_said("no .sloop at or above the working directory");

    // With a project, the same command says the project and why that one.
    let project = sandbox.make_dir("demo");
    sandbox.sloop_in(&project, &["init"]).expect_code(0);
    sandbox
        .sloop_in(
            &project,
            &[
                "db",
                "add",
                "here",
                "--url",
                "postgres://a@h/d",
                "--env",
                "PW",
            ],
        )
        .expect_code(0)
        .expect_said("in the project registry")
        .expect_said("the nearest .sloop");

    // And --global says so in as many words.
    sandbox
        .sloop_in(
            &project,
            &[
                "--global",
                "db",
                "add",
                "there",
                "--url",
                "postgres://a@h/d",
                "--env",
                "PW",
            ],
        )
        .expect_code(0)
        .expect_said("in the global registry")
        .expect_said("--global");
}

/// There is no flag that carries the new user's password, and the help says why.
#[test]
fn the_new_users_password_is_chosen_without_ever_reaching_argv() {
    let sandbox = Sandbox::new("create-password");

    sandbox
        .sloop(&["help", "db", "create"])
        .expect_code(0)
        .expect_said("yours to choose, and generated only when you do not")
        .expect_said("There is no flag that takes the password itself")
        .expect_said("readable in `ps`");

    sandbox
        .sloop(&["db", "create", "--help"])
        .expect_code(0)
        .expect_said("--role-password-command <COMMAND>");

    // A password manager answers for the pipe, so the superuser's may use standard input.
    sandbox
        .sloop(&[
            "db",
            "create",
            "orders",
            "--engine",
            "postgres",
            "--superuser-password-stdin",
            "--role-password-command",
            "echo chosen-by-me",
        ])
        .expect_code(3)
        .expect_silent_about("--role-password-stdin is missing");
}

// ---------------------------------------------------------------------------------------
// reaching it over SSH — R19e
// ---------------------------------------------------------------------------------------

/// **The whole path, through the real tables.** A database whose port is closed to
/// everything outside its own server is registered with `--ssh-host`, the five columns land
/// in `registered_database`, and every one of them comes back out unchanged.
#[test]
fn a_database_reached_over_ssh_is_registered_listed_and_read_back() {
    let sandbox = Sandbox::new("db-ssh-add");

    sandbox
        .sloop(&[
            "db",
            "add",
            "prod",
            // The address the SERVER sees, which is the sentence this feature turns on.
            "--url",
            "postgres://app@127.0.0.1:5432/orders",
            "--ssh-host",
            "bastion.internal",
            "--ssh-port",
            "2222",
            "--ssh-user",
            "deploy",
            "--ssh-identity",
            "/home/me/.ssh/id_ed25519",
            "--ssh-env",
            "SSH_KEY_PW",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(0)
        // And it says which way round the two addresses are, at the moment it writes them.
        .expect_said("127.0.0.1:5432 is as bastion.internal sees it");

    let file = written(&sandbox);
    for line in [
        "[databases.prod.ssh]",
        "host = \"bastion.internal\"",
        "port = 2222",
        "user = \"deploy\"",
        "identity = \"/home/me/.ssh/id_ed25519\"",
        "passphrase = \"${SSH_KEY_PW}\"",
    ] {
        assert!(
            file.contains(line),
            "{line} is not in the registry:\n{file}"
        );
    }

    // The listing says where it goes and how the key is opened, and neither is a value.
    sandbox
        .sloop(&["db", "list"])
        .expect_code(0)
        .expect_said("postgres://app@127.0.0.1:5432/orders")
        .expect_said("over deploy@bastion.internal:2222")
        .expect_said("the environment variable SSH_KEY_PW");
}

/// A registration that says nothing about SSH writes nothing about SSH. `Reach::Direct` is
/// the ordinary state, not a setting left blank — and the columns' own constraints say so.
#[test]
fn a_direct_registration_writes_no_ssh_at_all() {
    let sandbox = Sandbox::new("db-ssh-direct");
    registered(&sandbox, "orders", "postgres://app@db.internal/orders");

    let file = written(&sandbox);
    assert!(
        !file.contains(".ssh]"),
        "a direct registration named a server:\n{file}"
    );
}

/// `--no-ssh` puts a record back to a direct connection, and `db edit` changes one field of
/// a server without being told the rest.
#[test]
fn an_edit_moves_one_ssh_field_and_no_ssh_turns_the_tunnel_off() {
    let sandbox = Sandbox::new("db-ssh-edit");

    sandbox
        .sloop(&[
            "db",
            "add",
            "prod",
            "--url",
            "postgres://app@127.0.0.1/orders",
            "--ssh-host",
            "bastion.internal",
            "--ssh-user",
            "deploy",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(0);

    sandbox
        .sloop(&["db", "edit", "prod", "--ssh-port", "2222"])
        .expect_code(0);

    let file = written(&sandbox);
    assert!(file.contains("port = 2222"), "{file}");
    assert!(
        file.contains("user = \"deploy\""),
        "the account was dropped by an edit that did not mention it:\n{file}"
    );

    sandbox
        .sloop(&["db", "edit", "prod", "--no-ssh"])
        .expect_code(0);

    let file = written(&sandbox);
    assert!(
        !file.contains(".ssh]"),
        "--no-ssh left the tunnel behind:\n{file}"
    );
}

/// An SSH detail with no server to hang it off names the flag that would have given it one,
/// rather than guessing which machine was meant.
#[test]
fn an_ssh_flag_with_no_server_exits_two_and_names_the_flag() {
    let sandbox = Sandbox::new("db-ssh-orphan");

    sandbox
        .sloop(&[
            "db",
            "add",
            "prod",
            "--url",
            "postgres://app@127.0.0.1/orders",
            "--ssh-user",
            "deploy",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(2)
        .expect_said("--ssh-host");
}

/// Rule 4, on the second credential a record can hold: a passphrase sloop would have to
/// keep, with no terminal to ask at, exits `2` naming the flag — and it does not hang.
#[test]
fn a_passphrase_with_no_terminal_exits_two_and_names_the_flag() {
    let sandbox = Sandbox::new("db-ssh-no-tty");

    sandbox
        .sloop(&[
            "db",
            "add",
            "prod",
            "--url",
            "postgres://app@127.0.0.1/orders",
            "--ssh-host",
            "bastion.internal",
            "--ssh-keyring",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(2)
        .expect_said("--ssh-passphrase-stdin");

    assert!(
        !written(&sandbox).contains("prod"),
        "a registration that could not be completed was written anyway"
    );
}

/// A passphrase piped in is filed under the *server* and never written into the registry,
/// which is the whole of rule 3 applied to the second credential.
#[test]
fn a_piped_passphrase_reaches_the_store_and_never_the_registry() {
    let sandbox = Sandbox::new("db-ssh-piped");

    // Real punctuation, because a passphrase is allowed to hold it and nothing between the
    // pipe and the sealed store may interpret a byte of it.
    let passphrase = "key passphrase with >|$# \"quotes\" ";
    sandbox
        .command(
            sandbox.work(),
            &[
                "db",
                "add",
                "prod",
                "--url",
                "postgres://app@127.0.0.1/orders",
                "--ssh-host",
                "bastion.internal",
                "--ssh-encrypted-file",
                "--ssh-passphrase-stdin",
                "--env",
                "PGPASSWORD",
            ],
        )
        .stdin(format!("{passphrase}\n").as_bytes())
        .run()
        .expect_code(0)
        .expect_silent_about(passphrase.trim());

    let file = written(&sandbox);
    assert!(
        file.contains("passphrase = \"encrypted-file\""),
        "the route is what is written:\n{file}"
    );
    assert!(
        !file.contains(passphrase.trim()),
        "the passphrase reached the registry:\n{file}"
    );

    // It is in the sealed store, which is ciphertext — so the bytes are there and the
    // passphrase is not readable in them.
    let sealed = sandbox.sealed_bytes();
    assert!(!sealed.is_empty(), "nothing was sealed");
    assert!(
        !sealed
            .windows(passphrase.len())
            .any(|window| window == passphrase.as_bytes()),
        "the sealed store is not ciphertext"
    );
}

/// A server name `ssh` would read as an option is refused when it is written, rather than
/// becoming an argument on a command line later.
#[test]
fn a_server_name_that_would_become_an_ssh_option_is_refused() {
    let sandbox = Sandbox::new("db-ssh-bad-host");

    sandbox
        .sloop(&[
            "db",
            "add",
            "prod",
            "--url",
            "postgres://app@127.0.0.1/orders",
            "--ssh-host",
            "-oProxyCommand=id",
            "--env",
            "PGPASSWORD",
        ])
        .expect_code(2);

    assert!(!written(&sandbox).contains("prod"));
}

/// **The tunnel is tried, and nothing dials this machine's loopback.**
///
/// A tunnelled record's `host` is the address the *server* sees — usually `127.0.0.1` — so
/// a command that read it off the record would aim at this laptop: a connection error if
/// nothing is listening there, and something far worse if something is. There is no SSH
/// server called `bastion.internal`, so what every one of these has to produce is `ssh`
/// failing to reach it, exit `3`, and `ssh`'s own words. A message about *PostgreSQL*
/// refusing a connection would mean the address had been used raw.
///
/// The happy path needs a real server and is `ssh::tests`, behind `SLOOP_SSH_TEST`.
#[test]
fn a_command_that_needs_the_tunnel_goes_through_ssh_and_never_dials_locally() {
    let sandbox = Sandbox::new("db-ssh-tunnel");

    // A route that answers, so the run gets past the password and as far as the tunnel.
    // What is being tested is the order after that.
    let prints_it = if cfg!(windows) {
        "cmd /c echo pw"
    } else {
        "echo pw"
    };
    sandbox
        .sloop(&[
            "db",
            "add",
            "prod",
            "--url",
            "postgres://app@127.0.0.1:5432/orders",
            "--ssh-host",
            // Reserved for exactly this: RFC 2606 says it never resolves.
            "bastion.invalid",
            "--password-from",
            prints_it,
        ])
        .expect_code(0);

    // A direct database to copy into, so `mirror` gets past "the source is the destination"
    // and as far as the source it cannot reach.
    registered(&sandbox, "copy", "postgres://app@db.internal/orders");

    // `backup` stops at the backup-key question before it reaches any database, and that
    // ordering is right — so the question is answered here rather than reordered there.
    sandbox.sloop(&["key", "export"]).expect_code(0);

    for command in [
        vec!["db", "test", "prod"],
        vec!["backup", "prod"],
        // Rule 5 comes first on these two, so the name is typed up front and what is left
        // to fail is the source nobody can reach.
        vec!["mirror", "prod", "--to", "copy", "--confirm", "orders"],
        vec!["sync", "prod", "--to", "copy", "--confirm", "orders"],
    ] {
        let run = sandbox.sloop(&command);
        let said = format!("{}{}", run.stdout(), run.stderr());
        let shown = command.join(" ");

        assert_eq!(
            run.code(),
            Some(3),
            "`sloop {shown}` did not fail as a connection:\n{said}"
        );
        assert!(
            said.contains("bastion.invalid"),
            "`sloop {shown}` did not say which server it could not reach:\n{said}"
        );
        assert!(
            !said.contains("port 5432"),
            "`sloop {shown}` dialled this machine's loopback instead of a forward:\n{said}"
        );
    }

    // **And reading the registry needs no tunnel at all.** A database nobody can reach is
    // still a database somebody can list, rename and remove — opening a connection to say
    // what is registered would be the wrong shape entirely.
    sandbox
        .sloop(&["db", "list"])
        .expect_code(0)
        .expect_said("over bastion.invalid");
    sandbox
        .sloop(&["db", "rename", "prod", "old"])
        .expect_code(0);
    sandbox
        .sloop(&["db", "remove", "old", "--yes"])
        .expect_code(0);
}

/// **`doctor` reports on SSH only when something here needs it** — and then says everything
/// the entry asked for: whether `ssh` is there and which version, whether an agent is
/// running, whether each key named can be read, and which databases go through what.
#[test]
fn doctor_says_nothing_about_ssh_until_a_database_goes_through_it() {
    let sandbox = Sandbox::new("doctor-ssh");
    registered(&sandbox, "plain", "postgres://app@db.internal/orders");

    // Nothing here is reached over SSH, so a section about it would be a paragraph
    // everybody learns to scroll past.
    sandbox
        .sloop(&["doctor", "--offline"])
        .expect_code(0)
        .expect_silent_about("Over SSH");

    sandbox
        .sloop(&[
            "db",
            "add",
            "prod",
            "--url",
            "postgres://app@127.0.0.1/orders",
            "--ssh-host",
            "bastion.invalid",
            "--ssh-user",
            "deploy",
            "--ssh-identity",
            "/no/such/key",
            "--env",
            "PW",
        ])
        .expect_code(0);

    let run = sandbox.sloop(&["doctor", "--offline"]);
    run.expect_code(0)
        .expect_said("Over SSH")
        .expect_said("agent")
        // The key a record names, and the answer to the question people actually have.
        .expect_said("/no/such/key")
        .expect_said("cannot be read")
        .expect_said("prod")
        .expect_said("over deploy@bastion.invalid");
}
