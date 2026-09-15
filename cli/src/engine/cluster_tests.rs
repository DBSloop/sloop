//! The PostgreSQL adapter against a real server: one created for the test, on a port
//! nothing else uses, destroyed again when the test finishes.
//!
//! Never the machine's own PostgreSQL. `initdb` builds a cluster in a temporary directory
//! with its own superuser, its own roles and its own data, and `Drop` stops it and deletes
//! it whether the test passed or not.
//!
//! Skipped with a printed reason where `initdb` cannot be found, which is the honest
//! outcome on a machine with only the client tools installed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};

use super::postgres::{Postgres, Tools};
use super::{Adapter, Engine, Target, Version};
use crate::secret::Secret;

/// High enough to be out of the way, and stepped per cluster so two tests never collide.
static NEXT_PORT: AtomicU16 = AtomicU16::new(55_431);

/// Deliberately awful, and different for each role — the point of two roles is that a dump
/// and a restore each use their own credentials.
const SUPERUSER_PASSWORD: &str = r#"sup3r\pass >w #1 "q" ;x"#;
const ALPHA_PASSWORD: &str = r#"alpha\pw $HOME #2 'q' "d" |p"#;
const BETA_PASSWORD: &str = r#"beta\pw >out #3 "q" &b"#;

/// A throwaway cluster.
struct Cluster {
    root: PathBuf,
    data: PathBuf,
    port: u16,
    binaries: PathBuf,
    running: bool,
}

impl Cluster {
    /// Build and start one, or `None` when this machine has no `initdb`.
    fn start(label: &str) -> Option<Self> {
        let binaries = find_server_binaries()?;

        sweep_stale_clusters(&binaries);

        let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("sloop-pg-{}-{label}-{port}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a temporary directory");

        let mut cluster = Self {
            data: root.join("data"),
            root,
            port,
            binaries,
            running: false,
        };

        cluster.initdb();
        cluster.launch();
        cluster.running = true;
        Some(cluster)
    }

    /// The path to one of the PostgreSQL programs. An empty directory means `PATH`.
    fn tool(&self, name: &str) -> PathBuf {
        self.binaries.join(name)
    }

