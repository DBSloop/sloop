//! Bringing up a MySQL or a MariaDB, which is nothing like bringing up a PostgreSQL.
//!
//! PostgreSQL ships `pg_ctl`, a supervisor that starts a cluster, waits for it to accept
//! connections and returns. Neither of these two has one. `mysqld` *is* the server: it runs
//! in the foreground until it is told to stop, it takes its settings from a file rather than
//! from arguments, and the account it is initialised with has no password at all until
//! something connects and gives it one. So the sequence here is its own:
//!
//! ```text
//! 1  write my.cnf          basedir, datadir, port, loopback, where the log goes
//! 2  bootstrap             mysqld --initialize-insecure, or MariaDB's install-db script
//! 3  start                 spawn it, every stream closed, and wait for it to answer
//! 4  give root a password  over a pipe, and prove it opens the server
//! ```
//!
//! **The two forks bootstrap differently and that is not a detail.** Oracle's `mysqld` takes
//! `--initialize-insecure`; MariaDB refuses it and ships `mariadb-install-db` instead, which
//! on Linux defaults the root account to `unix_socket` authentication — a server nothing
//! could log into with a password, which is exactly what sloop is about to try. So MariaDB
//! is told `--auth-root-authentication-method=normal`, by name, rather than discovered to be
//! unreachable afterwards.
//!
//! **Every program named here has two names.** MariaDB renamed all of them at 10.5 and made
//! the new names the real ones at 11, so `mariadbd` and `mysqld` are both looked for and the
//! first one that is there is used — the same rule [`crate::tools::Tool::also_known_as`]
//! already follows for the client programs, for the same reason.
//!
//! **No plaintext password reaches the disk or `ps`.** `mysqladmin password <secret>` would
//! put one in the process list and rule 3 names `ps` output in so many words, so the
//! statement goes down the client's standard input exactly as PostgreSQL's does.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;
use crate::server::LOOPBACK;

use super::Installed;

/// How long the server is given to come up before it is called a failure.
///
/// Generous, because the first start of a fresh data directory does real work, and bounded,
/// because a server that will not come up has to be a failure rather than a hang.
const START_TIMEOUT: Duration = Duration::from_secs(90);

/// How often it is asked whether it is up yet.
const POLL_EVERY: Duration = Duration::from_millis(250);

/// Bring one up and close it behind `password`.
pub fn raise(installed: &Installed, password: &Secret) -> Outcome<()> {
    write_my_cnf(installed)?;
    bootstrap(installed)?;
    start(installed)?;
    wait_until_it_answers(installed)?;
    set_root_password(installed, password)?;
    prove(installed, password)
}

/// Does this password still open it?
///
/// A question, not an assertion: a server that is stopped, or one whose port something else
/// has taken, is an answer the list prints rather than a failure it reports.
#[must_use]
pub fn answers(installed: &Installed, password: &Secret) -> bool {
    query(installed, Some(password), "SELECT 1;").is_ok()
}

/// `my.cnf`, which is where every setting lives because the command line is not where
/// `mysqld` looks for them.
///
/// **Loopback, and that is not a hardening choice rule 0d would override.** Nothing sloop
/// does reaches this server from anywhere else, and a database server that appeared on every
/// interface of somebody's laptop the moment they picked it out of a menu would be a
/// surprise nobody asked for. Somebody who wants it on the network edits one line of a file
/// sloop names for them.
fn write_my_cnf(installed: &Installed) -> Outcome<()> {
    let mut text = format!(
        "# Written by sloop when it installed this server. Edit it freely; sloop reads the\n\
         # connection details from its own record and never reads this file back.\n\
         [mysqld]\n\
         basedir = {}\n\
         datadir = {}\n\
         port = {}\n\
         bind-address = {LOOPBACK}\n\
         log-error = {}\n\
         pid-file = {}\n",
        display(&installed.home),
        display(&installed.data),
        installed.port,
        display(&log_file(installed)),
        display(&installed.home.join("mysqld.pid")),
    );

    // A unix socket beside the data, rather than the build's compiled-in `/var/run` default,
    // which belongs to a system account sloop is not running as. Windows has no unix
    // sockets, so there is nothing to say there.
    if !cfg!(windows) {
        let _ = writeln!(
            text,
            "socket = {}",
            display(&installed.home.join("mysql.sock"))
        );
    }

    std::fs::create_dir_all(&installed.home).map_err(|error| {
        Failure::usage(format!(
            "could not create {}: {error}",
            installed.home.display()
        ))
    })?;
    std::fs::write(my_cnf(installed), text).map_err(|error| {
        Failure::usage(format!(
            "could not write {}: {error}",
            my_cnf(installed).display()
        ))
    })
}

