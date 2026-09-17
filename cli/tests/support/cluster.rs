//! The PostgreSQL every sandbox keeps its registry in.
//!
//! **`R19c4` moved the registry into a database, so a sandbox needs one.** Before it, a
//! sandbox was a directory; now it is a directory *and* a `sloop_database`, because every
//! command reads its registry out of one and a machine with none is told to run `sloop setup`.
//!
//! **One cluster per test binary, and a template database it clones.** `initdb` takes seconds
//! and there are over a hundred sandboxes; running Setup in each would be minutes per
//! platform. So Setup runs **once**, against a template, and every sandbox after that is a
//! `CREATE DATABASE … TEMPLATE …` — one statement, and an exact copy of a really-migrated
//! schema rather than a fixture that can drift from it.
//!
//! **A machine with no PostgreSQL server skips, out loud.** That is the rule
//! `server::cluster_tests` already follows and the reason CI stayed red for twelve commits
//! once: a runner given `postgresql-client` has `psql` and no `postgres`, and a test that
//! quietly passes on such a machine proved nothing. Every sandbox asks [`available`] first.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The role every sandbox's registry is owned by. One per cluster, not one per sandbox: a
/// second sandbox that found the role already there and could not read its password back
/// would *reset* it, and take the first sandbox's database with it.
pub const ROLE: &str = "sloop_db_admin";

/// The superuser of the cluster these tests build.
pub const SUPERUSER: &str = "postgres";

/// Both passwords. A throwaway cluster on loopback, on a port nothing else uses, that exists
/// for the length of one `cargo test` — this is a fixture, not a secret, and writing it here
/// is what keeps it out of the machine's keyring.
pub const PASSWORD: &str = "sloopTestClusterPw0";

/// The variable the sandbox hands the child so its `server.toml` can name a route rather
/// than a password. Rule 3 holds inside the harness too: the record says `${…}`, never a
/// value.
pub const PASSWORD_VAR: &str = "SLOOP_TEST_DB_PW";

/// The database Setup is run against once, and every sandbox copies.
const TEMPLATE: &str = "sloop_template";

/// Where `SLOOP_TEST_PG_BIN` points, shared with the other cluster tests so one machine sets
/// one variable.
const BIN_DIR_VAR: &str = "SLOOP_TEST_PG_BIN";

fn exe(name: &str) -> String {
    format!("{name}{}", if cfg!(windows) { ".exe" } else { "" })
}

/// Windows: start the postmaster without the console this test binary is attached to.
///
/// **This is why a finished test run used to look like a hung one.** On Windows a child
/// started from a console inherits its handles and holds them for as long as it lives — so a
/// postmaster that outlives `cargo test` keeps the shell's pipe from ever reaching end of
/// file, and whatever is reading it waits forever on a command that already printed its
/// results. `DETACHED_PROCESS` is the flag that stops it; the server's own output goes to the
/// log file `-l` names, which is where anything worth reading ends up anyway.
#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn detach(_command: &mut Command) {}