    fn initdb(&self) {
        let password_file = self.root.join("superuser-password");
        std::fs::write(&password_file, SUPERUSER_PASSWORD).expect("writing the password file");

        let output = Command::new(self.tool("initdb"))
            .arg("--pgdata")
            .arg(&self.data)
            .arg("--username=sloop_super")
            .arg("--auth-host=scram-sha-256")
            .arg("--auth-local=trust")
            .arg("--pwfile")
            .arg(&password_file)
            .arg("--encoding=UTF8")
            .arg("--locale=C")
            // A cluster that is about to be deleted does not need to survive a power cut.
            .arg("--no-sync")
            .stdin(Stdio::null())
            .output()
            .expect("running initdb");

        assert!(
            output.status.success(),
            "initdb failed\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        // The password is only needed to create the cluster. It lives in the keyring-free
        // fixture from here on, so the file goes.
        let _ = std::fs::remove_file(&password_file);
    }

    /// Start the server.
    ///
    /// `status`, with every stream sent to null, and **not** `output`. `output` builds pipes
    /// for the child's stdout and stderr, and on Windows the postmaster that `pg_ctl` spawns
    /// inherits those handles and holds them open for as long as it runs — so the read that
    /// `output` does to collect them never reaches end of file, and the call blocks until the
    /// server dies. Which it will not, having just started.
    ///
    /// The server's own output goes to `--log`, which is where anything worth reading after a
    /// failure ends up anyway.
    fn launch(&self) {
        let status = Command::new(self.tool("pg_ctl"))
            .arg("--pgdata")
            .arg(&self.data)
            .arg("--log")
            .arg(self.root.join("server.log"))
            .arg("--options")
            .arg(format!(
                "-p {} -c listen_addresses=127.0.0.1 -c fsync=off -c max_connections=40",
                self.port
            ))
            // Bounded, so a cluster that will not come up fails the test instead of
            // hanging it.
            .arg("--timeout=60")
            .arg("--wait")
            .arg("start")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("running pg_ctl start");

        assert!(
            status.success(),
            "pg_ctl start failed on port {}\n--- server log ---\n{}",
            self.port,
            std::fs::read_to_string(self.root.join("server.log")).unwrap_or_default()
        );
    }

    /// Run SQL as the superuser, against `database`.
    fn psql(&self, database: &str, sql: &str) {
        let output = Command::new(self.tool("psql"))
            .arg("--host=127.0.0.1")
            .arg(format!("--port={}", self.port))
            .arg("--username=sloop_super")
            .arg(format!("--dbname={database}"))
            // Never prompt. Without this, a password psql does not like makes it wait for
            // one forever — which is exactly how a seven-minute silence happens.
            .arg("--no-password")
            .arg("--no-psqlrc")
            .arg("--quiet")
            .arg("--variable=ON_ERROR_STOP=1")
            .arg("--command")
            .arg(sql)
            .env("PGPASSWORD", SUPERUSER_PASSWORD)
            .stdin(Stdio::null())
            .output()
            .expect("running psql");

        assert!(
            output.status.success(),
            "psql failed: {sql}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Two roles with different passwords, two databases, and real rows in one of them.
    fn seed(&self) {
        self.psql(
            "postgres",
            &format!(
                "CREATE ROLE alpha LOGIN PASSWORD {}; \
                 CREATE ROLE beta LOGIN PASSWORD {};",
                sql_literal(ALPHA_PASSWORD),
                sql_literal(BETA_PASSWORD)
            ),
        );
        self.psql("postgres", "CREATE DATABASE source_db OWNER alpha;");
        self.psql("postgres", "CREATE DATABASE target_db OWNER beta;");

        self.psql(
            "source_db",
            "CREATE SCHEMA app AUTHORIZATION alpha; \
             CREATE TABLE app.widgets (id serial PRIMARY KEY, name text NOT NULL); \
             INSERT INTO app.widgets (name) SELECT 'widget ' || g FROM generate_series(1, 500) g; \
             CREATE TABLE public.notes (id int PRIMARY KEY, body text); \
             INSERT INTO public.notes SELECT g, 'note ' || g FROM generate_series(1, 7) g; \
             CREATE VIEW app.recent AS SELECT * FROM app.widgets WHERE id > 490; \
             ALTER SCHEMA public OWNER TO alpha; \
             ALTER TABLE public.notes OWNER TO alpha; \
             ALTER TABLE app.widgets OWNER TO alpha; \
             ALTER SEQUENCE app.widgets_id_seq OWNER TO alpha;",
        );
        // The destination starts empty and owned by someone else entirely.
        self.psql("target_db", "ALTER SCHEMA public OWNER TO beta;");
    }

    fn database(&self, name: &str, user: &str) -> Connection {
        Connection {
            host: "127.0.0.1".to_owned(),
            port: self.port,
            database: name.to_owned(),
            user: user.to_owned(),
        }
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        if self.running {
            let _ = Command::new(self.tool("pg_ctl"))
                .arg("--pgdata")
                .arg(&self.data)
                .arg("--mode=immediate")
                .arg("--timeout=60")
                .arg("--wait")
                .arg("stop")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        // Best effort: on Windows a file handle can outlive the process by a moment, and a
        // test that has already passed should not fail over a temporary directory.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The parts of a connection the adapter needs, owned so a `Target` can borrow them.
struct Connection {
    host: String,
    port: u16,
    database: String,
    user: String,
}

impl Connection {
    fn target<'a>(&'a self, password: &'a Secret) -> Target<'a> {
        Target {
            engine: Engine::Postgres,
            host: &self.host,
            port: self.port,
            database: &self.database,
            user: &self.user,
            password,
        }
    }
}

/// Quote a string for SQL. Doubling the apostrophes is the whole rule.
fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Stop and delete any cluster a previous run left behind.
///
/// A test process that is killed — by a timeout, by Ctrl-C, by a harness that loses its
/// result — never runs `Drop`, and a PostgreSQL server that outlives it holds a port and
/// eats memory until somebody notices. Sweeping on the way in means a killed run repairs
/// itself on the next one instead of needing a person.
///
/// Only other processes' leftovers: anything belonging to this one is in use.
fn sweep_stale_clusters(binaries: &Path) {
    let mine = format!("sloop-pg-{}-", std::process::id());
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("sloop-pg-") || name.starts_with(&mine) {
            continue;
        }

        let data = entry.path().join("data");
        if data.join("postmaster.pid").is_file() {
            let _ = Command::new(binaries.join("pg_ctl"))
                .arg("--pgdata")
                .arg(&data)
                .arg("--mode=immediate")
                .arg("--timeout=20")
                .arg("--wait")
                .arg("stop")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

/// Where the server programs are, or `None` if this machine has none.
///
/// `PATH` first. Then the places a package manager puts them, because `initdb` is in the
/// server package and distributions leave it off `PATH` — a CI runner with PostgreSQL
/// installed would otherwise look like a machine without it.
fn find_server_binaries() -> Option<PathBuf> {
    if Command::new("initdb")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return Some(PathBuf::new());
    }

    let roots = [
        "/usr/lib/postgresql",
        "/usr/local/pgsql",
        "/opt/homebrew/opt",
        "/usr/local/opt",
        "C:/Program Files/PostgreSQL",
    ];

    let mut found: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join("bin");
            if candidate.join(initdb_file_name()).is_file() {
                found.push(candidate);
            }
        }
    }

    // Newest last, so the highest version wins. Version directories sort well enough for
    // this: what matters is that something is found at all.
    found.sort();
    found.pop()
}

fn initdb_file_name() -> &'static str {
    if cfg!(windows) {
        "initdb.exe"
    } else {
        "initdb"
    }
}

fn skip(reason: &str) {
    eprintln!("skipping the PostgreSQL cluster test: {reason}");
}

fn counts(adapter: &Postgres, target: &Target<'_>) -> Vec<(String, u64)> {
    adapter
        .row_counts(target)
        .expect("counting rows")
        .into_iter()
        .map(|count| (count.table.to_string(), count.rows))
        .collect()
}

/// The whole of R4's "done when", in order, against one cluster.
#[test]
fn a_dump_by_one_role_restores_under_another_and_the_counts_match() {
    let Some(cluster) = Cluster::start("roundtrip") else {
        skip("initdb is not on this machine");
        return;
    };
    cluster.seed();

    let adapter = Postgres::default();
    let alpha_password = Secret::new(ALPHA_PASSWORD.to_owned());
    let beta_password = Secret::new(BETA_PASSWORD.to_owned());
    let source = cluster.database("source_db", "alpha");
    let destination = cluster.database("target_db", "beta");
    let source_target = source.target(&alpha_password);
    let destination_target = destination.target(&beta_password);

    // --- connect, as two different roles with two different passwords ---------------
    let server = adapter.probe(&source_target).expect("probing as alpha");
    assert_eq!(server.engine, Engine::Postgres);
    assert!(
        server.version.major >= 9,
        "implausible version {}",
        server.version
    );
    // The cluster has no certificate, so a `prefer` connection must fall back to plain
    // rather than refuse. That fallback is the whole of "TLS when offered, plain when not".
    assert!(!server.tls, "a cluster with no TLS reported TLS");

    let destination_server = adapter.probe(&destination_target).expect("probing as beta");
    assert_eq!(destination_server.version, server.version);

    // --- what is there, and exactly how much of it ----------------------------------
    let tables = adapter.tables(&source_target).expect("listing tables");
    let listed: Vec<String> = tables.iter().map(ToString::to_string).collect();
    assert!(listed.contains(&"app.widgets".to_owned()), "{listed:?}");
    assert!(listed.contains(&"public.notes".to_owned()), "{listed:?}");
    // A view is not a table and must not be counted as one.
    assert!(!listed.contains(&"app.recent".to_owned()), "{listed:?}");

    let before = counts(&adapter, &source_target);
    assert_eq!(
        before,
        vec![
            ("app.widgets".to_owned(), 500),
            ("public.notes".to_owned(), 7),
        ],
        "exact counts, not estimates"
    );

    // The destination is empty to begin with.
    assert_eq!(counts(&adapter, &destination_target), Vec::new());

    // --- dump, as alpha --------------------------------------------------------------
    let dump_path = cluster.root.join("source.dump");
    let summary = adapter
        .dump(&source_target, &dump_path)
        .expect("dumping as alpha");
    assert!(summary.bytes > 0, "the dump is empty");
    assert!(dump_path.is_file());
    // Custom format, so pg_restore can read it selectively. The header says `PGDMP`.
    let head = std::fs::read(&dump_path).expect("reading the dump");
    assert_eq!(&head[..5], b"PGDMP", "not a custom-format dump");

    // Nothing was written to the source: it still has exactly what it had.
    assert_eq!(counts(&adapter, &source_target), before);

    // --- restore, as beta, into a database alpha has no rights over -------------------
    adapter
        .restore(&destination_target, &dump_path)
        .expect("restoring as beta");

    let after = counts(&adapter, &destination_target);
    assert_eq!(after, before, "the copy does not match the source");
}

/// The paths that go wrong, on the same kind of cluster.
#[test]
fn the_failures_are_told_apart_and_carry_the_right_codes() {
    let Some(cluster) = Cluster::start("failures") else {
        skip("initdb is not on this machine");
        return;
    };
    cluster.seed();

    let adapter = Postgres::default();
    let source = cluster.database("source_db", "alpha");
    let right = Secret::new(ALPHA_PASSWORD.to_owned());

    // --- a wrong password is a connection failure, code 3 ----------------------------
    let wrong = Secret::new("not the password".to_owned());
    let failure = adapter
        .probe(&source.target(&wrong))
        .expect_err("a wrong password must not connect");
    assert_eq!(failure.exit().code(), 3, "{failure:?}");
    assert!(
        !failure.message().contains("not the password"),
        "the password leaked into the error: {failure:?}"
    );

    // --- a password that is right still works, so the test above proves something -----
    adapter
        .probe(&source.target(&right))
        .expect("the right password connects");

    // --- nothing listening is also code 3 --------------------------------------------
    let nowhere = Connection {
        host: "127.0.0.1".to_owned(),
        port: 1,
        database: "source_db".to_owned(),
        user: "alpha".to_owned(),
    };
    let failure = adapter
        .probe(&nowhere.target(&right))
        .expect_err("port 1 has nothing on it");
    assert_eq!(failure.exit().code(), 3, "{failure:?}");

    // --- a client older than the server is refused before anything is written ---------
    let server = adapter.probe(&source.target(&right)).expect("probing");
    let old = Postgres::default().pretending_to_be(Version::new(server.version.major - 1, 0));
    let dump_path = cluster.root.join("never-written.dump");
    let failure = old
        .dump(&source.target(&right), &dump_path)
        .expect_err("an old pg_dump must be refused");

    assert_eq!(failure.exit().code(), 2, "{failure:?}");
    assert!(failure.message().contains("older pg_dump"), "{failure:?}");
    assert!(
        !dump_path.exists(),
        "it was refused and yet a file appeared: {}",
        dump_path.display()
    );

    // --- restoring a dump that is not there is code 5 ---------------------------------
    let failure = adapter
        .restore(
            &cluster.database("target_db", "beta").target(&right),
            Path::new("no-such.dump"),
        )
        .expect_err("there is no such dump");
    assert_eq!(failure.exit().code(), 5, "{failure:?}");
}

/// A tool that is not installed says so, and says where to look. No cluster needed.
#[test]
fn a_missing_client_tool_is_a_sentence_and_not_a_panic() {
    let adapter = Postgres::new(Tools {
        dump: PathBuf::from("pg_dump-that-is-not-installed"),
        restore: PathBuf::from("pg_restore-that-is-not-installed"),
        query: PathBuf::from("psql-that-is-not-installed"),
    });

    let failure = adapter.client_version().expect_err("it is not there");
    assert_eq!(failure.exit().code(), 2);
    assert!(failure.message().contains("pg_dump-that-is-not-installed"));
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("not bundled")),
        "it has to say why sloop does not have one"
    );
}
