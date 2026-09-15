//! The MySQL and MariaDB adapters against a real server: one created for the test, on a
//! port nothing else uses, destroyed again when the test finishes.
//!
//! Never the machine's own MySQL. The server is initialised into a temporary directory with
//! its own root account, its own roles and its own data, and `Drop` shuts it down and
//! deletes it whether the test passed or not.
//!
//! Three things differ from the PostgreSQL harness, and all three come from the server
//! itself. There is no `pg_ctl`, so the daemon is spawned as a child and waited for by
//! asking it `SELECT 1` until it answers. The accounts are made by the server on its way up
//! rather than over a connection, because there is no usable account until they exist — see
//! [`Server::bootstrap`]. And a leftover from a killed run is swept by asking it to
//! `SHUTDOWN` rather than by killing a pid, which needs no platform-specific way of
//! stopping a process by number.
//!
//! Skipped with a printed reason where no server can be found, which is the honest outcome
//! on a machine with only the client tools installed. Named explicitly through
//! `SLOOP_TEST_MYSQL_BIN` or `SLOOP_TEST_MARIADB_BIN` and it **fails** instead: a test that
//! quietly skips because a variable was mistyped is a green tick that proved nothing.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use super::mysql::{Family, MysqlFamily, Tools};
use super::{Adapter, Engine, Target};
use crate::secret::Secret;

/// High enough to be out of the way, and stepped per instance so two tests never collide.
static NEXT_PORT: AtomicU16 = AtomicU16::new(55_331);

/// Deliberately awful, and different for each role — the point of two roles is that a dump
/// and a restore each use their own credentials.
const ADMIN_PASSWORD: &str = r#"adm1n\pass >w #1 "q" ;x"#;
const ALPHA_PASSWORD: &str = r#"alpha\pw $HOME #2 'q' "d" |p"#;
const BETA_PASSWORD: &str = r#"beta\pw >out #3 "q" &b"#;

/// Long enough for MySQL 8 to build a data directory on a cold, busy CI runner.
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// A throwaway server.
struct Server {
    family: Family,
    root: PathBuf,
    data: PathBuf,
    port: u16,
    /// Where the client programs are. Empty means `PATH`.
    binaries: PathBuf,
    /// The daemon itself, which on Debian and Ubuntu is not beside the client programs.
    daemon: PathBuf,
    /// Whether this server was started with a certificate of its own.
    tls: Tls,
    process: Option<Child>,
}

/// Whether the server offers an encrypted connection at all.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tls {
    /// However the build has it, which for a current MySQL or MariaDB means a
    /// self-signed certificate the server generates for itself.
    Offered,
    /// Turned off at the server, the way a database on a private network usually is.
    Disabled,
}

impl Server {
    /// Build, start and seed one, or `None` when this machine cannot host a server.
    ///
    /// Everything up to and including the fixture happens here, so a test is handed a
    /// server that is ready or handed nothing at all. There is no half-built state for an
    /// assertion to trip over and report as a bug.
    fn start(family: Family, label: &str) -> Option<Self> {
        Self::start_with(family, label, Tls::Offered)
    }

    /// One with TLS switched off at the server.
    ///
    /// Most of the databases this tool is for sit inside a private network with no
    /// certificate at all, so "plain when the server does not offer it" is the common case
    /// and not the edge one. It needs asking for deliberately, because a modern MySQL 8 and
    /// a MariaDB 11.4 both generate a certificate for themselves while they are being
    /// initialised — left alone, neither of them can show this.
    fn start_without_tls(family: Family, label: &str) -> Option<Self> {
        Self::start_with(family, label, Tls::Disabled)
    }

