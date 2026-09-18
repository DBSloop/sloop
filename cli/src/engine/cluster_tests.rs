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
use super::{Adapter, Cell, Engine, Reading, Target, Version};
use crate::secret::Secret;

/// Out of the way, and stepped per cluster so two tests never collide.
///
/// **Below 32768**, for the reason `server::cluster_tests` gives at its own ports: 32768 and
/// up is the ephemeral range on Linux, so a fixed port in it can be taken by an outbound
/// connection the suite itself made. Its own block, so the three harnesses cannot overlap.
///
/// The range is the belt and [`a_free_port`] is the braces: two test binaries running at
/// once still have to agree, and only asking the operating system can settle that.
static NEXT_PORT: AtomicU16 = AtomicU16::new(24_201);

/// A port in this file's block that nothing is listening on.
fn a_free_port() -> u16 {
    for _ in 0..200 {
        let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port in the range these tests use");
}

/// Deliberately awful, and different for each role — the point of two roles is that a dump
/// and a restore each use their own credentials.
pub(crate) const SUPERUSER_PASSWORD: &str = r#"sup3r\pass >w #1 "q" ;x"#;
pub(crate) const ALPHA_PASSWORD: &str = r#"alpha\pw $HOME #2 'q' "d" |p"#;
pub(crate) const BETA_PASSWORD: &str = r#"beta\pw >out #3 "q" &b"#;

/// A throwaway cluster.
///
/// Reachable from anywhere in the crate's tests: `commands::backup` needs a real server
/// too, and a second harness would be a second thing to keep working.
pub(crate) struct Cluster {
    pub(crate) root: PathBuf,
    data: PathBuf,
    pub(crate) port: u16,
    binaries: PathBuf,
    running: bool,
}

impl Cluster {
    /// Build, start and seed one, or `None` when this machine cannot host a server.
    ///
    /// Everything up to and including the fixture happens here, so a test is handed a
    /// cluster that is ready or handed nothing at all. There is no half-built state for an
    /// assertion to trip over and report as a bug.
    pub(crate) fn start(label: &str) -> Option<Self> {
        let (binaries, asked) = find_server_binaries()?;

        announce(&binaries);
        sweep_stale_clusters(&binaries);

        let port = a_free_port();
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

        if let Err(why) = cluster.initdb() {
            return refuse_or_skip(asked, &why);
        }
        if let Err(why) = cluster.launch() {
            return refuse_or_skip(asked, &why);
        }
        cluster.running = true;

        if let Err(why) = cluster.seed() {
            return refuse_or_skip(asked, &why);
        }

        Some(cluster)
    }

    /// The client tools that belong to *this* server.
    ///
    /// Two reasons, and the second is the one that matters. A runner can have a PostgreSQL
    /// server installed without its client tools on `PATH`, and then the adapter cannot find
    /// `pg_dump` even though the cluster started — which looks like a failed assertion rather
    /// than a machine without the tools. And the version matrix is pointless otherwise: it
    /// would exercise whatever `pg_dump` is on `PATH` against each server in turn, instead of
    /// version N's client against version N's server, which is the whole question.
    pub(crate) fn tools(&self) -> Tools {
        Tools {
            dump: self.tool("pg_dump"),
            restore: self.tool("pg_restore"),
            query: self.tool("psql"),
        }
    }

    /// The path to one of the PostgreSQL programs. An empty directory means `PATH`.
    fn tool(&self, name: &str) -> PathBuf {
        self.binaries.join(name)
    }

    fn initdb(&self) -> Result<(), String> {
        let password_file = self.root.join("superuser-password");
        std::fs::write(&password_file, SUPERUSER_PASSWORD).expect("writing the password file");

        let built = Command::new(self.tool("initdb"))
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
            .output();

        // The password was only needed to create the cluster; the file goes either way.
        let _ = std::fs::remove_file(&password_file);

        let Ok(output) = built else {
            return Err(format!("{} would not run", self.tool("initdb").display()));
        };

        if output.status.success() {
            return Ok(());
        }

        // The usual reason on a hosted runner: initdb refuses to run as an administrator,
        // which is exactly what a Windows CI runner is.
        Err(format!(
            "initdb refused: {}",
            first_line(&output.stdout, &output.stderr)
        ))
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
    fn launch(&self) -> Result<(), String> {
        let status = Command::new(self.tool("pg_ctl"))
            .arg("--pgdata")
            .arg(&self.data)
            .arg("--log")
            .arg(self.root.join("server.log"))
            .arg("--options")
            .arg(self.server_options())
            // Bounded, so a cluster that will not come up fails the test instead of
            // hanging it.
            .arg("--timeout=60")
            .arg("--wait")
            .arg("start")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if status.is_ok_and(|status| status.success()) {
            return Ok(());
        }

        Err(format!(
            "the server would not start on port {}: {}",
            self.port,
            std::fs::read_to_string(self.root.join("server.log"))
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .rfind(|line| !line.is_empty())
                .unwrap_or("it left no log")
                .to_owned()
        ))
    }

    /// The settings the throwaway server runs with.
    ///
    /// The socket directory is the interesting one. Debian and Ubuntu build PostgreSQL with
    /// `unix_socket_directories` defaulting to `/var/run/postgresql`, which belongs to the
    /// `postgres` user — so on a CI runner the server refuses to start, and the message is
    /// about a socket rather than about a permission. Pointing it inside the cluster's own
    /// directory sidesteps that. Not on Windows, which has no such socket.
    fn server_options(&self) -> String {
        let mut options = format!(
            "-p {} -c listen_addresses=127.0.0.1 -c fsync=off -c max_connections=40",
            self.port
        );
        if !cfg!(windows) {
            use std::fmt::Write as _;
            let _ = write!(
                options,
                " -c unix_socket_directories={}",
                self.root.display()
            );
        }
        options
    }

    /// Run SQL as the superuser, against `database`.
    ///
    /// Fallible, because building the fixture is not what these tests are about. A server
    /// that will not take `CREATE ROLE` is a machine that cannot host the fixture, and that
    /// is a skip — the assertions further down are where a real bug shows up.
    pub(crate) fn psql(&self, database: &str, sql: &str) -> Result<(), String> {
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
            .output();

        match output {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => Err(format!(
                "the fixture would not build: {}",
                first_line(&output.stdout, &output.stderr)
            )),
            Err(error) => Err(format!("psql would not run: {error}")),
        }
    }

    /// Two roles with different passwords, two databases, and real rows in one of them.
    fn seed(&self) -> Result<(), String> {
        self.psql(
            "postgres",
            &format!(
                "CREATE ROLE alpha LOGIN PASSWORD {}; \
                 CREATE ROLE beta LOGIN PASSWORD {};",
                sql_literal(ALPHA_PASSWORD),
                sql_literal(BETA_PASSWORD)
            ),
        )?;
        self.psql("postgres", "CREATE DATABASE source_db OWNER alpha;")?;
        self.psql("postgres", "CREATE DATABASE target_db OWNER beta;")?;

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
        )?;
        // The destination starts empty and owned by someone else entirely.
        self.psql("target_db", "ALTER SCHEMA public OWNER TO beta;")
    }

    pub(crate) fn database(&self, name: &str, user: &str) -> Connection {
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
pub(crate) struct Connection {
    host: String,
    port: u16,
    database: String,
    user: String,
}

impl Connection {
    pub(crate) fn target<'a>(&'a self, password: &'a Secret) -> Target<'a> {
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

/// Why these tests have a server to talk to at all.
///
/// The distinction decides what a setup failure means. Asked for one by name and it will
/// not run: that is a red test, because somebody wanted that server exercised. Found one
/// lying around and it will not run: that is a skip, because nobody promised this machine
/// could host a database — a hosted runner will not let `initdb` run as an administrator,
/// and a preinstalled PostgreSQL there is scenery rather than a server.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// `SLOOP_TEST_PG_BIN` named it.
    Explicitly,
    /// It turned up on `PATH` or in a package manager's directory.
    ByLookingAround,
}

/// Skip, or fail — whichever the caller earned.
fn refuse_or_skip(asked: Asked, why: &str) -> Option<Cluster> {
    assert!(
        asked == Asked::ByLookingAround,
        "{BIN_DIR_VAR} named a PostgreSQL that will not run: {why}"
    );
    eprintln!("skipping the PostgreSQL cluster tests: {why}");
    None
}

/// The first line either stream had to say.
fn first_line(stdout: &[u8], stderr: &[u8]) -> String {
    for stream in [stderr, stdout] {
        let text = String::from_utf8_lossy(stream);
        if let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) {
            return line.to_owned();
        }
    }
    "it said nothing".to_owned()
}

/// The variable that says which PostgreSQL to test against.
///
/// Set it to a `bin` directory and these tests use that server and no other, which is how
/// one machine — or one CI matrix — covers several majors.
const BIN_DIR_VAR: &str = "SLOOP_TEST_PG_BIN";

/// Print which server is about to be exercised.
///
/// Without this a passing run says nothing about *what* it proved, and the whole point of
/// testing several versions is knowing which one each result belongs to.
fn announce(binaries: &Path) {
    let said = Command::new(binaries.join("initdb"))
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    eprintln!("cluster tests are using {said}");
}

/// Where a portable server unpacked for these tests would be, if one has been.
///
/// **So a machine with no database installed can still run every test on this list, and
/// nobody has to remember a variable.** The engines are not all on every developer's machine
/// — MySQL and MariaDB are on almost none — and a suite that quietly skips them proves
/// nothing. Unpack the vendor's portable archive into
///
/// ```text
/// Windows        %LOCALAPPDATA%\sloop-test-engines\<engine>in
/// macOS, Linux   ~/.local/share/sloop-test-engines/<engine>/bin
/// ```
///
/// and the tests find it. `<engine>` is `postgres`, `mysql` or `mariadb`, and what goes in it
/// is the archive's own directory, whole — the server looks beside its `bin` for the rest of
/// itself. Nothing is installed on the machine and nothing is written outside that directory;
/// deleting it is how it is undone.
///
/// `SLOOP_TEST_*_BIN` still outranks this, for a run aimed at one particular build.
pub(crate) fn portable(engine: &str) -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
    }?;

    let bin = home.join("sloop-test-engines").join(engine).join("bin");
    bin.is_dir().then_some(bin)
}

/// Where the server programs are, or `None` if this machine has none.
///
/// `SLOOP_TEST_PG_BIN` first, and when it is set and wrong this **panics** rather than
/// skipping. A test that quietly skips because a variable was mistyped is a green tick that
/// proved nothing, which is worse than a red one.
///
/// Otherwise `PATH`, then the places a package manager puts them — `initdb` ships in the
/// server package and distributions leave it off `PATH`, so a runner with PostgreSQL
/// installed would otherwise look like a machine without it.
fn find_server_binaries() -> Option<(PathBuf, Asked)> {
    if let Some(named) = std::env::var_os(BIN_DIR_VAR) {
        let directory = PathBuf::from(named);
        assert!(
            directory.join(initdb_file_name()).is_file(),
            "{BIN_DIR_VAR} is set to {} but there is no {} in it. These tests were asked \
             for a specific PostgreSQL, so not finding it is a failure and not something \
             to skip past.",
            directory.display(),
            initdb_file_name()
        );
        return Some((directory, Asked::Explicitly));
    }

    if Command::new("initdb")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return Some((PathBuf::new(), Asked::ByLookingAround));
    }

    // One unpacked for these tests, before the package manager's — somebody who put a
    // particular server there meant it to be used.
    if let Some(bin) = portable("postgres").filter(|bin| bin.join(initdb_file_name()).is_file()) {
        return Some((bin, Asked::ByLookingAround));
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
    found.pop().map(|dir| (dir, Asked::ByLookingAround))
}

fn initdb_file_name() -> &'static str {
    if cfg!(windows) {
        "initdb.exe"
    } else {
        "initdb"
    }
}

