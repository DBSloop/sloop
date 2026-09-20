//! Making sloop's own PostgreSQL 18 cluster, and talking to whichever one it ends up with.
//!
//! **No plaintext password ever reaches the disk or `ps`.** `initdb --pwfile` would write one
//! to a file, and `psql -c "ALTER ROLE … PASSWORD '…'"` would put one in the process list —
//! rule 3 forbids both in so many words. So the cluster is initialised with `trust`, bound to
//! loopback, given its password over a pipe, and only then switched to `scram-sha-256`. The
//! window in which it would take a connection without one is a few hundred milliseconds, on
//! `127.0.0.1`, on a port nothing else uses.
//!
//! **Everything here is a program, not a library.** `initdb`, `pg_ctl`, `psql`. There is no
//! PostgreSQL client crate in the dependency graph and there is not going to be one.

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::engine::Version;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::{Route, Secret};
use crate::style;

use super::{LOOPBACK, Origin, Ready, SLOOPS_PORT, SUPERUSER, Server, WANTED_MAJOR, find};

/// How the superuser password of a server sloop did not make is obtained.
///
/// Three ways in, in the order somebody would expect, and **no terminal means no question**:
/// rule 4 says a scheduled run must never stop on one, so without a terminal and without
/// either flag this fails and names them both.
pub struct Asking<'a> {
    /// `--superuser-password-command`: a password manager prints it.
    pub command: Option<&'a str>,
    /// `--superuser-password-stdin`: it is piped in.
    pub stdin: bool,
}

impl Asking<'_> {
    /// The password for a server that already exists.
    pub fn superuser_password(&self, server: &Server) -> Outcome<Secret> {
        if let Some(command) = self.command {
            return Ok(crate::secret::resolve(
                &Route::Command(command.to_owned()),
                &crate::secret::Lookup {
                    key: &server.superuser,
                    vault: &crate::secret::sealed::Vault::File(Path::new("")),
                },
            )?
            .secret);
        }

        if self.stdin {
            return find::from_stdin();
        }

        if !std::io::stdin().is_terminal() {
            return Err(Failure::new(
                Exit::Usage,
                format!(
                    "{}'s password is needed for the PostgreSQL already on this machine, and \
                     there is no terminal to ask at",
                    server.superuser
                ),
            )
            .hint(
                "pipe it in with --superuser-password-stdin, or have a password manager print \
                 it with --superuser-password-command",
            ));
        }

        // **Through the console, like every other question this tool asks.** From a shell
        // that is `rpassword` and nothing has changed; from the menu it is a box on the
        // menu's own screen, because the terminal is never handed back any more.
        let typed = crate::console::ask_hidden(&format!(
            "Password for {}@{LOOPBACK}:{}: ",
            server.superuser, server.port
        ))?;
        Ok(Secret::new(typed))
    }
}

/// Make — or reopen — the cluster sloop keeps under the global store.
///
/// The binaries come from whichever PostgreSQL 18 this machine can be given: one already on
/// it, or the one [`crate::tools::acquire`] fetches with consent. Either way the cluster is
/// sloop's, on [`SLOOPS_PORT`], and the machine's own PostgreSQL is not touched.
pub fn sloops_own(global: &Path) -> Outcome<Ready> {
    let bin = binaries(global)?;
    with_binaries(global, bin, SLOOPS_PORT)
}

/// The same, out of a `bin` directory and on a port somebody else chose.
///
/// **Split off so the cluster tests can drive the real sequence.** What is risky here is
/// `initdb`, then `pg_ctl`, then setting the password over a pipe, then switching to
/// `scram-sha-256`, then proving it — and that sequence is identical whichever major runs it.
/// A test that had to download a third of a gigabyte before it could check any of it would be
/// a test nobody runs, and one that built on 5433 would collide with the cluster a developer's
/// own `sloop setup` made.
pub(super) fn with_binaries(global: &Path, bin: std::path::PathBuf, port: u16) -> Outcome<Ready> {
    raise(Server {
        bin,
        data: Some(super::data_dir(global)),
        port,
        superuser: SUPERUSER.to_owned(),
        origin: Origin::Sloops,
    })
}

