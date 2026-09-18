//! The decisions in this module that can be checked without a PostgreSQL.
//!
//! What needs one is `cluster_tests`, which builds a throwaway cluster and drives the real
//! `initdb`, `pg_ctl` and `psql`.

use std::path::{Path, PathBuf};

use super::{Origin, Server, record};
use crate::secret::{Route, Secret};

/// A temporary directory of this test's own, removed when it goes out of scope.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "sloop-server-{}-{label}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a temporary directory should be creatable");
        Self(root)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Can this machine keep a secret at all?
///
/// **A fact about the machine, not about the code under test.** `keep_somewhere` uses the two
/// stores that hold something — the OS keyring, and the Argon2id file that `SLOOP_PASSPHRASE`
/// unlocks. A Linux box with no Secret Service running and no passphrase set has neither, and
/// that is exactly what a CI runner is. A test that needed one and never said so is how the
/// build stayed red for twelve commits once already.
///
/// Rule 3's half of these tests is not lost where this is false: the file's password field is
/// a [`Route`], and `Route::parse` refuses anything that is not one of the four — which
/// `secret`'s own tests prove on every platform.
pub(crate) fn this_machine_can_keep_a_secret(global: &Path) -> bool {
    let kept = crate::secret::keep_somewhere(
        "sloop-server:can-this-machine-keep-a-secret",
        &Secret::new("a throwaway probe".to_owned()),
        &crate::secret::sealed::Vault::File(&global.join(crate::registry::file::SEALED_FILE)),
    );

    if kept.is_err() {
        eprintln!(
            "skipping: this machine has no keyring and no SLOOP_PASSPHRASE, so sloop cannot              keep a password anywhere"
        );
    }
    kept.is_ok()
}

fn a_server() -> Server {
    Server {
        bin: PathBuf::from("/opt/pg18/bin"),
        data: Some(PathBuf::from("/home/me/.sloop/postgres/data")),
        port: super::SLOOPS_PORT,
        superuser: super::SUPERUSER.to_owned(),
        origin: Origin::Sloops,
    }
}

#[test]
fn sloops_own_cluster_is_on_its_own_port_and_never_5432() {
    assert_eq!(super::SLOOPS_PORT, 5433);
    assert_ne!(super::SLOOPS_PORT, 5432);
}

/// A connection string with no password in it, which is what makes it printable.
#[test]
fn the_url_names_the_server_and_carries_no_secret() {
    assert_eq!(
        a_server().url("sloop_database"),
        "postgres://postgres@127.0.0.1:5433/sloop_database"
    );
}

/// Rule 3 by construction: the file has a `password` field and the only thing it can hold
/// is the *name* of a route.
#[test]
fn the_record_holds_a_route_and_never_a_password() {
    let scratch = Scratch::new("record");
    let global = scratch.path();

    if !this_machine_can_keep_a_secret(global) {
        return;
    }
    let server = a_server();
    let password = Secret::new("aPasswordNobodyShouldEverSee".to_owned());

    record::write(global, &server, &password).expect("a machine can keep a secret somewhere");

    let written = std::fs::read_to_string(global.join(record::FILE)).expect("it was written");
    assert!(
        !written.contains("aPasswordNobodyShouldEverSee"),
        "the password reached the file:\n{written}"
    );
    assert!(written.contains("5433"), "{written}");
    assert!(written.contains("postgres"), "{written}");

    // And what it does hold parses back as a route, which is the type that cannot be a
    // password — `Route::parse` refuses anything that is not one of the four.
    let route = written
        .lines()
        .find_map(|line| line.strip_prefix("password = "))
        .map(|value| value.trim().trim_matches('"').to_owned())
        .expect("the file names a route");
    assert!(Route::parse(&route).is_ok(), "{route} is not a route");
}