    fn start_with(family: Family, label: &str, tls: Tls) -> Option<Self> {
        let (binaries, daemon, asked) = find_server_binaries(family)?;

        announce(&daemon);
        sweep_stale_servers(family, &binaries);

        let port = a_free_port();
        let root =
            std::env::temp_dir().join(format!("sloop-my-{}-{label}-{port}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a temporary directory");

        let mut server = Self {
            family,
            data: root.join("data"),
            root,
            port,
            binaries,
            daemon,
            tls,
            process: None,
        };

        if let Err(why) = server.initialise() {
            return refuse_or_skip(family, asked, &why);
        }
        if let Err(why) = server.launch() {
            return refuse_or_skip(family, asked, &why);
        }
        if let Err(why) = server.seed() {
            return refuse_or_skip(family, asked, &why);
        }

        Some(server)
    }

    /// The accounts and the databases, created by the server itself as it starts.
    ///
    /// `--init-file` rather than a connection, because there is no account to connect *as*
    /// until this has run. A freshly initialised server has only `root@localhost`, and
    /// `localhost` is a name — over TCP from 127.0.0.1 with `--skip-name-resolve` on, it
    /// matches nothing. Every account below is created at `%` and the question never comes
    /// up again.
    ///
    /// One statement per line and no comments: that is the whole of what this file is
    /// allowed to contain, on both engines.
    fn bootstrap(&self) -> Result<PathBuf, String> {
        let path = self.root.join("bootstrap.sql");
        let sql = format!(
            "CREATE USER 'sloop_admin'@'%' IDENTIFIED BY {admin};\n\
             GRANT ALL PRIVILEGES ON *.* TO 'sloop_admin'@'%' WITH GRANT OPTION;\n\
             CREATE USER 'alpha'@'%' IDENTIFIED BY {alpha};\n\
             CREATE USER 'beta'@'%' IDENTIFIED BY {beta};\n\
             CREATE DATABASE source_db;\n\
             CREATE DATABASE target_db;\n\
             GRANT ALL PRIVILEGES ON source_db.* TO 'alpha'@'%';\n\
             GRANT ALL PRIVILEGES ON target_db.* TO 'beta'@'%';\n\
             FLUSH PRIVILEGES;\n",
            admin = sql_literal(ADMIN_PASSWORD),
            alpha = sql_literal(ALPHA_PASSWORD),
            beta = sql_literal(BETA_PASSWORD),
        );

        std::fs::write(&path, sql).map_err(|error| format!("the init file: {error}"))?;
        Ok(path)
    }

    /// The client tools that belong to *this* server.
    ///
    /// A runner can have a server installed without the matching client on `PATH`, and then
    /// the adapter cannot find its dump program even though the server started — which looks
    /// like a failed assertion rather than a machine without the tools.
    fn tools(&self) -> Tools {
        Tools {
            dump: self.tool(match self.family {
                Family::Mysql => "mysqldump",
                Family::Mariadb => "mariadb-dump",
            }),
            client: self.tool(match self.family {
                Family::Mysql => "mysql",
                Family::Mariadb => "mariadb",
            }),
        }
    }

    /// The path to one of the client programs. An empty directory means `PATH`.
    fn tool(&self, name: &str) -> PathBuf {
        self.binaries.join(name)
    }

    /// Build the data directory.
    ///
    /// The two families do this differently and neither can do it the other's way: MySQL has
    /// `--initialize-insecure` built into the daemon, MariaDB has a separate program and no
    /// such flag.
    fn initialise(&self) -> Result<(), String> {
        let output = match self.family {
            Family::Mysql => Command::new(&self.daemon)
                .arg("--no-defaults")
                .arg("--initialize-insecure")
                .arg(format!("--datadir={}", self.data.display()))
                .arg(format!("--log-error={}", self.log().display()))
                .stdin(Stdio::null())
                .output(),
            Family::Mariadb => {
                let installer = self.install_db_program()?;
                let mut command = Command::new(&installer);
                command.arg(format!("--datadir={}", self.data.display()));

                // Two programs that share a name. On Unix it is the long-standing shell
                // script, which reads configuration files and has to be told not to. On
                // Windows it is a small C program with an option set of its own — no
                // `--no-defaults` to give it, and it works out where it lives by itself.
                if cfg!(windows) {
                    command.arg("--silent");
                } else {
                    command.arg("--no-defaults");
                }

                command.stdin(Stdio::null()).output()
            }
        };

        let Ok(output) = output else {
            return Err(format!("{} would not run", self.daemon.display()));
        };

        if output.status.success() {
            return Ok(());
        }

        Err(format!(
            "the data directory could not be built: {}",
            first_line(&output.stdout, &output.stderr, &self.read_log())
        ))
    }

    /// MariaDB's `mariadb-install-db`, under whichever of its two names this build ships.
    fn install_db_program(&self) -> Result<PathBuf, String> {
        let exe = if cfg!(windows) { ".exe" } else { "" };
        for name in ["mariadb-install-db", "mysql_install_db"] {
            let candidate = self.binaries.join(format!("{name}{exe}"));
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        Err("no mariadb-install-db beside the server".to_owned())
    }

    /// Start the server and wait until it answers.
    ///
    /// `spawn` with every stream sent to null, and **never** `output`: `output` builds pipes
    /// and then reads them to end of file, which for a daemon that is not going to exit means
    /// blocking forever. The server's own output goes to `--log-error`, which is where
    /// anything worth reading after a failure ends up anyway.
    fn launch(&mut self) -> Result<(), String> {
        let bootstrap = self.bootstrap()?;

        let mut command = Command::new(&self.daemon);
        command
            .arg("--no-defaults")
            .arg(format!("--datadir={}", self.data.display()))
            .arg(format!("--port={}", self.port))
            .arg(format!("--log-error={}", self.log().display()))
            .arg(format!("--init-file={}", bootstrap.display()))
            .arg("--bind-address=127.0.0.1")
            // So an account defined for `%` matches a connection from 127.0.0.1 without
            // this machine's resolver having an opinion about it.
            .arg("--skip-name-resolve")
            // MySQL 8 turns the binary log on by default, and with it a rule that a
            // non-SUPER account may not create a stored function. Nothing here replicates,
            // so the log buys a restriction and nothing else.
            .arg("--skip-log-bin")
            // A server that is about to be deleted does not need to survive a power cut.
            .arg("--innodb-flush-log-at-trx-commit=0");

        match self.family {
            Family::Mysql => {
                // Otherwise the X plugin takes port 33060 and a second instance cannot
                // start.
                command.arg("--mysqlx=0");
                // Windows only, and MySQL only — MariaDB's Windows build has no such
                // variable and refuses to start when handed it.
                if cfg!(windows) {
                    command.arg("--shared-memory=0");
                }
            }
            Family::Mariadb => {
                let base = self
                    .binaries
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                if !base.as_os_str().is_empty() {
                    command.arg(format!("--basedir={}", base.display()));
                }
            }
        }

        if self.tls == Tls::Disabled {
            match self.family {
                // MySQL 8.4 has no `--ssl` option left to turn off; an empty list of
                // acceptable TLS versions is what does it there.
                Family::Mysql => command.arg("--tls-version="),
                Family::Mariadb => command.arg("--skip-ssl"),
            };
        }

        if !cfg!(windows) {
            command.arg(format!(
                "--socket={}",
                self.root.join("server.sock").display()
            ));
            command.arg(format!(
                "--pid-file={}",
                self.root.join("server.pid").display()
            ));
        }

        let process = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("{} would not start: {error}", self.daemon.display()))?;
        self.process = Some(process);

        self.wait_until_it_answers()
    }

    /// Poll until the server takes a connection, or give up with what its log said.
    ///
    /// There is no `pg_ctl --wait` here, so this is the wait. Bounded, so a server that will
    /// not come up fails the test instead of hanging it.
    fn wait_until_it_answers(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + READY_TIMEOUT;

        while Instant::now() < deadline {
            if let Some(process) = self.process.as_mut() {
                if let Ok(Some(status)) = process.try_wait() {
                    self.process = None;
                    return Err(format!(
                        "the server exited with {status} before it was ready: {}",
                        self.read_log()
                    ));
                }
            }

            // As the account the init file made, so this waits for the bootstrap and not
            // merely for the port: a server answering before its accounts exist is not
            // ready for anything this test wants to do.
            if self.client("mysql", "SELECT 1").is_ok() {
                return Ok(());
            }

            std::thread::sleep(Duration::from_millis(250));
        }

        Err(format!(
            "the server never answered on port {}: {}",
            self.port,
            self.read_log()
        ))
    }

    fn log(&self) -> PathBuf {
        self.root.join("server.log")
    }

    /// What the server's log says went wrong.
    ///
    /// The **first error**, not the last line. A server that aborts still logs its shutdown
    /// afterwards, so the last line of a failed start is always `MySQL Server - end.` —
    /// which says only that it is over and nothing about why.
    fn read_log(&self) -> String {
        let log = std::fs::read_to_string(self.log()).unwrap_or_default();

        let first_error = log
            .lines()
            .map(str::trim)
            .find(|line| line.contains("[ERROR]"));

        first_error
            .or_else(|| log.lines().map(str::trim).rfind(|line| !line.is_empty()))
            .unwrap_or("it left no log")
            .to_owned()
    }

    /// Run SQL as the fixture's administrator, against `database`.
    ///
    /// Fallible, because building the fixture is not what these tests are about. A server
    /// that will not take `CREATE TABLE` is a machine that cannot host the fixture, and that
    /// is a skip — the assertions further down are where a real bug shows up.
    fn client(&self, database: &str, sql: &str) -> Result<String, String> {
        self.client_as("sloop_admin", ADMIN_PASSWORD, database, sql)
    }

    /// The same, as whichever account the statement is supposed to come from.
    fn client_as(
        &self,
        user: &str,
        password: &str,
        database: &str,
        sql: &str,
    ) -> Result<String, String> {
        let program = self.tool(match self.family {
            Family::Mysql => "mysql",
            Family::Mariadb => "mariadb",
        });

        let mut command = Command::new(&program);

        // First, and it has to be first: the client rejects `--no-defaults` in any other
        // position.
        command.arg("--no-defaults");

        // The same thing the adapter has to do, for the same reason: MySQL's default
        // authentication plugin will not send a password over an unencrypted connection
        // without the server's public key to seal it with, so the `no TLS` fixture cannot
        // even be built without this. MariaDB does not use that plugin and its client has
        // no such option.
        if self.family == Family::Mysql {
            command.arg("--get-server-public-key");
        }

        let output = command
            .arg("--protocol=TCP")
            .arg("--host=127.0.0.1")
            .arg(format!("--port={}", self.port))
            .arg(format!("--user={user}"))
            .arg(format!("--database={database}"))
            // Otherwise the client takes the console's codepage, and a routine created
            // through here records `cp850` on Windows and `utf8mb4` on Linux — which makes
            // the dump differ between machines for a reason that is the fixture's fault.
            .arg("--default-character-set=utf8mb4")
            .arg("--batch")
            .arg("--skip-column-names")
            .arg("--connect-timeout=5")
            .arg("--execute")
            .arg(sql)
            .env("MYSQL_PWD", password)
            .stdin(Stdio::null())
            .output();

        match output {
            Ok(output) if output.status.success() => {
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
            Ok(output) => Err(first_line(
                &output.stdout,
                &output.stderr,
                "it said nothing",
            )),
            Err(error) => Err(format!("{} would not run: {error}", program.display())),
        }
    }

    fn sql(&self, database: &str, statements: &str) -> Result<(), String> {
        self.client(database, statements)
            .map(|_| ())
            .map_err(|why| format!("the fixture would not build: {why}"))
    }

    /// A statement issued by `alpha`, the account whose database this is.
    ///
    /// The schema is built by the role that owns it rather than by the administrator, and
    /// that is not decoration. `SHOW CREATE FUNCTION` on a routine somebody *else* defined
    /// needs the global `SHOW_ROUTINE` privilege from MySQL 8.0.20 on — so a fixture whose
    /// routines belonged to the administrator would be testing a permission model no real
    /// backup role has, and failing for a reason that has nothing to do with sloop.
    fn sql_as_alpha(&self, database: &str, statements: &str) -> Result<(), String> {
        self.client_as("alpha", ALPHA_PASSWORD, database, statements)
            .map(|_| ())
            .map_err(|why| format!("the fixture would not build: {why}"))
    }

    /// The schema, with every object type R5 has to carry across: a view, a trigger, a
    /// routine and an event.
    ///
    /// The accounts and the databases are not here — the server made those on its way up.
    /// See [`Server::bootstrap`].
    fn seed(&self) -> Result<(), String> {
        self.sql_as_alpha(
            "source_db",
            "CREATE TABLE widgets (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(80) NOT NULL);",
        )?;
        self.sql_as_alpha(
            "source_db",
            "CREATE TABLE notes (id INT PRIMARY KEY, body TEXT);",
        )?;
        // A row that contains the exact text the dump filter looks for. If the filter ever
        // reaches past the lines it is allowed to touch, this row comes back changed.
        self.sql_as_alpha(
            "source_db",
            "CREATE TABLE quotes (id INT PRIMARY KEY, body TEXT);",
        )?;
        self.sql_as_alpha(
            "source_db",
            "INSERT INTO quotes VALUES \
             (1, '/*!50013 DEFINER=`alpha`@`%` SQL SECURITY DEFINER */'), \
             (2, 'a back\\\\slash and a \\'quote\\' walk into a bar');",
        )?;
        self.sql_as_alpha(
            "source_db",
            "INSERT INTO widgets (name) VALUES ('widget 1'); \
             INSERT INTO widgets (name) SELECT CONCAT('widget ', id) FROM widgets; \
             INSERT INTO widgets (name) SELECT CONCAT('widget ', id) FROM widgets; \
             INSERT INTO widgets (name) SELECT CONCAT('widget ', id) FROM widgets;",
        )?;
        self.sql_as_alpha(
            "source_db",
            "INSERT INTO notes VALUES (1, 'note one'), (2, 'note two'), \
             (3, 'note three'), (4, 'note four'), (5, 'note five'), (6, 'note six'), \
             (7, 'note seven');",
        )?;

        // The three object types the round-trip has to preserve, every one of them stamped
        // by root with a DEFINER that beta is not allowed to impersonate.
        self.sql_as_alpha(
            "source_db",
            "CREATE VIEW recent AS SELECT * FROM widgets WHERE id > 4;",
        )?;
        self.sql_as_alpha(
            "source_db",
            "CREATE TRIGGER widgets_upper BEFORE INSERT ON widgets \
             FOR EACH ROW SET NEW.name = UPPER(NEW.name);",
        )?;
        self.sql_as_alpha(
            "source_db",
            "CREATE FUNCTION widget_count() RETURNS INT READS SQL DATA \
             RETURN (SELECT COUNT(*) FROM widgets);",
        )?;
        self.sql_as_alpha(
            "source_db",
            "CREATE EVENT nightly ON SCHEDULE EVERY 1 DAY DISABLE \
             DO INSERT INTO notes VALUES (99, 'from the event');",
        )
    }

    fn database(&self, name: &str, user: &str) -> Connection {
        Connection {
            engine: self.family.engine(),
            host: "127.0.0.1".to_owned(),
            port: self.port,
            database: name.to_owned(),
            user: user.to_owned(),
        }
    }

    /// Ask the server a question directly, without going through the adapter.
    ///
    /// The point of the round-trip test is what the destination *contains* afterwards, and
    /// the adapter's own `row_counts` is the thing under test — so what the objects look
    /// like on the other side is read from the server itself.
    /// Whether the fixture's own connection to this server came up encrypted.
    ///
    /// The independent half of the TLS assertion: same server, same client library, a
    /// different piece of code asking.
    fn connection_is_encrypted(&self) -> bool {
        // `--batch` prints `Ssl_cipher<TAB><cipher>`, and an unencrypted session leaves the
        // second column empty.
        self.ask("mysql", "SHOW SESSION STATUS LIKE 'Ssl_cipher'")
            .split('\t')
            .nth(1)
            .is_some_and(|cipher| !cipher.trim().is_empty())
    }

    fn ask(&self, database: &str, sql: &str) -> String {
        self.client(database, sql)
            .unwrap_or_else(|why| panic!("asking the server `{sql}`: {why}"))
            .trim()
            .to_owned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            // Ask first. A clean shutdown releases the port and the files immediately,
            // where killing the process leaves Windows holding both for a moment.
            let asked = self.client("mysql", "SHUTDOWN").is_ok()
                && wait_for_exit(&mut process, Duration::from_secs(30));

            if !asked {
                let _ = process.kill();
                let _ = process.wait();
            }
        }
        // Best effort: on Windows a file handle can outlive the process by a moment, and a
        // test that has already passed should not fail over a temporary directory.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Wait for a child to exit, or give up and say so.
fn wait_for_exit(process: &mut Child, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        match process.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return false,
        }
    }
    false
}

/// The parts of a connection the adapter needs, owned so a `Target` can borrow them.
struct Connection {
    engine: Engine,
    host: String,
    port: u16,
    database: String,
    user: String,
}

impl Connection {
    fn target<'a>(&'a self, password: &'a Secret) -> Target<'a> {
        Target {
            engine: self.engine,
            host: &self.host,
            port: self.port,
            database: &self.database,
            user: &self.user,
            password,
        }
    }
}

/// Quote a string for SQL. Both the apostrophes and the backslashes: MySQL reads a
/// backslash as an escape inside a literal, so doubling only the apostrophes is not enough.
fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

/// A port with nothing on it.
///
/// Stepping a counter is not enough on its own. Two test binaries can run at once, and a
/// server left behind by a killed run holds its port until something takes it away — and
/// the failure that produces, `Bind on TCP/IP port`, says nothing about either cause.
/// Binding it here first is the question actually being asked.
fn a_free_port() -> u16 {
    for _ in 0..200 {
        let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port in the range these tests use");
}

/// Shut down and delete any server a previous run left behind.
///
/// A test process that is killed — by a timeout, by Ctrl-C, by a harness that loses its
/// result — never runs `Drop`, and a server that outlives it holds a port and eats memory
/// until somebody notices. Sweeping on the way in means a killed run repairs itself on the
/// next one instead of needing a person.
///
/// The port is in the directory's own name and the administrator's password is a constant,
/// so the leftover can simply be asked to stop — no process table, no platform-specific way
/// of killing something by number. One that will not answer is gone already, and only its
/// directory is left to remove.
///
/// Only other processes' leftovers: anything belonging to this one is in use.
fn sweep_stale_servers(family: Family, binaries: &Path) {
    let mine = format!("sloop-my-{}-", std::process::id());
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };

    let program = binaries.join(match family {
        Family::Mysql => "mysql",
        Family::Mariadb => "mariadb",
    });

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("sloop-my-") || name.starts_with(&mine) {
            continue;
        }

        if let Some(port) = name
            .rsplit('-')
            .next()
            .and_then(|port| port.parse::<u16>().ok())
        {
            let _ = Command::new(&program)
                .arg("--no-defaults")
                .arg("--protocol=TCP")
                .arg("--host=127.0.0.1")
                .arg(format!("--port={port}"))
                .arg("--user=sloop_admin")
                .arg("--connect-timeout=3")
                .arg("--execute")
                .arg("SHUTDOWN")
                .env("MYSQL_PWD", ADMIN_PASSWORD)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        let _ = std::fs::remove_dir_all(entry.path());
    }
}

/// Why these tests have a server to talk to at all. See the PostgreSQL harness for the
/// reasoning; it is the same distinction.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// The variable named it.
    Explicitly,
    /// It turned up on `PATH` or in a package manager's directory.
    ByLookingAround,
}