/// Bring a cluster up at the data directory a [`Server`] names, and hand back the password
/// that opens it.
///
/// **One sequence, two callers.** Setup builds sloop's own state cluster with it and
/// `R19d`'s menu builds the PostgreSQL somebody chose from a list with it — and the risky
/// part is identical either way: `initdb`, then `pg_ctl`, then the password over a pipe,
/// then `scram-sha-256`, then proving the connection. A second copy of that for the menu
/// would be a second chance to leave a cluster on `trust`, which is the one mistake here
/// that nobody would notice until somebody else did.
pub(crate) fn raise(server: Server) -> Outcome<Ready> {
    let data = server.data.clone().ok_or_else(|| {
        Failure::usage("this server names no data directory to create")
            .hint("a server sloop installed has one — `sloop server list` says which")
    })?;

    // An existing cluster with no record beside it: `server.toml` was deleted, or a previous
    // Setup stopped between `initdb` and writing it. Nothing here can recover its password,
    // so it is said plainly rather than silently initialised over — that directory is a
    // database, and rule 5's spirit says a thing like this is never destroyed quietly.
    if data.join("PG_VERSION").is_file() {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "{} is already a PostgreSQL cluster, but sloop has no record of its password",
                data.display()
            ),
        )
        .hint(format!(
            "delete {} to let sloop start again, or restore the server.toml that went with it",
            data.display()
        )));
    }

    let password = generated()?;
    initdb(&server, &data)?;
    start(&server)?;
    set_superuser_password(&server, &password)?;
    require_a_password_from_now_on(&server, &data)?;

    // Proof rather than hope: the same connection every later run will make, made once here
    // while somebody is watching.
    let Some(version) = server_version(&server, &password)? else {
        return Err(Failure::new(
            Exit::Connect,
            format!(
                "{} would not accept the password sloop just set",
                server.url("postgres")
            ),
        )
        .hint(
            "something is answering on that port that is not the cluster sloop just started \
             — `sloop server list` shows the port each one uses",
        ));
    };
    crate::say!(
        "  {} PostgreSQL {version} on {}",
        style::label("Started"),
        server.port
    );

    Ok(Ready {
        server,
        password,
        made_now: true,
    })
}

/// Start a cluster sloop made, if it is not already running.
///
/// The machine's own server is never touched by this — starting somebody else's PostgreSQL
/// is not sloop's decision to make.
pub fn start_if_stopped(server: &Server) -> Outcome<()> {
    if server.origin != Origin::Sloops {
        return Ok(());
    }
    if find::is_serving(&server.bin, server.port) {
        return Ok(());
    }
    start(server)
}

/// The PostgreSQL 18 binaries to build the cluster out of.
fn binaries(global: &Path) -> Outcome<std::path::PathBuf> {
    if let Some(found) = find::wanted(global) {
        crate::say!(
            "{} PostgreSQL {} at {}",
            style::heading("Using:"),
            found.version,
            found.bin.display()
        );
        return Ok(found.bin);
    }

    // **Say what is here before saying what is missing.** A machine with PostgreSQL 17 on it
    // being told "this machine has none" reads as sloop not looking, and the whole reason
    // 18 arrives *beside* 17 rather than instead of it is invisible unless the 17 is named.
    let others = find::servers(global);
    if let Some(newest) = others.first() {
        crate::say!(
            "{} PostgreSQL {} at {}, which sloop leaves exactly as it is.",
            style::heading("Already here:"),
            newest.version,
            newest.bin.display()
        );
    }

    crate::tools::acquire::postgres_server(global)?;

    let bin = super::fetched_bin(global);
    find::wanted(global).map(|found| found.bin).ok_or_else(|| {
        Failure::new(
            Exit::Usage,
            format!(
                "the PostgreSQL {WANTED_MAJOR} install finished but no server was found at {}",
                bin.display()
            ),
        )
        .hint("`sloop doctor` says where it looked")
    })
}