/// A record naming a PostgreSQL that has been uninstalled is a reason to set up again, not
/// a reason to fail — the alternative is a machine that cannot run any sloop command until
/// somebody finds and deletes a file.
#[test]
fn a_record_naming_binaries_that_are_gone_starts_over_rather_than_failing() {
    let scratch = Scratch::new("stale");
    let global = scratch.path();

    if !this_machine_can_keep_a_secret(global) {
        return;
    }

    record::write(global, &a_server(), &Secret::new("whatever".to_owned()))
        .expect("a machine can keep a secret somewhere");

    let reopened = record::reopen(global).expect("a missing PostgreSQL is not an error");
    assert!(reopened.is_none(), "it should have started over");
}

#[test]
fn no_record_at_all_is_not_an_error() {
    let scratch = Scratch::new("absent");
    assert!(
        record::reopen(scratch.path())
            .expect("a machine before Setup is not an error")
            .is_none()
    );
}

/// A file from a newer sloop is refused rather than half-read. Reading a shape this build
/// does not know would mean connecting somewhere on a guess.
#[test]
fn a_record_from_a_newer_sloop_is_refused() {
    let scratch = Scratch::new("future");
    let global = scratch.path();
    std::fs::write(
        global.join(record::FILE),
        "version = 99\nbin = \"/opt/pg/bin\"\nport = 5433\n\
         superuser = \"postgres\"\npassword = \"keyring\"\norigin = \"sloop\"\n",
    )
    .expect("writable");

    let failure = record::reopen(global).expect_err("a newer file is not readable");
    assert_eq!(failure.exit().code(), 2);
    assert!(
        failure.message().contains("newer sloop"),
        "{}",
        failure.message()
    );
}

/// A field that is not in the shape is an error rather than a silent default, the same rule
/// the registry file follows: a typo that quietly changes which port sloop connects to is
/// the worst kind of quiet.
#[test]
fn a_typo_in_the_record_is_an_error_rather_than_a_silent_default() {
    let scratch = Scratch::new("typo");
    let global = scratch.path();
    std::fs::write(
        global.join(record::FILE),
        "version = 1\nbin = \"/opt/pg/bin\"\nprot = 5433\n\
         superuser = \"postgres\"\npassword = \"keyring\"\norigin = \"sloop\"\n",
    )
    .expect("writable");

    let failure = record::reopen(global).expect_err("prot is not port");
    assert_eq!(failure.exit().code(), 2);
}

/// A password somebody chose goes into the statement escaped, not refused.
///
/// **`R19f` is what made this reachable.** `--db-password-command` hands back whatever a
/// password manager holds, and a real one has apostrophes in it — so `literal` doubles them
/// the way `db create` has always doubled them, and the quote cannot close the literal.
#[test]
fn a_password_with_a_quote_in_it_is_escaped_rather_than_refused() {
    let awkward = crate::secret::Secret::new("o'brien';DROP TABLE x;--".to_owned());
    assert_eq!(
        super::make::literal(&awkward).expect("a real password is not a refusal"),
        "'o''brien'';DROP TABLE x;--'"
    );
}

/// An empty one still is a refusal: a role that can log in is never given a blank password.
#[test]
fn an_empty_password_never_reaches_a_statement() {
    let nothing = crate::secret::Secret::new(String::new());
    let failure = super::make::literal(&nothing).expect_err("empty is refused");
    assert!(failure.message().contains("empty"), "{}", failure.message());
}

/// The generated password needs no quoting at all — it pastes into a URL, a YAML file and a
/// shell without one escaping rule between them, which is what the alphabet is chosen for.
#[test]
fn a_generated_superuser_password_needs_no_quoting() {
    for _ in 0..25 {
        let password = crate::secret::generated_password().expect("the OS has randomness");
        assert_eq!(password.chars().count(), 28, "{password}");
        assert!(
            password.chars().all(|c| c.is_ascii_alphanumeric()),
            "{password} would need quoting inside an SQL literal"
        );
    }
}