/// Skip, or fail — whichever the caller earned.
fn refuse_or_skip(family: Family, asked: Asked, why: &str) -> Option<Server> {
    assert!(
        asked == Asked::ByLookingAround,
        "{} named a server that will not run: {why}",
        bin_dir_var(family)
    );
    eprintln!("skipping the {} cluster tests: {why}", family.engine());
    None
}

/// The first line any of these had to say.
fn first_line(stdout: &[u8], stderr: &[u8], log: &str) -> String {
    for stream in [stderr, stdout] {
        let text = String::from_utf8_lossy(stream);
        if let Some(line) = text.lines().map(str::trim).find(|line| {
            !line.is_empty() && !line.to_ascii_lowercase().contains("using a password")
        }) {
            return line.to_owned();
        }
    }
    log.to_owned()
}

/// The variable that says which server to test against.
const fn bin_dir_var(family: Family) -> &'static str {
    match family {
        Family::Mysql => "SLOOP_TEST_MYSQL_BIN",
        Family::Mariadb => "SLOOP_TEST_MARIADB_BIN",
    }
}

/// Print which server is about to be exercised.
///
/// Without this a passing run says nothing about *what* it proved.
fn announce(daemon: &Path) {
    let said = Command::new(daemon)
        .arg("--no-defaults")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    eprintln!("cluster tests are using {said}");
}