/// Create the cluster.
///
/// `trust` on purpose, and undone a few lines later by [`require_a_password_from_now_on`].
/// The alternative is `--pwfile`, which is a plaintext password written to disk, and rule 3
/// does not have an exception for "only for a moment".
fn initdb(server: &Server, data: &Path) -> Outcome<()> {
    if let Some(parent) = data.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Failure::usage(format!("could not create {}: {error}", parent.display()))
        })?;
    }

    crate::say!("  {} {}", style::label("Creating"), data.display());

    let status = Command::new(server.program("initdb"))
        .arg("-D")
        .arg(data)
        .args(["-U", &server.superuser])
        .args(["--encoding", "UTF8"])
        // C, so ordering is the same on every machine sloop runs on. Nothing in this cluster
        // is ever shown to somebody in their own language — it is one machine's own state.
        .args(["--locale", "C"])
        .args(["--auth-local", "trust", "--auth-host", "trust"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|error| ran_nothing("initdb", &error.to_string()))?;

    if !status.success() {
        return Err(Failure::new(
            Exit::Usage,
            format!("initdb could not create a cluster at {}", data.display()),
        )
        .hint("the directory has to be empty and writable by the account sloop is running as"));
    }

    Ok(())
}

/// Start it, on loopback and on sloop's own port.
fn start(server: &Server) -> Outcome<()> {
    let data = server.data.as_ref().ok_or_else(|| {
        Failure::usage("this server is not one sloop started")
            .hint("`sloop server list` shows the ones it did, and how to reach each")
    })?;

    let status = Command::new(server.program("pg_ctl"))
        .arg("-D")
        .arg(data)
        .arg("-o")
        .arg(format!(
            "-p {}{} -c listen_addresses={LOOPBACK}",
            server.port,
            no_unix_socket()
        ))
        .arg("-l")
        .arg(data.join("server.log"))
        // Wait for it to be accepting connections, rather than returning to a caller that
        // would then fail to connect to a server which is merely still starting. Bounded, so
        // a cluster that will not come up is a failure instead of a hang.
        .arg("--timeout=60")
        .arg("-w")
        .arg("start")
        .stdin(Stdio::null())
        // **Every stream to null, and the last one is not tidiness.** On Windows the
        // postmaster `pg_ctl` spawns inherits whatever handles it is given and holds them
        // open for as long as it runs — so an inherited stderr keeps the parent's pipe from
        // ever reaching end of file, and whatever is reading it waits for a server that is
        // not going to stop. The server's own output goes to the log file above, which is
        // where anything worth reading after a failure ends up anyway.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| ran_nothing("pg_ctl", &error.to_string()))?;

    if !status.success() {
        return Err(Failure::new(
            Exit::Connect,
            format!("pg_ctl could not start the cluster at {}", data.display()),
        )
        // **The log, not a path to it.** A failure that names a file somebody then has to go
        // and open is a failure nobody reads — and on a CI runner the directory is gone by
        // the time anybody looks. The last few lines are what says why.
        .hint(last_words(&data.join("server.log"))));
    }

    Ok(())
}

/// Turn the unix socket off, as an extra `postgres` option.
///
/// **sloop's cluster answers on `127.0.0.1` and nowhere else**, which [`LOOPBACK`] already
/// says is the whole design — so the unix socket is a file nothing ever connects to, and two
/// platforms make it actively harmful.
///
/// Debian and Ubuntu patch the default to `/var/run/postgresql`, a directory belonging to the
/// `postgres` system user that nobody else may write to, so the postmaster dies at startup on
/// the one platform where the package manager is the normal way to get PostgreSQL. And a
/// socket *path* is capped at about 104 bytes on macOS — `/var/folders/…/T/` plus a data
/// directory is past that before sloop has written a character — so pointing it at the data
/// directory trades one failure for another.
///
/// No socket, no path, no permission. Empty on Windows, which has no unix sockets at all.
fn no_unix_socket() -> &'static str {
    if cfg!(windows) {
        ""
    } else {
        " -c unix_socket_directories="
    }
}