/// Where everything lives, relative to the global store — so that uninstalling sloop is
/// deleting one directory and nothing is left on the machine.
#[test]
fn everything_sloop_installs_is_under_the_global_store() {
    let global = Path::new("/home/me/.sloop");

    for path in [
        super::home(global),
        super::data_dir(global),
        super::fetched_dir(global),
        super::fetched_bin(global),
    ] {
        assert!(
            path.starts_with(global),
            "{} is outside the global store",
            path.display()
        );
    }
}

// ---------------------------------------------------------------------------------------
// R19c2 — the record's half
// ---------------------------------------------------------------------------------------

/// Setup is re-runnable, and re-running it must not forget which database sloop's state is
/// in. `write` is what a second Setup calls, and it used to overwrite the whole file.
#[test]
fn writing_the_server_again_keeps_the_database_it_already_knew() {
    let scratch = Scratch::new("keeps-database");
    let global = scratch.path();
    if !this_machine_can_keep_a_secret(global) {
        return;
    }

    let server = a_server();
    let password = Secret::new("aPasswordNobodyShouldEverSee".to_owned());
    record::write(global, &server, &password).expect("it can keep a secret");

    let own = super::own::Own::sloops();
    record::remember_database(global, &server, &own, &password).expect("it can keep a secret");
    assert!(
        record::database_route(global)
            .expect("it reads back")
            .is_some()
    );

    // Setup, run again.
    record::write(global, &server, &password).expect("it can keep a secret");

    assert!(
        record::database_route(global)
            .expect("it reads back")
            .is_some(),
        "re-running Setup forgot where sloop's state lives"
    );
}

/// A machine that has a server but no database yet — every machine between `R19c1` and the
/// first `R19c2` run, and every one where Setup stopped in between.
#[test]
fn a_record_without_a_database_section_is_not_an_error() {
    let scratch = Scratch::new("no-database");
    let global = scratch.path();
    if !this_machine_can_keep_a_secret(global) {
        return;
    }

    record::write(global, &a_server(), &Secret::new("whatever".to_owned()))
        .expect("it can keep a secret");

    assert_eq!(record::database_route(global).expect("it reads back"), None);
}

/// The names are the owner's, spelled once. A test rather than a comment because `R19c3`'s
/// migrations and `R19c4`'s registry will both name them, and a typo in one of those is a
/// second database nobody asked for.
#[test]
fn sloops_own_database_and_role_are_named_what_the_owner_asked_for() {
    assert_eq!(super::own::DATABASE, "sloop_database");
    assert_eq!(super::own::ROLE, "sloop_db_admin");

    let own = super::own::Own::sloops();
    let server = a_server();
    assert_eq!(
        own.url(&server),
        "postgres://sloop_db_admin@127.0.0.1:5433/sloop_database"
    );
    // The key names the connection, so two clusters on one machine keep two passwords.
    assert!(own.credential_key(&server).contains("5433"));
}

/// Rule 3, on the second password as well as the first.
#[test]
fn the_database_section_holds_a_route_and_never_a_password() {
    let scratch = Scratch::new("database-route");
    let global = scratch.path();
    if !this_machine_can_keep_a_secret(global) {
        return;
    }

    let server = a_server();
    let secret = Secret::new("theRolePasswordNobodyShouldSee".to_owned());
    record::write(global, &server, &Secret::new("superuser".to_owned()))
        .expect("it can keep a secret");
    record::remember_database(global, &server, &super::own::Own::sloops(), &secret)
        .expect("it can keep a secret");

    let written = std::fs::read_to_string(global.join(record::FILE)).expect("it was written");
    assert!(
        !written.contains("theRolePasswordNobodyShouldSee"),
        "the role's password reached the file:\n{written}"
    );
    assert!(written.contains("sloop_database"), "{written}");
    assert!(written.contains("sloop_db_admin"), "{written}");
}