pub(crate) fn skip(reason: &str) {
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

    let adapter = Postgres::new(cluster.tools());
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
        .dump(&source_target, &dump_path, &[])
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

    let adapter = Postgres::new(cluster.tools());
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
    let old =
        Postgres::new(cluster.tools()).pretending_to_be(Version::new(server.version.major - 1, 0));
    let dump_path = cluster.root.join("never-written.dump");
    let failure = old
        .dump(&source.target(&right), &dump_path, &[])
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

/// The privilege check, closed as a loop: a role that cannot dump, the report that names
/// why, and the same report's own remedy making the dump work.
///
/// **The point is that the advice is executed rather than inspected.** A test that asserted
/// "the report mentions `pg_read_all_data`" would pass just as happily on advice that does
/// not work. This one grants exactly what `sloop doctor` prints and then proves the dump
/// it enables is the same dump a superuser gets — which is the claim `R6a` actually makes.
///
/// CI runs this against every PostgreSQL major in the matrix, so the day one of them
/// changes what a backup role needs, a build goes red instead of somebody's backup going
/// quietly short.
#[test]
fn the_reported_minimum_is_exactly_what_a_dump_needs() {
    use super::privileges::{Phase, Verdict};

    let Some(cluster) = Cluster::start("privileges") else {
        skip("initdb is not on this machine");
        return;
    };

    // Two things `pg_read_all_data` does not cover, both of which stop a dump dead. They
    // live here rather than in `seed` so that the shared fixture — and the counts the
    // round-trip test asserts on — stay exactly as R4 left them.
    let extras = cluster.psql(
        "source_db",
        "CREATE TABLE public.tenanted (id int PRIMARY KEY, tenant text, body text); \
         INSERT INTO public.tenanted VALUES (1, 't1', 'a'), (2, 't2', 'b'); \
         ALTER TABLE public.tenanted ENABLE ROW LEVEL SECURITY; \
         CREATE POLICY only_mine ON public.tenanted USING (tenant = current_user); \
         SELECT lo_from_bytea(0, '\x0102030405'::bytea); \
         REVOKE ALL ON DATABASE source_db FROM PUBLIC; \
         CREATE ROLE gamma LOGIN PASSWORD 'gamma-pw'; \
         GRANT CONNECT ON DATABASE source_db TO gamma;",
    );
    if let Err(why) = extras {
        skip(&format!("this server would not take the fixture: {why}"));
        return;
    }

    let adapter = Postgres::new(cluster.tools());
    let password = Secret::new("gamma-pw".to_owned());
    let source = cluster.database("source_db", "gamma");
    let target = source.target(&password);

    // --- before: the dump fails, and the report says why ----------------------------
    let dump = cluster.root.join("gamma-before.dump");
    assert!(
        adapter.dump(&target, &dump, &[]).is_err(),
        "a role with only CONNECT dumped a database it cannot read"
    );

    let report = adapter
        .check_privileges(&target)
        .expect("the check itself connects and answers");
    assert_eq!(report.role, "gamma");
    assert!(!report.can_dump(), "the report called this role ready");

    let named: Vec<&str> = report
        .gaps_in(Phase::Dump)
        .map(|finding| finding.requirement.id)
        .collect();
    assert_eq!(
        named,
        [
            "pg-read-everything",
            "pg-bypass-row-security",
            "pg-read-large-objects"
        ],
        "the three things a bare role is short of, in the order pg_dump meets them"
    );

    // Every gap says what it costs and how much of the database it affects.
    for finding in report.gaps_in(Phase::Dump) {
        let Verdict::Missing(detail) = &finding.verdict else {
            unreachable!("gaps_in yields only missing findings")
        };
        assert!(
            !detail.is_empty(),
            "{} is missing and says nothing about this database",
            finding.requirement.id
        );
    }

    // --- apply the report's own remedy, and nothing else -----------------------------
    let remedy = report.remedy_for(Phase::Dump, "source_db");
    let (statements, notes): (Vec<_>, Vec<_>) = remedy
        .iter()
        .partition(|line| !line.trim_start().starts_with("--"));

    for statement in &statements {
        cluster
            .psql("source_db", statement)
            .unwrap_or_else(|why| panic!("the report printed `{statement}`, which failed: {why}"));
    }

    // The one line that is a note rather than a statement, because PostgreSQL has no
    // `GRANT ... ON ALL LARGE OBJECTS` to print. Doing by hand what it describes.
    assert_eq!(notes.len(), 1, "unexpected notes: {notes:?}");
    assert!(notes[0].contains("LARGE OBJECT"), "{}", notes[0]);
    cluster
        .psql(
            "source_db",
            "DO $$ DECLARE o oid; BEGIN \
             FOR o IN SELECT oid FROM pg_largeobject_metadata LOOP \
             EXECUTE format('GRANT SELECT ON LARGE OBJECT %s TO gamma', o); \
             END LOOP; END $$;",
        )
        .expect("granting the large objects the note names");

    // --- after: the report agrees, and so does pg_dump -------------------------------
    let after = adapter
        .check_privileges(&target)
        .expect("checking again after the grants");
    assert!(
        after.can_dump(),
        "the remedy was applied and the report still says no: {:?}",
        after.gaps_in(Phase::Dump).collect::<Vec<_>>()
    );

    let summary = adapter
        .dump(&target, &dump, &[])
        .expect("the reported minimum did not actually let the role dump");
    assert!(summary.bytes > 0, "the dump is empty");

    // And the role still cannot write, which is the other half of a *backup* role: the
    // grants above widened what it can read and nothing else.
    assert!(
        after.gaps_in(Phase::Restore).next().is_some(),
        "a read-only remedy handed out write privileges"
    );
}

/// **The two methods `R19a` added, against a real server.**
///
/// `read` and `foreign_keys` are the whole of what the query builder asks an engine for, and
/// neither can be checked without one: what a read-only transaction refuses is PostgreSQL's
/// decision, and what a key looks like column by column is in its catalogue.
#[test]
fn a_read_is_read_only_and_a_key_says_which_columns_match() {
    let Some(cluster) = Cluster::start("reading") else {
        skip("initdb is not on this machine");
        return;
    };

    let adapter = Postgres::new(cluster.tools());
    let password = Secret::new(ALPHA_PASSWORD.to_owned());
    let source = cluster.database("source_db", "alpha");
    let target = source.target(&password);

    // A parent and a child, with a key of two columns — enough to catch a pairing that has
    // been crossed, which a single-column key never would.
    cluster
        .psql(
            "source_db",
            "CREATE TABLE public.shop (region text, code int, name text, \
                                       PRIMARY KEY (region, code)); \
             CREATE TABLE public.sale (id int PRIMARY KEY, region text, shop_code int, \
                                       total numeric, memo text, \
                                       FOREIGN KEY (region, shop_code) \
                                           REFERENCES public.shop (region, code)); \
             INSERT INTO public.shop VALUES ('north', 1, 'Ada'), ('south', 2, 'Bo'); \
             INSERT INTO public.sale VALUES (10, 'north', 1, 99.5, 'first'), \
                                            (11, 'north', 1, 10, ''), \
                                            (12, 'south', 2, 5.25, NULL); \
             ALTER TABLE public.shop OWNER TO alpha; \
             ALTER TABLE public.sale OWNER TO alpha;",
        )
        .expect("seeding the two tables");

    let reading = |sql: &str| {
        adapter.read(
            &target,
            &Reading {
                sql,
                timeout: std::time::Duration::from_secs(30),
            },
        )
    };

    // --- a read comes back with its heading and its rows ----------------------------
    let rows = reading("SELECT id, memo FROM public.sale ORDER BY id").expect("a plain read");
    assert_eq!(rows.columns, vec!["id".to_owned(), "memo".to_owned()]);
    assert_eq!(rows.rows.len(), 3);

    // **A `NULL` and an empty string are told apart**, which is the property a grid is
    // useless without.
    assert_eq!(rows.rows[1][1], Cell::Text(String::new()));
    assert_eq!(rows.rows[2][1], Cell::Null);

    // --- the server refuses a write, and refuses the CTE a blacklist would miss ------
    for statement in [
        "UPDATE public.sale SET memo = 'moved'",
        "DELETE FROM public.sale",
        "CREATE TABLE public.sneaky (id int)",
        "WITH gone AS (DELETE FROM public.sale RETURNING *) SELECT * FROM gone",
    ] {
        let refused = reading(statement).expect_err(statement);
        assert!(
            refused.message().contains("read-only transaction")
                || refused
                    .hint_text()
                    .is_some_and(|hint| hint.contains("read-only transaction")),
            "{statement} was refused for the wrong reason: {}",
            refused.message()
        );
    }

    // And nothing moved: three rows before, three rows after.
    let after = reading("SELECT count(*) FROM public.sale").expect("counting afterwards");
    assert_eq!(after.rows[0][0], Cell::Text("3".to_owned()));

    // --- a psql command is not SQL ---------------------------------------------------
    let refused = reading(r"\! echo hello").expect_err("a meta-command is not a statement");
    assert!(
        refused.message().contains("psql command"),
        "{}",
        refused.message()
    );

    // --- the key, column by column, in key order -------------------------------------
    let keys = adapter.foreign_keys(&target).expect("reading the keys");
    let found = keys
        .iter()
        .find(|key| key.from.name == "sale")
        .unwrap_or_else(|| panic!("no key on sale: {keys:?}"));

    assert_eq!(found.to.name, "shop");
    assert_eq!(
        found.pairs(),
        vec![
            ("region".to_owned(), "region".to_owned()),
            ("shop_code".to_owned(), "code".to_owned()),
        ],
        "the two lists are crossed"
    );

    // --- and the join that key describes returns the right rows ----------------------
    let joined = reading(
        "SELECT s.name, l.total FROM public.sale l \
         LEFT JOIN public.shop s ON l.region = s.region AND l.shop_code = s.code \
         WHERE s.name = 'Ada' AND l.total > '50' \
         LIMIT 201 OFFSET 0",
    )
    .expect("the join runs");
    assert_eq!(joined.rows.len(), 1, "{:?}", joined.rows);
    assert_eq!(joined.rows[0][0], Cell::Text("Ada".to_owned()));
}

/// **`R26`: what a real PostgreSQL will say about a database, and what it will not.**
///
/// The numbers come off `pg_stat_database` and `pg_database_size`, and neither can be checked
/// without a server: whether a counter moves when rows move is the server's business, and the
/// whole entry rests on it moving.
#[test]
fn activity_counts_rows_and_size_and_moves_when_rows_move() {
    let Some(cluster) = Cluster::start("activity") else {
        skip("initdb is not on this machine");
        return;
    };

    let adapter = Postgres::new(cluster.tools());
    let password = Secret::new(ALPHA_PASSWORD.to_owned());
    let source = cluster.database("source_db", "alpha");
    let target = source.target(&password);

    cluster
        .psql(
            "source_db",
            "CREATE TABLE public.note (id int PRIMARY KEY, body text); \
             ALTER TABLE public.note OWNER TO alpha;",
        )
        .expect("seeding a table");

    let first = adapter.activity(&target).expect("a server answers");
    assert!(
        first.said_anything(),
        "PostgreSQL said nothing at all: {first:?}"
    );
    assert!(
        first.size_bytes.is_some_and(|bytes| bytes > 0),
        "a database with a table in it reported no size: {first:?}"
    );

    // **Rows in.** A thousand of them, so the counter cannot move by chance.
    cluster
        .psql(
            "source_db",
            "INSERT INTO public.note SELECT n, repeat('x', 200) FROM generate_series(1, 1000) n;",
        )
        .expect("writing rows");

    // The statistics collector reports asynchronously, so the counter is waited for rather
    // than read once — a fixed sleep would be flaky on a slow machine and slow on a fast one.
    let moved = wait_for(|| {
        adapter
            .activity(&target)
            .ok()
            .and_then(|now| Some((now.rows_in? > first.rows_in?, now)))
            .filter(|(moved, _)| *moved)
            .map(|(_, now)| now)
    });
    let Some(after_writing) = moved else {
        panic!("tup_inserted did not move after a thousand inserts");
    };

    // **Rows out**, which is a different counter and has to be seen moving separately —
    // otherwise one number standing in for both would pass this test.
    cluster
        .psql("source_db", "SELECT count(*) FROM public.note;")
        .expect("reading rows");

    let read_more = wait_for(|| {
        adapter
            .activity(&target)
            .ok()
            .filter(|now| now.rows_out > after_writing.rows_out)
    });
    assert!(
        read_more.is_some(),
        "tup_returned did not move after the table was read"
    );

    // And the size grew, because a thousand rows of two hundred bytes is not nothing.
    let now = adapter.activity(&target).expect("a server answers");
    assert!(
        now.size_bytes > first.size_bytes,
        "the database did not grow after a thousand rows went in: {first:?} then {now:?}"
    );
}

/// Poll until `settled` gives an answer, or give up. The counters below are updated by the
/// statistics collector on its own schedule, so the wait is for it rather than for the work.
fn wait_for<T>(mut settled: impl FnMut() -> Option<T>) -> Option<T> {
    let waited = std::time::Instant::now();
    while waited.elapsed() < std::time::Duration::from_secs(20) {
        if let Some(answer) = settled() {
            return Some(answer);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    None
}
