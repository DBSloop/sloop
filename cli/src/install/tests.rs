//! Where an install goes, which port it takes, and what it refuses to do.
//!
//! **Nothing here downloads anything or starts anything.** What is worth testing without a
//! network is the part that decides: the port that collides with nothing, the record that
//! holds a route and never a password, and the two refusals that stand between a menu item
//! and something of the user's being overwritten. Driving a real install end to end is
//! `cli/tests/install.rs`, which needs a real archive and says so.

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use super::{Installed, home_for, port, record};
use crate::engine::Engine;

/// A global store on disk, thrown away when it goes out of scope.
struct Store(PathBuf);

impl Store {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "sloop-install-{}-{label}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a temporary directory should be creatable");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One installed server, pointed at a directory under `store`.
fn a_server(store: &Store, engine: Engine, version: &str, port: u16) -> Installed {
    let home = home_for(store.path(), engine, version);
    Installed {
        engine,
        version: version.to_owned(),
        bin: home.join("bin"),
        data: home.join("data"),
        home,
        port,
        superuser: super::superuser_of(engine).to_owned(),
    }
}

/// **The version is part of the directory name**, so installing 8.4 beside 9.5 is two
/// servers rather than the second one quietly replacing the first.
#[test]
fn two_versions_of_one_engine_live_in_two_directories() {
    let store = Store::new("two-versions");

    let older = home_for(store.path(), Engine::Mysql, "8.4.11");
    let newer = home_for(store.path(), Engine::Mysql, "9.5.0");

    assert_ne!(older, newer);
    assert!(older.ends_with("mysql-8.4.11"), "{}", older.display());
    assert!(newer.ends_with("mysql-9.5.0"), "{}", newer.display());
    assert_eq!(
        older.parent(),
        newer.parent(),
        "both under the one servers directory"
    );
}

/// **A port somebody else is holding is never offered.** This is the whole reason the
/// operating system is asked rather than an offset being guessed.
#[test]
fn a_port_in_use_is_stepped_over() {
    let taken = TcpListener::bind("127.0.0.1:0").expect("the kernel should hand out a port");
    let held = taken.local_addr().expect("it has an address").port();

    let chosen = port::choose(held, &[]).expect("something above it should be free");
    assert_ne!(chosen, held, "the port under a live listener was offered");
    assert!(chosen > held, "the search goes up from the one asked for");
}

/// **And neither is one this machine has already given out**, even with nothing listening on
/// it: a server sloop installed and then stopped still owns its port, and handing the same
/// one to the next install would collide the moment somebody started the first again.
#[test]
fn a_port_already_promised_to_a_stopped_server_is_stepped_over() {
    let free = TcpListener::bind("127.0.0.1:0").expect("a port");
    let number = free.local_addr().expect("an address").port();
    drop(free);

    let chosen = port::choose(number, &[number]).expect("something above it should be free");
    assert_ne!(chosen, number);
}

/// A machine with nothing in the way gets the port everybody expects, because somebody who
/// sees 5432 knows what it is.
#[test]
fn the_usual_port_is_the_first_choice_when_it_is_free() {
    let free = TcpListener::bind("127.0.0.1:0").expect("a port");
    let number = free.local_addr().expect("an address").port();
    drop(free);

    // Only meaningful while nothing has raced in and taken it, which is exactly the race the
    // module's header says the server starting is what settles.
    if port::is_free(number) {
        assert_eq!(port::choose(number, &[]).expect("free"), number);
    }
}

/// **Rule 3, and the record is where it would be broken.** What goes in the file is a route —
/// the word `keyring` — and there is no spelling of a password the shape would accept.
#[test]
fn the_record_holds_a_route_and_never_a_password() {
    let store = Store::new("route");
    let installed = a_server(&store, Engine::Mariadb, "11.4.4", 3307);
    let password = crate::secret::Secret::new("Sup3rSecretValue".to_owned());

    // The keyring is not available on every runner, so a failure to keep the secret is a
    // skip rather than a failure: what is being tested is what the file ends up holding.
    if record::remember(store.path(), &installed, &password).is_err() {
        eprintln!("skipping: this machine has nowhere to keep a secret");
        return;
    }

    let text = std::fs::read_to_string(store.path().join(record::FILE)).expect("it was written");
    assert!(
        !text.contains("Sup3rSecretValue"),
        "the password reached the file:\n{text}"
    );
    assert!(
        text.contains("password = \"keyring\"") || text.contains("password = \"encrypted-file\""),
        "the password field should be a route:\n{text}"
    );
    assert!(text.contains("port = 3307"), "{text}");
}