/// Where the client programs are, where the daemon is, and how hard the caller asked.
///
/// The variable names a directory of **client** programs, and the daemon is looked for
/// beside them, then in the sibling `sbin`, then on `PATH`. Debian and Ubuntu put `mysqld`
/// in `/usr/sbin` and everything else in `/usr/bin`, so one directory is not enough to
/// describe an installation and two variables would be one too many to remember.
fn find_server_binaries(family: Family) -> Option<(PathBuf, PathBuf, Asked)> {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let daemons: &[&str] = match family {
        Family::Mysql => &["mysqld"],
        Family::Mariadb => &["mariadbd", "mysqld"],
    };

    let find_daemon = |binaries: &Path| -> Option<PathBuf> {
        let mut roots = vec![binaries.to_path_buf()];
        if let Some(parent) = binaries.parent() {
            roots.push(parent.join("sbin"));
            roots.push(parent.join("libexec"));
        }
        for root in roots {
            for name in daemons {
                let candidate = root.join(format!("{name}{exe}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    };

    if let Some(named) = std::env::var_os(bin_dir_var(family)) {
        let binaries = PathBuf::from(named);
        let daemon = find_daemon(&binaries);
        assert!(
            daemon.is_some(),
            "{} is set to {} but there is no {} beside it or in the sibling sbin. These \
             tests were asked for a specific server, so not finding it is a failure and \
             not something to skip past.",
            bin_dir_var(family),
            binaries.display(),
            daemons[0],
        );
        return Some((binaries, daemon?, Asked::Explicitly));
    }

    // `PATH`, then the places a package manager puts them.
    let on_path = PathBuf::new();
    if let Some(daemon) = find_daemon(&on_path).or_else(|| {
        daemons.iter().find_map(|name| {
            Command::new(name)
                .arg("--version")
                .stdin(Stdio::null())
                .output()
                .is_ok_and(|output| output.status.success())
                .then(|| PathBuf::from(*name))
        })
    }) {
        return Some((on_path, daemon, Asked::ByLookingAround));
    }

    let roots: &[&str] = match family {
        Family::Mysql => &["/usr/local/mysql", "C:/Program Files/MySQL"],
        Family::Mariadb => &[
            "/usr/local/mariadb",
            "/opt/homebrew/opt",
            "C:/Program Files/MariaDB",
        ],
    };

    let mut found: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join("bin");
            if find_daemon(&candidate).is_some() {
                found.push(candidate);
            }
        }
    }

    // Newest last, so the highest version wins.
    found.sort();
    let binaries = found.pop()?;
    let daemon = find_daemon(&binaries)?;
    Some((binaries, daemon, Asked::ByLookingAround))
}

fn skip(family: Family, reason: &str) {
    eprintln!("skipping the {} cluster test: {reason}", family.engine());
}

/// Every table and its exact count, named without the database in front of it.
///
/// The qualifier is dropped on purpose. MySQL has no schemas and puts the *database* where
/// PostgreSQL puts the schema, so a copy of `source_db` into `target_db` is qualified
/// differently on the two sides by definition — comparing the qualified names would be
/// asserting that a correct copy is wrong. What has to match is the tables and their rows.
fn counts(adapter: &MysqlFamily, target: &Target<'_>) -> Vec<(String, u64)> {
    adapter
        .row_counts(target)
        .expect("counting rows")
        .into_iter()
        .map(|count| (count.table.name, count.rows))
        .collect()
}

/// The whole of R5's "done when", in order, against one server.
///
/// One function, two engines: MySQL and MariaDB differ in the programs they ship and not in
/// what a round-trip has to prove, and writing the assertions twice would be writing two
/// things that drift.
fn a_round_trip_keeps_the_schema(family: Family) {
    let Some(server) = Server::start(family, "roundtrip") else {
        skip(family, "no server on this machine");
        return;
    };

    let adapter = MysqlFamily::new(family, server.tools());
    let alpha_password = Secret::new(ALPHA_PASSWORD.to_owned());
    let beta_password = Secret::new(BETA_PASSWORD.to_owned());
    let source = server.database("source_db", "alpha");
    let destination = server.database("target_db", "beta");
    let source_target = source.target(&alpha_password);
    let destination_target = destination.target(&beta_password);

    // --- connect, as two different roles with two different passwords ---------------
    let reported = adapter.probe(&source_target).expect("probing as alpha");
    assert_eq!(reported.engine, family.engine());
    assert!(
        reported.version.major >= 5,
        "implausible version {}",
        reported.version
    );
    // What the adapter says about the connection has to be what the server says about it,
    // checked through the fixture's own client rather than against a constant. Both current
    // engines generate a certificate for themselves and so come up encrypted; the other
    // half of the rule — plain when the server offers nothing — has a server of its own, in
    // `a_server_without_tls_is_still_reachable`.
    assert_eq!(
        reported.tls,
        server.connection_is_encrypted(),
        "the adapter and the server disagree about whether the connection is encrypted"
    );

    let other = adapter.probe(&destination_target).expect("probing as beta");
    assert_eq!(other.version, reported.version);

    // --- what this engine cannot do, said out loud -----------------------------------
    assert!(
        !adapter.capabilities().custom_format,
        "mysqldump writes SQL text; there is no archive to read selectively"
    );
    assert!(!adapter.capabilities().parallel_restore);

    // --- what is there, and exactly how much of it ----------------------------------
    let tables = adapter.tables(&source_target).expect("listing tables");
    let listed: Vec<String> = tables.iter().map(ToString::to_string).collect();
    assert!(
        listed.contains(&"source_db.widgets".to_owned()),
        "{listed:?}"
    );
    assert!(listed.contains(&"source_db.notes".to_owned()), "{listed:?}");
    // A view is not a table and must not be counted as one.
    assert!(
        !listed.contains(&"source_db.recent".to_owned()),
        "{listed:?}"
    );

    let before = counts(&adapter, &source_target);
    assert_eq!(
        before,
        vec![
            ("notes".to_owned(), 7),
            ("quotes".to_owned(), 2),
            ("widgets".to_owned(), 8),
        ],
        "exact counts, not estimates"
    );

    // The destination is empty to begin with.
    assert_eq!(counts(&adapter, &destination_target), Vec::new());

    // --- dump, as alpha --------------------------------------------------------------
    let dump_path = server.root.join("source.sql");
    let summary = adapter
        .dump(&source_target, &dump_path)
        .expect("dumping as alpha");
    assert!(summary.bytes > 0, "the dump is empty");
    assert!(dump_path.is_file());

    let dump = std::fs::read(&dump_path).expect("reading the dump");
    assert_eq!(
        summary.bytes,
        dump.len() as u64,
        "the summary and the file disagree about the size"
    );
    // SQL text, not an archive. The capability said so; this is the file agreeing.
    //
    // Either comment opener. MySQL leads with `-- MySQL dump`, MariaDB 11 puts its
    // sandbox-mode pragma first, and both are a text file beginning with a comment rather
    // than a container with a magic number in front of it.
    assert!(
        dump.starts_with(b"--") || dump.starts_with(b"/*"),
        "a dump of SQL begins with a comment, and this begins {:?}",
        String::from_utf8_lossy(&dump[..dump.len().min(40)])
    );
    // Written on Windows or on Linux, the same database produces the same bytes.
    assert!(!dump.contains(&b'\r'), "the dump carries carriage returns");
    // Every object type R5 names is in there.
    let text = String::from_utf8_lossy(&dump);
    for object in ["CREATE TABLE", "VIEW", "TRIGGER", "FUNCTION", "EVENT"] {
        assert!(
            text.to_uppercase().contains(object),
            "the dump has no {object} in it"
        );
    }
    // And no account is stamped on any of them. This is what lets beta restore it.
    //
    // Per statement, not over the whole file: one of the rows in `quotes` is deliberately a
    // string that reads exactly like a DEFINER line, and a whole-file search would match
    // that row and call the filter broken when it is working.
    let stamped: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("/*!") && line.contains("DEFINER="))
        .collect();
    assert!(
        stamped.is_empty(),
        "a DEFINER survived into the dump: {stamped:#?}"
    );

    // Nothing was written to the source: it still has exactly what it had.
    assert_eq!(counts(&adapter, &source_target), before);

    // --- restore, as beta, into a database alpha has no rights over -------------------
    adapter
        .restore(&destination_target, &dump_path)
        .expect("restoring as beta");

    let after = counts(&adapter, &destination_target);
    assert_eq!(after, before, "the copy does not match the source");

    // --- and the schema arrived, not just the rows ------------------------------------
    assert_eq!(
        server.ask(
            "target_db",
            "SELECT COUNT(*) FROM information_schema.VIEWS WHERE TABLE_SCHEMA='target_db'"
        ),
        "1",
        "the view did not survive"
    );
    assert_eq!(
        server.ask(
            "target_db",
            "SELECT COUNT(*) FROM information_schema.TRIGGERS \
             WHERE TRIGGER_SCHEMA='target_db'"
        ),
        "1",
        "the trigger did not survive"
    );
    assert_eq!(
        server.ask(
            "target_db",
            "SELECT COUNT(*) FROM information_schema.ROUTINES \
             WHERE ROUTINE_SCHEMA='target_db'"
        ),
        "1",
        "the routine did not survive"
    );
    assert_eq!(
        server.ask(
            "target_db",
            "SELECT COUNT(*) FROM information_schema.EVENTS WHERE EVENT_SCHEMA='target_db'"
        ),
        "1",
        "the event did not survive"
    );
    // And the objects belong to the account that restored them, not to the one that dumped
    // them. This is the whole point of taking the DEFINER off: the destination has its own
    // accounts, and `alpha` is a name this database has never needed to know.
    assert_eq!(
        server.ask(
            "target_db",
            "SELECT DEFINER FROM information_schema.VIEWS WHERE TABLE_SCHEMA='target_db'"
        ),
        "beta@%",
        "the restored view is still stamped with the account that dumped it"
    );
    assert_eq!(
        server.ask(
            "target_db",
            "SELECT DEFINER FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA='target_db'"
        ),
        "beta@%",
        "the restored routine is still stamped with the account that dumped it"
    );

    // The view is a view and not a table someone flattened: it still answers.
    assert_eq!(
        server.ask("target_db", "SELECT COUNT(*) FROM recent"),
        server.ask("source_db", "SELECT COUNT(*) FROM recent"),
        "the view does not return what it returned on the source"
    );
    // The trigger fires on the destination, which a copied CREATE statement would not.
    server
        .sql("target_db", "INSERT INTO widgets (name) VALUES ('shouty');")
        .expect("inserting through the restored trigger");
    assert_eq!(
        server.ask(
            "target_db",
            "SELECT name FROM widgets WHERE name IN ('SHOUTY','shouty')"
        ),
        "SHOUTY",
        "the trigger did not fire"
    );
    // The routine runs.
    assert_eq!(
        server.ask("target_db", "SELECT widget_count()"),
        "9",
        "the routine did not come across"
    );

    // --- and the rows that would catch a careless dump filter came through untouched ---
    //
    // Compared against the source rather than against a literal: the client escapes
    // backslashes and tabs on its way out in batch mode, so a literal here would be
    // asserting something about the client's output format rather than about the copy.
    // The `LIKE` underneath is what proves the row really does hold the text the filter
    // hunts for, which is the whole reason this row exists.
    for id in [1, 2] {
        let question = format!("SELECT body FROM quotes WHERE id = {id}");
        assert_eq!(
            server.ask("target_db", &question),
            server.ask("source_db", &question),
            "row {id} of `quotes` did not survive the round trip"
        );
    }
    assert_eq!(
        server.ask(
            "source_db",
            r"SELECT body LIKE '%DEFINER=%' FROM quotes WHERE id = 1"
        ),
        "1",
        "the row that is supposed to look like a DEFINER line does not"
    );
    assert_eq!(
        server.ask(
            "target_db",
            r"SELECT body LIKE '%DEFINER=%' FROM quotes WHERE id = 1"
        ),
        "1",
        "the dump filter reached into a row of data"
    );
}

/// The paths that go wrong, on the same kind of server.
fn the_failures_are_told_apart(family: Family) {
    let Some(server) = Server::start(family, "failures") else {
        skip(family, "no server on this machine");
        return;
    };

    let adapter = MysqlFamily::new(family, server.tools());
    let source = server.database("source_db", "alpha");
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
    // And the line it shows is the real one, not the advice both tools print about
    // passwords on every single invocation.
    assert!(
        !failure.message().to_lowercase().contains("insecure"),
        "the password warning was mistaken for the failure: {failure:?}"
    );

    // --- a password that is right still works, so the test above proves something -----
    adapter
        .probe(&source.target(&right))
        .expect("the right password connects");

    // --- nothing listening is also code 3 --------------------------------------------
    let nowhere = Connection {
        engine: family.engine(),
        host: "127.0.0.1".to_owned(),
        port: 1,
        database: "source_db".to_owned(),
        user: "alpha".to_owned(),
    };
    let failure = adapter
        .probe(&nowhere.target(&right))
        .expect_err("port 1 has nothing on it");
    assert_eq!(failure.exit().code(), 3, "{failure:?}");

    // --- a database that is not there is code 3 too -----------------------------------
    let missing = server.database("no_such_db", "alpha");
    let failure = adapter
        .probe(&missing.target(&right))
        .expect_err("there is no such database");
    assert_eq!(failure.exit().code(), 3, "{failure:?}");

    // --- the other family is refused before anything is dumped ------------------------
    // This is what two engines buys: mysqldump 8 against a MariaDB server fails deep
    // inside the dump, and here it is a sentence beforehand.
    let other = match family {
        Family::Mysql => Family::Mariadb,
        Family::Mariadb => Family::Mysql,
    };
    let wrong_engine = Connection {
        engine: other.engine(),
        ..server.database("source_db", "alpha")
    };
    let mistaken = MysqlFamily::new(other, server.tools());
    let dump_path = server.root.join("never-written.sql");
    let failure = mistaken
        .dump(&wrong_engine.target(&right), &dump_path)
        .expect_err("a MariaDB adapter must not dump a MySQL server, or the other way");
    assert_eq!(failure.exit().code(), 2, "{failure:?}");
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("register it as")),
        "it has to say what to do about it: {failure:?}"
    );
    assert!(
        !dump_path.exists(),
        "it was refused and yet a file appeared: {}",
        dump_path.display()
    );

    // --- restoring a dump that is not there is code 5 ---------------------------------
    let failure = adapter
        .restore(
            &server.database("target_db", "beta").target(&right),
            Path::new("no-such.sql"),
        )
        .expect_err("there is no such dump");
    assert_eq!(failure.exit().code(), 5, "{failure:?}");
}