/// Create the data directory, with a root account that has no password yet.
///
/// **Some archives arrive with one already made.** MariaDB's Windows zip ships a `data`
/// holding the system schema, ready to be started — and running an install program over it
/// would either refuse or overwrite the thing that was shipped. A directory with `mysql` in
/// it is a data directory, and it is used as it stands.
fn bootstrap(installed: &Installed) -> Outcome<()> {
    if installed.data.join("mysql").is_dir() {
        crate::say!(
            "  {} {}",
            crate::style::label("Already made"),
            installed.data.display()
        );
        return Ok(());
    }

    crate::say!(
        "  {} {}",
        crate::style::label("Creating"),
        installed.data.display()
    );

    let (program, arguments) = bootstrap_command(installed)?;

    let said = Command::new(&program)
        .args(&arguments)
        .current_dir(&installed.home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| ran_nothing(&display(&program), &error.to_string()))?;

    if said.status.success() {
        return Ok(());
    }

    Err(Failure::new(
        Exit::Usage,
        format!(
            "could not initialise {} at {}",
            installed.describe(),
            installed.data.display()
        ),
    )
    .hint(told(&said.stdout, &said.stderr)))
}

/// Which program creates the data directory, and what it is told.
///
/// Oracle's server does it itself. MariaDB's will not — `--initialize` is a MySQL option it
/// does not have — and ships a script for it, under two names and in two places depending on
/// the platform and the version.
fn bootstrap_command(installed: &Installed) -> Outcome<(PathBuf, Vec<String>)> {
    if installed.engine == crate::engine::Engine::Mysql {
        let program = one_of(&installed.bin, &["mysqld"])?;
        return Ok((
            program,
            vec![
                format!("--defaults-file={}", display(&my_cnf(installed))),
                // Insecure means "no password", not "reachable": the account it makes is
                // root with an empty password, on a server that is not running yet, and the
                // next two steps start it on loopback and give it one. The alternative is
                // `--initialize`, which prints a temporary password to the error log — a
                // plaintext password on disk, which rule 3 has no exception for.
                "--initialize-insecure".to_owned(),
            ],
        ));
    }

    let program = one_of_in(
        &[installed.bin.clone(), installed.home.join("scripts")],
        &["mariadb-install-db", "mysql_install_db"],
    )?;

    // **Two programs with one name.** On Windows this is `mysql_install_db.exe`, a C program
    // from 2011 whose entire option list is `--datadir`, `--service`, `--password`, `--port`
    // and four more — it has no `--basedir` and refuses one by name, which is how the first
    // real run of this found out. Everywhere else it is the shell script, which takes
    // `--basedir` and needs `--auth-root-authentication-method=normal` because the root
    // account it would otherwise create authenticates by unix socket: a server nothing can
    // log into with a password, which is exactly what the next step is about to try.
    //
    // `--password` exists on the Windows one and is not used. It would put the password in
    // `ps`, which rule 3 forbids by name — the statement over a pipe two steps down is how
    // root gets one.
    let arguments = if cfg!(windows) {
        vec![format!("--datadir={}", display(&installed.data))]
    } else {
        vec![
            format!("--datadir={}", display(&installed.data)),
            format!("--basedir={}", display(&installed.home)),
            "--auth-root-authentication-method=normal".to_owned(),
            "--skip-test-db".to_owned(),
        ]
    };

    Ok((program, arguments))
}

/// Start it, detached, with every stream closed.
///
/// **Not a supervisor, because there is not one to use.** `mysqld` runs until it is stopped,
/// so it is spawned and left: the child outlives this process on both platforms, and dropping
/// the handle does not kill it. Every stream goes to the null device rather than being
/// inherited — an inherited stderr on Windows is held open for as long as the server runs,
/// and whatever is reading the parent's pipe then waits forever for a command that already
/// finished. What the server has to say goes to `log-error`, which is where anybody looking
/// at a failure wants it anyway.
fn start(installed: &Installed) -> Outcome<()> {
    let program = one_of(&installed.bin, &["mariadbd", "mysqld"])?;

    Command::new(&program)
        .arg(format!("--defaults-file={}", display(&my_cnf(installed))))
        .current_dir(&installed.home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| ran_nothing(&display(&program), &error.to_string()))?;

    Ok(())
}