/// A server written down is a server that comes back, with its port spoken for.
#[test]
fn a_recorded_server_is_read_back_and_its_port_is_spoken_for() {
    let store = Store::new("round-trip");
    let installed = a_server(&store, Engine::Mysql, "8.4.11", 3306);
    let password = crate::secret::Secret::new(crate::secret::generated_password().expect("random"));

    if record::remember(store.path(), &installed, &password).is_err() {
        eprintln!("skipping: this machine has nowhere to keep a secret");
        return;
    }

    assert_eq!(record::ports(store.path()), vec![3306]);

    // `read` only offers servers still on disk, so nothing comes back until the directory
    // does — which is what stops a menu offering to start something somebody deleted.
    assert!(
        record::read(store.path()).expect("readable").is_empty(),
        "a record naming a directory that is gone should not be offered"
    );

    std::fs::create_dir_all(&installed.bin).expect("creatable");
    let back = record::read(store.path()).expect("readable");
    assert_eq!(back, vec![installed]);
}

/// **A record from a newer sloop is a refusal, not a misreading.** An engine it does not
/// speak is skipped; a file version it does not know stops it.
#[test]
fn a_record_from_a_newer_sloop_is_read_as_far_as_it_can_be() {
    let store = Store::new("newer");

    std::fs::write(
        store.path().join(record::FILE),
        "version = 1\n\
         [[server]]\n\
         engine = \"cockroach\"\n\
         version = \"24.1\"\n\
         home = \"/nowhere\"\n\
         bin = \"/nowhere/bin\"\n\
         data = \"/nowhere/data\"\n\
         port = 26257\n\
         superuser = \"root\"\n\
         password = \"keyring\"\n\
         installed_at = \"20260917T000000Z\"\n",
    )
    .expect("writable");

    assert!(
        record::read(store.path())
            .expect("an unknown engine is skipped, not fatal")
            .is_empty()
    );
    assert_eq!(
        record::ports(store.path()),
        vec![26257],
        "its port is still spoken for, whoever wrote it"
    );

    std::fs::write(store.path().join(record::FILE), "version = 99\n").expect("writable");
    let failure = record::read(store.path()).expect_err("a newer file version stops it");
    assert_eq!(failure.exit(), crate::exit::Exit::Usage);
}

/// **Nothing is ever installed over.** The two halves are checked separately because either
/// one on its own is what survives a failure: a record with no directory, or a directory
/// with no record.
#[test]
fn a_directory_that_is_already_there_is_never_written_into() {
    let store = Store::new("already");
    let build = crate::tools::catalogue::Build {
        engine: Engine::Mysql,
        version: "8.4.11".to_owned(),
        file_name: "mysql-8.4.11-winx64.zip".to_owned(),
        url: "https://example.test/mysql-8.4.11-winx64.zip".to_owned(),
        proof: crate::tools::proof::Proof::Published {
            sha256: "c".repeat(64),
            from: "example.test".to_owned(),
        },
    };

    let into = home_for(store.path(), Engine::Mysql, "8.4.11");
    std::fs::create_dir_all(into.join("bin")).expect("creatable");

    let failure = super::already_there(store.path(), &build, &into)
        .expect_err("a directory that is already there should stop it");
    assert_eq!(failure.exit(), crate::exit::Exit::Usage);
    assert!(
        failure.message().contains("already there"),
        "{}",
        failure.message()
    );
}

/// The administrative account is the conventional one for each family, so that everything a
/// person does with the server afterwards behaves the way every tutorial says it will.
#[test]
fn each_family_gets_the_account_name_its_documentation_uses() {
    assert_eq!(super::superuser_of(Engine::Postgres), "postgres");
    assert_eq!(super::superuser_of(Engine::Mysql), "root");
    assert_eq!(super::superuser_of(Engine::Mariadb), "root");
}

/// The URL an installed server is described by carries no password, because there is nowhere
/// in it one could go.
#[test]
fn how_a_server_is_described_carries_no_secret() {
    let store = Store::new("url");
    let installed = a_server(&store, Engine::Postgres, "18.6", 5433);

    assert_eq!(installed.url(), "postgres://postgres@127.0.0.1:5433/");
    assert_eq!(installed.describe(), "PostgreSQL 18.6");
    assert_eq!(installed.as_server().port, 5433);
    assert_eq!(
        installed.as_server().origin,
        crate::server::Origin::Sloops,
        "sloop made this cluster, so it is sloop's to start"
    );
}