/// The end of a log file, for a failure that would otherwise only name it.
fn last_words(log: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(log) else {
        return format!("{} would say why, and could not be read", log.display());
    };

    let tail: Vec<&str> = text.lines().rev().take(8).collect();
    if tail.is_empty() {
        return format!("{} is empty", log.display());
    }

    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// Give the superuser its password, over a pipe.
///
/// **Not `-c`.** An argument is in `ps` and in a shell's history, and rule 3 names `ps`
/// output specifically. Standard input is in neither.
fn set_superuser_password(server: &Server, password: &Secret) -> Outcome<()> {
    let statement = format!(
        "ALTER ROLE \"{}\" WITH PASSWORD {};",
        server.superuser,
        literal(password)?
    );

    let (status, said) = psql(server, &As::superuser(server), None, &statement, &[])?;
    if !status {
        return Err(Failure::new(
            Exit::Connect,
            format!("could not set {}'s password", server.superuser),
        )
        .hint(said));
    }

    Ok(())
}

/// Rewrite `pg_hba.conf` so nothing connects without a password, and reload.
///
/// Every `trust` becomes `scram-sha-256`. The file is rewritten wholesale rather than
/// appended to, because a later `trust` line further down would win over anything added at
/// the end and the cluster would still be open.
fn require_a_password_from_now_on(server: &Server, data: &Path) -> Outcome<()> {
    let path = data.join("pg_hba.conf");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| Failure::usage(format!("could not read {}: {error}", path.display())))?;

    let hardened: String = text
        .lines()
        .map(|line| {
            if line.trim_start().starts_with('#') {
                return line.to_owned();
            }
            // The method is the last field of a rule. Replacing the word anywhere else would
            // corrupt a database or role called `trust`, which is why this is not a
            // substring replace.
            match line.rsplit_once(char::is_whitespace) {
                Some((before, "trust")) => format!("{before} scram-sha-256"),
                _ => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    std::fs::write(&path, hardened + "\n")
        .map_err(|error| Failure::usage(format!("could not write {}: {error}", path.display())))?;

    let status = Command::new(server.program("pg_ctl"))
        .arg("-D")
        .arg(data)
        .arg("reload")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|error| ran_nothing("pg_ctl", &error.to_string()))?;

    if !status.success() {
        return Err(Failure::new(
            Exit::Connect,
            format!("could not reload the cluster at {}", data.display()),
        )
        .hint(last_words(&data.join("server.log"))));
    }

    Ok(())
}

/// What version this server says it is, or `None` when the password did not open it.
///
/// The one place a connection to sloop's own PostgreSQL is made in this task, so it is also
/// the one place `PGPASSWORD` is set — on the single child process and nowhere wider, which
/// is the rule `engine` already follows.
pub fn server_version(server: &Server, password: &Secret) -> Outcome<Option<Version>> {
    let (ok, said) = psql(
        server,
        &As::superuser(server),
        Some(password),
        "SHOW server_version;",
        &["--tuples-only", "--no-align"],
    )?;

    if !ok {
        // Told apart on purpose. A wrong password is an answer to the question being asked;
        // a server that is not there at all is a different problem and should not be
        // reported as a bad password.
        if said.contains("password authentication failed") || said.contains("no password supplied")
        {
            return Ok(None);
        }
        return Err(Failure::new(
            Exit::Connect,
            format!("{} would not answer", server.url("postgres")),
        )
        .hint(said));
    }

    Ok(Version::from_tool_output(&said))
}

/// Run one statement through `psql`, with the SQL on standard input.
///
/// Returns whether it succeeded and everything it said, so a caller can tell a wrong password
/// from a server that is not listening.
fn psql(
    server: &Server,
    as_who: &As<'_>,
    password: Option<&Secret>,
    sql: &str,
    extra: &[&str],
) -> Outcome<(bool, String)> {
    let mut command = Command::new(server.program("psql"));
    command
        .args(["-h", LOOPBACK])
        .args(["-p", &server.port.to_string()])
        .args(["-U", as_who.role])
        .args(["-d", as_who.database])
        .args(["-v", "ON_ERROR_STOP=1"])
        .arg("--no-psqlrc")
        // **Rule 4, and it is not theoretical.** Asked for a password it has not been given,
        // `psql` opens the *console* and waits — not standard input, so closing that does not
        // help, and a scheduled run would hang there forever with nobody to see the prompt.
        // This makes it fail and say so instead, which is what every caller here wants.
        .arg("--no-password")
        .args(extra)
        // Read the statement from the pipe below rather than from a file or an argument.
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match password {
        // On this child and nothing wider. `PGPASSWORD` in the parent's environment would be
        // inherited by every other program sloop runs.
        Some(secret) => {
            command.env("PGPASSWORD", secret.expose());
        }
        None => {
            command.env_remove("PGPASSWORD");
        }
    }

    let mut child = command
        .spawn()
        .map_err(|error| ran_nothing("psql", &error.to_string()))?;

    {
        let mut pipe = child.stdin.take().ok_or_else(|| {
            Failure::usage("psql's standard input could not be opened").report_a_bug()
        })?;
        pipe.write_all(sql.as_bytes())
            .map_err(|error| Failure::usage(format!("could not write to psql: {error}")))?;
        // Dropped here so psql sees end of file and runs, rather than waiting for more.
    }

    let output = child
        .wait_with_output()
        .map_err(|error| Failure::usage(format!("psql would not finish: {error}")))?;

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok((output.status.success(), said.trim().to_owned()))
}

/// A password for the superuser of a cluster that is about to exist.
///
/// Letters and digits, for the reason `db create` already settled: what the character set
/// buys is a password that survives being pasted into a connection string, a service file
/// and a shell without one escaping rule between them.
fn generated() -> Outcome<Secret> {
    Ok(Secret::new(crate::secret::generated_password()?))
}

fn ran_nothing(program: &str, error: &str) -> Failure {
    Failure::new(Exit::Usage, format!("could not run {program}: {error}"))
        .hint("PostgreSQL 18's programs have to be runnable by the account sloop is running as")
}

/// Who a connection is made as, and to what.
///
/// **Its own type because the pair travels together and getting them crossed is silent.**
/// Connecting as the owning role to `postgres`, or as the superuser to `sloop_database`,
/// both work and both do the wrong thing.
pub struct As<'a> {
    /// The role to connect as.
    pub role: &'a str,
    /// The database to connect to.
    pub database: &'a str,
}

impl<'a> As<'a> {
    /// The superuser, on the database every server has.
    ///
    /// `postgres` rather than sloop's own: this is the connection used *before* sloop's own
    /// database exists, and the one used to make it.
    #[must_use]
    pub fn superuser(server: &'a Server) -> Self {
        Self {
            role: &server.superuser,
            database: "postgres",
        }
    }
}

/// A password as an SQL string literal, or a refusal.
///
/// **Doubling the apostrophe is the whole rule**, exactly as `db create` has always doubled
/// one — see `engine::postgres::sql_literal`. So a password with a quote in it cannot close
/// the literal and there is no statement here anybody could steer, whatever the password
/// happens to contain.
///
/// It used to refuse anything outside the generated alphabet, which worked while a generated
/// password was the only kind that reached it. `R19f` let the owner supply
/// `sloop_db_admin`'s, and a real password has apostrophes in it — an alphabet guard there
/// would be a hardening choice that makes the feature refuse, which rule 0d settles the
/// other way. Escaping is what the guard was standing in for.
///
/// **Empty is still a refusal.** A password manager that printed nothing, or a pipe with
/// nothing in it, would otherwise set a blank password on a role that can log in — and that
/// is a mistake worth stopping rather than escaping.
pub fn literal(password: &Secret) -> Outcome<String> {
    if password.expose().is_empty() {
        return Err(Failure::new(
            Exit::Failure,
            "the password came back empty, and a role that can log in is not given one",
        )
        .hint(
            "whatever supplied it printed nothing — check `--superuser-password-command`, or type \
             the password instead",
        ));
    }

    Ok(format!("'{}'", password.expose().replace('\'', "''")))
}

/// Run one statement as the superuser, and fail with what the server said.
pub fn run_sql(server: &Server, password: Option<&Secret>, sql: &str) -> Outcome<()> {
    let (ok, said) = psql(server, &As::superuser(server), password, sql, &[])?;
    if ok {
        return Ok(());
    }

    Err(Failure::new(
        Exit::Connect,
        format!("{} refused a statement", server.url("postgres")),
    )
    .hint(said))
}

/// Ask the server one question and hand back what it answered, with nothing around it.
pub fn query(server: &Server, password: Option<&Secret>, sql: &str) -> Outcome<String> {
    ask(server, &As::superuser(server), password, sql)
}

/// The same question, asked as somebody in particular.
///
/// **`R19c3` reads sloop's own tables as the role that owns them, not as the superuser.**
/// Same connection, same rules about the password; the only thing that changes is who is on
/// the other end of it.
pub fn ask(
    server: &Server,
    as_who: &As<'_>,
    password: Option<&Secret>,
    sql: &str,
) -> Outcome<String> {
    let (ok, said) = psql(
        server,
        as_who,
        password,
        sql,
        &["--tuples-only", "--no-align"],
    )?;

    if ok {
        return Ok(said);
    }

    Err(Failure::new(
        Exit::Connect,
        format!(
            "{} would not answer",
            server.url_as(as_who.role, as_who.database)
        ),
    )
    .hint(said))
}

/// Run a whole script as somebody in particular — all of it, or none of it.
///
/// **`--single-transaction`, which is the half that matters.** A migration and the ledger row
/// that records it go in together: a script that fails on its fourth statement leaves no
/// trace of the first three, so the next run finds a database at the version it was at rather
/// than at half of the next one.
pub fn script(
    server: &Server,
    as_who: &As<'_>,
    password: Option<&Secret>,
    sql: &str,
) -> Outcome<()> {
    let (ok, said) = psql(server, as_who, password, sql, &["--single-transaction"])?;
    if ok {
        return Ok(());
    }

    Err(Failure::new(
        Exit::Connect,
        format!("{} refused a statement", server.url(as_who.database)),
    )
    .hint(said))
}

/// Does this role and password open this database?
///
/// **A wrong password is an answer, not an error.** It is what says a role has to be given a
/// new one. A server that will not answer at all is a different thing and comes back as a
/// failure, because acting on "the password is wrong" when the truth is "nothing is
/// listening" would reset a password for no reason.
pub fn connects_as(
    server: &Server,
    role: &str,
    database: &str,
    password: &Secret,
) -> Outcome<bool> {
    let (ok, said) = psql(
        server,
        &As { role, database },
        Some(password),
        "SELECT 1;",
        &["--tuples-only", "--no-align"],
    )?;

    if ok {
        return Ok(true);
    }
    if said.contains("password authentication failed")
        || said.contains("no password supplied")
        || said.contains("does not exist")
    {
        return Ok(false);
    }

    Err(Failure::new(
        Exit::Connect,
        format!("{} would not answer", server.url_as(role, database)),
    )
    .hint(said))
}