/// Ask it, over and over, whether it is up — with its own client, not with a socket.
///
/// **A program rather than a connection, deliberately.** Everything in this tool that talks
/// to a database talks to it through the database's own client, which is what keeps `cargo
/// tree` free of anything that could speak a wire protocol. Polling with a `TcpStream` would
/// have been shorter and would have put the first line of network code in the binary.
fn wait_until_it_answers(installed: &Installed) -> Outcome<()> {
    let deadline = std::time::Instant::now() + START_TIMEOUT;

    loop {
        if query(installed, None, "SELECT 1;").is_ok() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(Failure::new(
                Exit::Connect,
                format!(
                    "{} did not start listening on {} within {} seconds",
                    installed.describe(),
                    installed.port,
                    START_TIMEOUT.as_secs()
                ),
            )
            .hint(last_words(&log_file(installed))));
        }
        std::thread::sleep(POLL_EVERY);
    }
}

/// Give root its password, over a pipe — **every** root, not only the one that connected.
///
/// A fresh bootstrap leaves more than one administrative account with an empty password:
/// `root@localhost`, and on Windows `root@127.0.0.1` and `root@::1` beside it. Closing the
/// one this connection happened to match would leave a database server on somebody's machine
/// that anything else on that machine could open — which is not a server sloop should be
/// willing to say it installed. So the accounts are asked for and every one of them is
/// closed.
///
/// **The statement is not the same in the two forks.** `ALTER USER … IDENTIFIED BY` is
/// MySQL 5.7's and MariaDB did not take it; `SET PASSWORD … = PASSWORD(…)` is MariaDB's all
/// the way to 11, and MySQL 8 removed the `PASSWORD()` function. There is no one spelling, so
/// there are two — and the first real run of this is what found that out.
fn set_root_password(installed: &Installed, password: &Secret) -> Outcome<()> {
    let literal = literal(password)?;
    let mut script = String::new();

    for account in administrative_accounts(installed)? {
        let statement = match installed.engine {
            crate::engine::Engine::Mysql => {
                format!("ALTER USER {account} IDENTIFIED BY {literal};")
            }
            _ => format!("SET PASSWORD FOR {account} = PASSWORD({literal});"),
        };
        let _ = writeln!(script, "{statement}");
    }

    if script.is_empty() {
        return Err(Failure::new(
            Exit::Connect,
            format!(
                "{} started, and has no {} account to give a password to",
                installed.describe(),
                installed.superuser
            ),
        )
        .hint("nothing has been written down — that server is not one sloop can use"));
    }

    script.push_str(
        "FLUSH PRIVILEGES;
",
    );
    query(installed, None, &script)
        .map(|_| ())
        .map_err(|failure| failure.at(Exit::Connect))
}

/// Every account named after the superuser, as `'root'@'localhost'`.
///
/// **What comes back is checked before it goes into a statement.** It is the server's own
/// answer rather than anybody's input, but a statement built out of a string is a statement
/// built out of a string — so anything that is not the shape this asked for is dropped, which
/// is the same guard [`literal`] makes about the password.
fn administrative_accounts(installed: &Installed) -> Outcome<Vec<String>> {
    // `installed.superuser` is one of two words this module chose itself, so it is not a
    // value anybody outside can steer.
    let said = query(
        installed,
        None,
        &format!(
            "SELECT CONCAT(QUOTE(user), '@', QUOTE(host)) FROM mysql.user WHERE user = '{}';",
            installed.superuser
        ),
    )?;

    Ok(said
        .lines()
        .map(str::trim)
        .filter(|line| looks_like_an_account(line, &installed.superuser))
        .map(ToOwned::to_owned)
        .collect())
}

/// Is this `'root'@'somewhere'`, and nothing else?
fn looks_like_an_account(line: &str, superuser: &str) -> bool {
    let Some(host) = line.strip_prefix(&format!("'{superuser}'@'")) else {
        return false;
    };
    let Some(host) = host.strip_suffix('\'') else {
        return false;
    };

    !host.is_empty()
        && host.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | ':' | '%' | '-' | '_')
        })
}

/// Prove the password opens it — the same connection every later run will make, made once
/// here while somebody is watching.
fn prove(installed: &Installed, password: &Secret) -> Outcome<()> {
    let version = query(installed, Some(password), "SELECT VERSION();")?;

    crate::say!(
        "  {} {} on {}",
        crate::style::label("Started"),
        version.lines().last().unwrap_or("it").trim(),
        installed.port
    );
    Ok(())
}