/// The other half of "TLS when the server offers it, plain when it does not".
///
/// The half that matters most, in fact. The databases this tool is for mostly sit inside a
/// private network with no certificate at all, and an adapter that quietly demanded TLS
/// would fail every one of them — while passing a test run against a server that happens to
/// have generated a certificate for itself.
fn a_server_without_tls_is_still_reachable(family: Family) {
    let Some(server) = Server::start_without_tls(family, "plain") else {
        skip(family, "no server on this machine");
        return;
    };

    let adapter = MysqlFamily::new(family, server.tools());
    let password = Secret::new(ALPHA_PASSWORD.to_owned());
    let source = server.database("source_db", "alpha");

    let reported = adapter
        .probe(&source.target(&password))
        .expect("a server with no TLS has to be reachable, not refused");

    assert!(
        !server.connection_is_encrypted(),
        "this server was supposed to have TLS turned off"
    );
    assert!(
        !reported.tls,
        "the adapter reported an encrypted connection to a server with TLS turned off"
    );

    // And it is a working connection, not merely one that answered a version query.
    assert!(
        !adapter
            .tables(&source.target(&password))
            .expect("listing tables over a plain connection")
            .is_empty()
    );
}

#[test]
fn mysql_connects_to_a_server_with_no_tls() {
    a_server_without_tls_is_still_reachable(Family::Mysql);
}