/// Stop and delete any cluster a previous run left behind.
///
/// **Statics are never dropped**, which is the other half of the same problem: the cluster
/// lives in a `OnceLock`, so `Drop for Cluster` does not run when the test binary exits and
/// the postmaster outlives it. Rust's test harness has no "after everything" hook to hang the
/// teardown on, so the next run cleans up the last one — by process id, so a run that is
/// still going is never touched.
fn reap_stale() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(pid) = name.strip_prefix("sloop-test-cluster-") else {
            continue;
        };
        // Never our own, and never one belonging to a test binary still running beside us.
        if pid == std::process::id().to_string() || is_running(pid) {
            continue;
        }

        if let Some(bin) = binaries() {
            let _ = Command::new(bin.join(exe("pg_ctl")))
                .arg("-D")
                .arg(&path)
                .args(["-m", "immediate", "-w", "stop"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Is a process with this id still alive?
fn is_running(pid: &str) -> bool {
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains(pid))
    } else {
        Path::new(&format!("/proc/{pid}")).exists()
            || Command::new("kill")
                .args(["-0", pid])
                .status()
                .is_ok_and(|status| status.success())
    }
}

/// A cluster these tests started, and will stop.
pub struct Cluster {
    bin: PathBuf,
    data: PathBuf,
    port: u16,
}

impl Cluster {
    /// Where its programs are.
    #[must_use]
    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Which port it answers on.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Make a database for one sandbox, copied from the migrated template.
    ///
    /// The name is the sandbox's, so two sandboxes are two machines as far as sloop is
    /// concerned — which matters more than it sounds. The *global* registry is
    /// `project_id IS NULL`, so two sandboxes sharing one database would share their global
    /// stores, and every test that registers a database globally would see every other's.
    pub fn database_for(&self, label: &str) -> String {
        let name = format!("sloop_test_{label}");
        let sql = format!(
            "DROP DATABASE IF EXISTS \"{name}\";
             CREATE DATABASE \"{name}\" TEMPLATE \"{TEMPLATE}\" OWNER \"{ROLE}\";"
        );

        let (ok, said) = self.psql(SUPERUSER, "postgres", &sql);
        assert!(ok, "could not make {name} for a sandbox: {said}");
        name
    }

    /// The sealed password store this registry holds, as bytes.
    ///
    /// **`secrets.sealed` is a row since `R19c4`**, so a test that read the file reads this.
    /// The bytes are byte for byte what the file held — `R3`'s `SLOOPSEC` format, unchanged —
    /// which is what lets the assertion that a password is *not* readable in them keep
    /// meaning exactly what it meant.
    pub fn sealed_bytes(&self, database: &str, project: Option<&str>) -> Vec<u8> {
        let predicate = match project {
            None => "project_id IS NULL".to_owned(),
            Some(dir) => format!(
                "project_id = (SELECT id FROM project WHERE directory = '{}')",
                dir.replace('\'', "''")
            ),
        };

        let (ok, said) = self.psql(
            ROLE,
            database,
            &format!("SELECT encode(blob, 'hex') FROM sealed_vault WHERE {predicate};"),
        );
        assert!(ok, "could not read the sealed vault: {said}");

        let hex = said.trim();
        (0..hex.len())
            .step_by(2)
            .filter_map(|at| hex.get(at..at + 2))
            .filter_map(|pair| u8::from_str_radix(pair, 16).ok())
            .collect()
    }

    /// Run SQL against this cluster, with the password on the environment of one child.
    fn psql(&self, role: &str, database: &str, sql: &str) -> (bool, String) {
        let mut child = Command::new(self.bin.join(exe("psql")))
            .args(["-h", "127.0.0.1"])
            .args(["-p", &self.port.to_string()])
            .args(["-U", role])
            .args(["-d", database])
            .args(["-v", "ON_ERROR_STOP=1"])
            .arg("--no-psqlrc")
            .arg("--no-password")
            .args(["--tuples-only", "--no-align"])
            .args(["--field-separator", &UNIT.to_string()])
            .args(["-f", "-"])
            .env("PGPASSWORD", PASSWORD)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("psql should run");

        {
            use std::io::Write as _;
            let mut pipe = child.stdin.take().expect("psql has standard input");
            pipe.write_all(sql.as_bytes()).expect("writing to psql");
        }

        let output = child.wait_with_output().expect("psql should finish");
        (
            output.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
    }
}

/// The separator rows come back on.
///
/// `0x1F` is ASCII's own field separator and is not a character anybody puts in a host name,
/// a database name or a command line. The alternative — a comma, a tab, a pipe — is a
/// character somebody eventually registers, and then a test fails for a reason that has
/// nothing to do with what it was testing.
const UNIT: char = '\u{1f}';

impl Cluster {
    /// This registry's databases, rendered the way `registry.toml` used to hold them.
    ///
    /// **`R19c4` moved the registry into PostgreSQL, so a test that asserted on the file now
    /// asks here.** What the tests were ever checking is what the registry *holds* — the file
    /// was the medium, not the point — so this renders the same shape from the rows and every
    /// assertion about names, hosts, routes and ports keeps its meaning.
    pub fn registry_text(&self, database: &str, project: Option<&str>) -> String {
        let predicate = match project {
            None => "d.project_id IS NULL".to_owned(),
            Some(dir) => format!(
                "d.project_id = (SELECT id FROM project WHERE directory = '{}')",
                dir.replace('\'', "''")
            ),
        };

        let (ok, said) = self.psql(
            ROLE,
            database,
            &format!(
                "SELECT d.label, e.name, d.host, d.port, d.database_name, d.username,
                        d.password_route
                   FROM registered_database d
                   JOIN engine e ON e.id = d.engine_id
                  WHERE {predicate}
                  ORDER BY d.label;"
            ),
        );
        assert!(ok, "could not read the registry back: {said}");

        let mut text = String::new();
        for line in said.lines() {
            let fields: Vec<&str> = line.split(UNIT).collect();
            if fields.len() != 7 {
                continue;
            }
            let _ = write!(
                text,
                "[databases.{}]\nengine = \"{}\"\nhost = \"{}\"\nport = {}\n\
                 database = \"{}\"\nuser = \"{}\"\npassword = \"{}\"\n\n",
                fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6]
            );
        }

        // The backup keypair, which `key` and `backup` assert on the same way.
        let (ok, said) = self.psql(
            ROLE,
            database,
            &format!(
                "SELECT k.public_key, k.private_key_route, coalesce(k.key_kept, '')
                   FROM backup_key k
                  WHERE {};",
                predicate.replace("d.", "k.")
            ),
        );
        assert!(ok, "could not read the backup key back: {said}");

        for line in said.lines() {
            let fields: Vec<&str> = line.split(UNIT).collect();
            if fields.len() != 3 {
                continue;
            }
            let _ = write!(
                text,
                "[encryption]\npublic-key = \"{}\"\nprivate-key = \"{}\"\n",
                fields[0], fields[1]
            );
            if !fields[2].is_empty() {
                let _ = writeln!(text, "key-kept = \"{}\"", fields[2]);
            }
        }

        text
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let _ = Command::new(self.bin.join(exe("pg_ctl")))
            .arg("-D")
            .arg(&self.data)
            .args(["-m", "immediate", "-w", "stop"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

/// The cluster for this test binary, built the first time something asks for one.
///
/// `None` on a machine that cannot host a database, and every caller skips rather than fails
/// — see the module header for why that distinction is load-bearing here.
pub fn available() -> Option<&'static Cluster> {
    static CLUSTER: OnceLock<Option<Cluster>> = OnceLock::new();
    CLUSTER.get_or_init(build).as_ref()
}

/// Where the server programs are, or `None` if this machine has none.
///
/// `SLOOP_TEST_PG_BIN` first, and when it is set and wrong this **panics** rather than
/// skipping: a suite that quietly skipped because a variable was mistyped is a green tick
/// that proved nothing.
fn binaries() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os(BIN_DIR_VAR) {
        let directory = PathBuf::from(named);
        assert!(
            directory.join(exe("initdb")).is_file(),
            "{BIN_DIR_VAR} is set to {} but there is no {} in it",
            directory.display(),
            exe("initdb")
        );
        return Some(directory);
    }

    let mut looked: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();

    // The directories a package manager puts a server in, which are very often off `PATH`.
    for guess in [
        "/usr/lib/postgresql",
        "/usr/local/pgsql/bin",
        "/opt/homebrew/opt/postgresql@18/bin",
        "/usr/local/opt/postgresql@18/bin",
    ] {
        let path = Path::new(guess);
        if path.join(exe("initdb")).is_file() {
            looked.push(path.to_path_buf());
        } else if let Ok(entries) = std::fs::read_dir(path) {
            // `/usr/lib/postgresql/<major>/bin`
            for entry in entries.flatten() {
                looked.push(entry.path().join("bin"));
            }
        }
    }
    for root in ["C:/Program Files/PostgreSQL"] {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                looked.push(entry.path().join("bin"));
            }
        }
    }

    // All three, which is what a *server* means. A runner given only the client package has
    // `psql` and no `postgres`, and building a cluster on it fails where it should not have
    // begun.
    looked.into_iter().find(|directory| {
        ["initdb", "pg_ctl", "postgres", "psql"]
            .iter()
            .all(|name| directory.join(exe(name)).is_file())
    })
}

/// A port this test binary can have to itself.
///
/// Derived from the process id, because `cargo test` runs the eleven test binaries at once
/// and two of them on one port would each see the other's databases.
fn port() -> u16 {
    let spread = u16::try_from(std::process::id() % 2000).unwrap_or(0);
    51000 + spread
}

/// Build the cluster, run Setup against a template database, and hand it back.
fn build() -> Option<Cluster> {
    reap_stale();

    let Some(bin) = binaries() else {
        eprintln!(
            "skipping: this machine has no PostgreSQL server, so no sandbox can have a \
             registry to read"
        );
        return None;
    };

    let data = std::env::temp_dir().join(format!("sloop-test-cluster-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let port = port();

    let initialised = Command::new(bin.join(exe("initdb")))
        .arg("-D")
        .arg(&data)
        .args(["-U", SUPERUSER])
        .args(["--encoding", "UTF8"])
        .args(["--locale", "C"])
        // `trust`, and only on loopback, on a port nothing else uses, for the length of one
        // `cargo test`. The alternative is `--pwfile`, which is a plaintext password on disk
        // — and the harness follows rule 3 for the same reason the product does.
        .args(["--auth-local", "trust", "--auth-host", "trust"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if !initialised.is_ok_and(|status| status.success()) {
        eprintln!(
            "skipping: initdb would not build a cluster at {}",
            data.display()
        );
        return None;
    }

    let mut starting = Command::new(bin.join(exe("pg_ctl")));
    detach(&mut starting);
    let started = starting
        .arg("-D")
        .arg(&data)
        .arg("-o")
        .arg(format!("-p {port} -c listen_addresses=127.0.0.1"))
        .arg("-l")
        .arg(data.join("server.log"))
        .arg("--timeout=60")
        .args(["-w", "start"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // Every stream to null: on Windows the postmaster inherits whatever handles it is
        // given and holds them for as long as it runs, so an inherited stderr keeps the
        // parent's pipe from ever reaching end of file.
        .stderr(Stdio::null())
        .status();

    if !started.is_ok_and(|status| status.success()) {
        eprintln!("skipping: this machine cannot host a database on {port}");
        let _ = std::fs::remove_dir_all(&data);
        return None;
    }

    let cluster = Cluster { bin, data, port };

    // The role every sandbox's registry is owned by, and the template every sandbox copies.
    let (ok, said) = cluster.psql(
        SUPERUSER,
        "postgres",
        &format!(
            "ALTER ROLE \"{SUPERUSER}\" WITH PASSWORD '{PASSWORD}';
             CREATE ROLE \"{ROLE}\" WITH LOGIN PASSWORD '{PASSWORD}';"
        ),
    );
    assert!(ok, "could not prepare the test cluster: {said}");

    Some(cluster)
}

/// The `server.toml` a sandbox writes so that `sloop` finds this cluster instead of hunting
/// for a PostgreSQL 18 of its own.
///
/// **Both passwords are routes, not values** — `${SLOOP_TEST_DB_PW}`, which the sandbox sets
/// on the child. A harness that wrote a password into a file would be proving the opposite of
/// what rule 3 says, and `Route::parse` would refuse it anyway.
#[must_use]
pub fn record_for(cluster: &Cluster, database: &str) -> String {
    format!(
        "version = 2\n\
         bin = '{bin}'\n\
         data = '{data}'\n\
         port = {port}\n\
         superuser = \"{SUPERUSER}\"\n\
         password = \"${{{PASSWORD_VAR}}}\"\n\
         origin = \"machine\"\n\
         \n\
         [database]\n\
         name = \"{database}\"\n\
         role = \"{ROLE}\"\n\
         password = \"${{{PASSWORD_VAR}}}\"\n",
        bin = cluster.bin.display(),
        data = cluster.data.display(),
        port = cluster.port,
    )
}

/// Run Setup once, against the template, so every sandbox after this copies a schema that a
/// real `sloop setup` wrote rather than one the harness imitated.
pub fn prepare_template(cluster: &Cluster, run_setup: impl FnOnce(&str) -> bool) {
    static READY: OnceLock<()> = OnceLock::new();
    READY.get_or_init(|| {
        let (ok, said) = cluster.psql(
            SUPERUSER,
            "postgres",
            &format!("CREATE DATABASE \"{TEMPLATE}\" OWNER \"{ROLE}\";"),
        );
        assert!(ok, "could not make the template database: {said}");

        assert!(
            run_setup(TEMPLATE),
            "`sloop setup` would not migrate the template database"
        );

        // A template cannot be copied while anything is connected to it, and nothing is —
        // Setup closed its last `psql` before returning. Marking it as a template is what
        // lets `CREATE DATABASE … TEMPLATE` run without being the owner of every object.
        let (ok, said) = cluster.psql(
            SUPERUSER,
            "postgres",
            &format!("UPDATE pg_database SET datistemplate = true WHERE datname = '{TEMPLATE}';"),
        );
        assert!(ok, "could not mark {TEMPLATE} as a template: {said}");
    });
}