/// Run one statement through the server's own client, with the SQL on standard input.
fn query(installed: &Installed, password: Option<&Secret>, sql: &str) -> Outcome<String> {
    let program = one_of(&installed.bin, &["mariadb", "mysql"])?;

    let mut command = Command::new(&program);
    command
        .args(["--protocol=TCP", "-h", LOOPBACK])
        .args(["-P", &installed.port.to_string()])
        .args(["-u", &installed.superuser])
        .arg("--batch")
        .arg("--skip-column-names")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(secret) = password {
        // `MYSQL_PWD` on this child and nothing wider. The alternative, `-p<secret>`, is in
        // `ps` for the length of the statement, which rule 3 forbids by name.
        command.env("MYSQL_PWD", secret.expose());
    } else {
        // Rule 4: told there is no password rather than left to open the console and wait
        // for one that nobody is there to type.
        command.arg("--skip-password");
        command.env_remove("MYSQL_PWD");
    }

    let mut child = command
        .spawn()
        .map_err(|error| ran_nothing(&display(&program), &error.to_string()))?;

    {
        let mut pipe = child
            .stdin
            .take()
            .ok_or_else(|| Failure::usage("the client's standard input could not be opened"))?;
        pipe.write_all(sql.as_bytes())
            .map_err(|error| Failure::usage(format!("could not write to the client: {error}")))?;
        // Dropped here so the client sees end of file and runs, rather than waiting for more.
    }

    let output = child
        .wait_with_output()
        .map_err(|error| Failure::usage(format!("the client would not finish: {error}")))?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }

    Err(Failure::new(
        Exit::Connect,
        format!(
            "{}://{}@{LOOPBACK}:{} refused a statement",
            installed.engine.scheme(),
            installed.superuser,
            installed.port
        ),
    )
    .hint(told(&output.stdout, &output.stderr)))
}

/// A password as an SQL string literal, or a refusal.
///
/// The same guard `crate::server::make::literal` makes, one engine along and for the same
/// reason: the generated alphabet has no quote and no backslash in it, and this is what makes
/// that a guarantee rather than an assumption.
fn literal(password: &Secret) -> Outcome<String> {
    if password.expose().is_empty()
        || !password
            .expose()
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(Failure::new(
            Exit::Failure,
            "the generated password is not the alphabet sloop generates",
        ));
    }

    Ok(format!("'{}'", password.expose()))
}

/// Where the settings file is.
fn my_cnf(installed: &Installed) -> PathBuf {
    installed.home.join("my.cnf")
}

/// Where the server writes what it has to say.
fn log_file(installed: &Installed) -> PathBuf {
    installed.home.join("error.log")
}

/// The first of these names that is a program in `directory`.
fn one_of(directory: &Path, names: &[&str]) -> Outcome<PathBuf> {
    one_of_in(&[directory.to_path_buf()], names)
}

/// The same, across more than one directory — MariaDB puts its install script in `scripts`
/// in the source layout and in `bin` in the binary one, and which of those a release ships
/// has changed more than once.
fn one_of_in(directories: &[PathBuf], names: &[&str]) -> Outcome<PathBuf> {
    let suffixes: &[&str] = if cfg!(windows) {
        &[".exe", ""]
    } else {
        &["", ".sh"]
    };

    for directory in directories {
        for name in names {
            for suffix in suffixes {
                let candidate = directory.join(format!("{name}{suffix}"));
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err(Failure::new(
        Exit::Usage,
        format!(
            "the archive unpacked, but none of {} is in {}",
            names.join(", "),
            directories
                .iter()
                .map(|directory| display(directory))
                .collect::<Vec<_>>()
                .join(" or ")
        ),
    )
    .hint("that archive is not the one sloop expected — nothing has been started"))
}

/// A path as an argument, with the separators this platform's programs read.
///
/// **Backslashes are escape characters in `my.cnf`**, so a Windows path written into one
/// straight turns `\t` in a directory name into a tab. Forward slashes are accepted by every
/// one of these programs on every platform, which makes this one rule rather than two.
fn display(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// What a program said, both streams, for a hint.
fn told(stdout: &[u8], stderr: &[u8]) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .trim()
    .to_owned()
}

/// The end of the error log, for a failure that would otherwise only name it.
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

fn ran_nothing(program: &str, error: &str) -> Failure {
    Failure::new(Exit::Usage, format!("could not run {program}: {error}"))
        .hint("the programs in the archive have to be runnable by the account sloop is running as")
}