#[test]
fn mariadb_connects_to_a_server_with_no_tls() {
    a_server_without_tls_is_still_reachable(Family::Mariadb);
}

#[test]
fn mysql_round_trips_a_schema_with_views_triggers_and_routines() {
    a_round_trip_keeps_the_schema(Family::Mysql);
}

#[test]
fn mysql_tells_its_failures_apart() {
    the_failures_are_told_apart(Family::Mysql);
}

#[test]
fn mariadb_round_trips_a_schema_with_views_triggers_and_routines() {
    a_round_trip_keeps_the_schema(Family::Mariadb);
}

#[test]
fn mariadb_tells_its_failures_apart() {
    the_failures_are_told_apart(Family::Mariadb);
}

/// A tool that is not installed says so, and says where to look. No server needed.
#[test]
fn a_missing_client_tool_is_a_sentence_and_not_a_panic() {
    for (family, expected) in [
        (Family::Mysql, "not bundled"),
        // MariaDB's has one more thing to say, because the programs were called something
        // else until 10.5 and "MariaDB is not installed" would be wrong on such a machine.
        (Family::Mariadb, "mysqldump"),
    ] {
        let adapter = MysqlFamily::new(
            family,
            Tools {
                dump: PathBuf::from("dump-tool-that-is-not-installed"),
                client: PathBuf::from("client-that-is-not-installed"),
            },
        );

        let failure = adapter.client_version().expect_err("it is not there");
        assert_eq!(failure.exit().code(), 2);
        assert!(
            failure
                .message()
                .contains("dump-tool-that-is-not-installed")
        );
        assert!(
            failure
                .hint_text()
                .is_some_and(|hint| hint.contains(expected)),
            "{family:?}: {failure:?}"
        );
    }
}
