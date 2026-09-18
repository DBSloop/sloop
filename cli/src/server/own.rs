//! `sloop_database`, owned by `sloop_db_admin` — the database sloop keeps its own state in.
//!
//! **`R19c1` found a server; this puts sloop's own database on it.** What goes *inside* it —
//! the tables and the migrations that create them — is [`super::schema`]. What is here is the
//! database, the role that owns it, the password that role gets, and the one property that
//! makes a second run ordinary: nothing is typed.
//!
//! **It runs as a guest wherever the server came from.** A cluster sloop made has one
//! superuser and nothing else on it; the machine's own PostgreSQL 18 may have a hundred
//! databases and other people's roles beside them. So every step asks before it acts —
//! `CREATE ROLE` only when there is no such role, `CREATE DATABASE` only when there is no
//! such database — and nothing that already exists is dropped, reassigned or reset. Setup is
//! re-runnable because of that, not in spite of it.
//!
//! **Neither password is in a file.** The role's password is generated here and goes
//! wherever this machine keeps secrets, exactly as the superuser's does; `server.toml` holds
//! the *route*, which is a word like `keyring`. The connection details have to live outside
//! the database because they are what opens it, and that is the whole of the exception.
//!
//! **`R19f` made the role's password the owner's to choose.** `sloop_db_admin` is what a
//! person opens `sloop_database` with in DataGrip, so it is theirs as much as sloop's: two
//! flags supply one, a terminal is asked, and only a run that was given no answer generates
//! one. Whichever way it arrives it lands in the same place and the record still holds a
//! route — what changes is whether anybody already knows the value.

use std::io::IsTerminal as _;
use std::path::Path;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::{Route, Secret};
use crate::style;

use super::{LOOPBACK, Server, find, make};

/// The database sloop keeps its state in.
pub const DATABASE: &str = "sloop_database";

/// The role that owns it. Not the superuser: everything sloop does to its own state it does
/// as an ordinary owner, so a bug here cannot reach the rest of the server.
pub const ROLE: &str = "sloop_db_admin";

/// How sloop reaches its own database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Own {
    /// The database's name on the server.
    pub database: String,
    /// The role sloop connects as.
    pub role: String,
}

impl Own {
    /// The one sloop uses.
    #[must_use]
    pub fn sloops() -> Self {
        Self {
            database: DATABASE.to_owned(),
            role: ROLE.to_owned(),
        }
    }

    /// How a connection to it is spelled, with no password anywhere in it.
    #[must_use]
    pub fn url(&self, server: &Server) -> String {
        format!(
            "postgres://{}@{LOOPBACK}:{}/{}",
            self.role, server.port, self.database
        )
    }

    /// Who a connection to it is made as, and to what.
    ///
    /// **Both halves together, because getting them crossed is silent.** Connecting as the
    /// owning role to `postgres`, or as the superuser to `sloop_database`, both work and both
    /// do the wrong thing — `R19c3` reads and writes sloop's tables as the role that owns
    /// them, and this is the one place that pair is spelled.
    #[must_use]
    pub fn as_who(&self) -> make::As<'_> {
        make::As {
            role: &self.role,
            database: &self.database,
        }
    }

    /// What the role's password is filed under.
    ///
    /// Derived from the connection rather than invented, the way a registered database's key
    /// is: two sloops on one machine with clusters on different ports keep different
    /// passwords, and neither can read the other's by accident.
    #[must_use]
    pub fn credential_key(&self, server: &Server) -> String {
        format!(
            "sloop-database:{}@{LOOPBACK}:{}/{}",
            self.role, server.port, self.database
        )
    }
}

/// Where `sloop_db_admin`'s password comes from, when one has to be settled.
///
/// **Four answers, in the order somebody would expect them.** Two flags for a run with
/// nobody to ask, a hidden prompt for a run with somebody, and a generated one for anybody
/// who presses Enter — which is every ordinary Setup.
///
/// The order matters at exactly one point and it is rule 4: **no terminal means no
/// question.** Unlike `db create`, though, a run with no terminal still generates one rather
/// than refusing. There the password is needed by an application and sloop keeps a copy only
/// incidentally, so one that could never be printed is one nobody could use; here sloop
/// keeps it, `sloop server connection --show-password` reads it back whenever somebody wants
/// it, and a headless `sloop setup` that refused to finish over a password it can hand over
/// later would be refusing for no reason at all.
pub struct Choosing<'a> {
    /// `--db-password-command`: a password manager prints it.
    pub command: Option<&'a str>,
    /// `--db-password-stdin`: it is piped in.
    pub stdin: bool,
}

impl Choosing<'_> {
    /// Nothing was supplied, which is every Setup that names neither flag.
    #[must_use]
    pub const fn unsupplied() -> Self {
        Self {
            command: None,
            stdin: false,
        }
    }

    /// A password somebody gave on the command line, if they gave one.
    ///
    /// **Separate from [`Self::or_generated`] because it outranks a password already on the
    /// role.** A flag is an instruction rather than a preference: a second Setup naming one
    /// is how a password gets rotated, and quietly keeping the old one because it still
    /// worked would be ignoring what was asked for.
    fn supplied(&self, role: &str) -> Outcome<Option<Secret>> {
        if let Some(command) = self.command {
            return Ok(Some(
                crate::secret::resolve(
                    &Route::Command(command.to_owned()),
                    &crate::secret::Lookup {
                        key: role,
                        vault: &crate::secret::sealed::Vault::File(Path::new("")),
                    },
                )?
                .secret,
            ));
        }

        if self.stdin {
            return Ok(Some(find::from_stdin()?));
        }

        Ok(None)
    }

    /// The password to give the role when nobody supplied one: typed, or invented.
    ///
    /// The `bool` is whether sloop invented it, which is the whole of what Setup prints at
    /// the end — a password somebody typed is one they already have.
    fn or_generated(role: &str) -> Outcome<(Secret, bool)> {
        if !std::io::stdin().is_terminal() {
            return Ok((Secret::new(crate::secret::generated_password()?), true));
        }

        match crate::secret::typed_twice(role)? {
            Some(typed) => Ok((typed, false)),
            None => Ok((Secret::new(crate::secret::generated_password()?), true)),
        }
    }
}

/// sloop's own database, the way into it, and whether anybody has seen that way yet.
#[derive(Debug)]
pub struct Opened {
    /// The database and the role that owns it.
    pub own: Own,
    /// The password that opens them.
    pub password: Secret,
    /// Where that password is kept on this machine — the route, never the value.
    pub route: Route,
    /// True when sloop invented this password in this run.
    ///
    /// **The one case there is something to show.** A password that was supplied is already
    /// in the hands of whoever supplied it, and one read back out of the keyring was shown
    /// when it was made — so this is what says Setup is holding a password nobody has ever
    /// seen, which is the only moment rule 3 leaves room to print one.
    pub generated: bool,
}

/// Make sure `sloop_database` and `sloop_db_admin` exist, and hand back the way in.
///
/// **Idempotent, and that is the `Done when`.** A second run finds both already there, reads
/// the password back from wherever this machine keeps secrets, proves the connection, and
/// returns — with nothing typed and nothing created twice.
pub fn ensure(
    global: &Path,
    server: &Server,
    superuser: &Secret,
    choosing: &Choosing<'_>,
) -> Outcome<Opened> {
    // Whatever a previous run settled on, and only otherwise the name sloop would pick today.
    // See `record::recorded_own`.
    let own = super::record::recorded_own(global)?.unwrap_or_else(Own::sloops);
    let sealed = global.join(crate::registry::file::SEALED_FILE);

    // What a previous run left, if anything. A password sloop cannot read back is the same
    // as no password at all: the role is reset below rather than guessed at.
    let known = super::record::database_route(global)?
        .and_then(|route| {
            crate::secret::resolve(
                &route,
                &crate::secret::Lookup {
                    key: &own.credential_key(server),
                    vault: &crate::secret::sealed::Vault::File(&sealed),
                },
            )
            .ok()
        })
        .map(|resolved| resolved.secret);

    let role_exists = exists(
        server,
        superuser,
        &format!("SELECT 1 FROM pg_roles WHERE rolname = '{}';", own.role),
    )?;

    // **Asked before anything else is decided.** Resolving `--db-password-command` runs
    // somebody's password manager, and finding out it is locked *after* a role has been
    // created is finding out too late.
    let supplied = choosing.supplied(&own.role)?;

    // A password only where one is needed: an existing role whose password sloop can still
    // read is left exactly as it is, because resetting it would break anything else that had
    // been given it.
    let (password, generated) = match (supplied, role_exists, known) {
        (Some(given), exists, _) => {
            set_password(server, superuser, &own, &given, exists)?;
            (given, false)
        }
        (None, true, Some(known)) if opens(server, &own, &known)? => (known, false),
        (None, exists, _) => {
            let (chosen, generated) = Choosing::or_generated(&own.role)?;
            set_password(server, superuser, &own, &chosen, exists)?;
            if exists {
                crate::say!(
                    "  {} {ROLE} already existed; its password was reset, because sloop could \
                     not read the old one back",
                    style::label("Note")
                );
            }
            (chosen, generated)
        }
    };

    if exists(
        server,
        superuser,
        &format!(
            "SELECT 1 FROM pg_database WHERE datname = '{}';",
            own.database
        ),
    )? {
        crate::say!("  {} {}", style::label("Already there"), own.database);
    } else {
        // **Not in a DO block, and not with the checks above folded into it.** `CREATE
        // DATABASE` cannot run inside a transaction, which is what a `DO` block is.
        make::run_sql(
            server,
            Some(superuser),
            &format!(
                "CREATE DATABASE \"{}\" OWNER \"{}\";",
                own.database, own.role
            ),
        )
        .map_err(|failure| {
            failure.hint(format!(
                "{} exists; {} could not be made",
                own.role, own.database
            ))
        })?;
        crate::say!("  {} {}", style::label("Created"), own.database);
    }

    // Proof rather than hope, and as the owning role rather than as the superuser: what every
    // later run does is exactly this connection, so it is made once here while somebody is
    // watching.
    if !opens(server, &own, &password)? {
        return Err(Failure::new(
            Exit::Connect,
            format!(
                "{} would not accept {}'s password",
                own.url(server),
                own.role
            ),
        )
        .hint("the role and the database exist; it is the password that did not open them"));
    }

    let route = super::record::remember_database(global, server, &own, &password)?;
    Ok(Opened {
        own,
        password,
        route,
        generated,
    })
}

/// Does this one-row query find anything?
fn exists(server: &Server, superuser: &Secret, query: &str) -> Outcome<bool> {
    let said = make::query(server, Some(superuser), query)?;
    Ok(said.trim() == "1")
}

/// Give the role its password, creating it first if it is not there.
///
/// **One statement either way, chosen by whether the role exists.** The two differ by a
/// word, and keeping them apart meant two places where a password is spelled into SQL —
/// which is exactly the spelling that has to be right every time. The value goes in as a
/// literal [`make::literal`] has escaped, and on standard input rather than in `argv`, so
/// `ps` never sees it.
///
/// `pub(super)` for one caller outside this module: `cluster_tests` drives it against a real
/// server with a password full of punctuation, which is the only way to prove that what
/// `make::literal` escapes is what PostgreSQL ends up holding.
pub(super) fn set_password(
    server: &Server,
    superuser: &Secret,
    own: &Own,
    password: &Secret,
    role_exists: bool,
) -> Outcome<()> {
    let verb = if role_exists { "ALTER" } else { "CREATE" };
    make::run_sql(
        server,
        Some(superuser),
        &format!(
            "{verb} ROLE \"{}\" WITH LOGIN PASSWORD {};",
            own.role,
            make::literal(password)?
        ),
    )?;

    if !role_exists {
        crate::say!("  {} {}", style::label("Created"), own.role);
    }

    Ok(())
}

/// Does this password open sloop's own database, as the role that owns it?
///
/// A wrong password is an answer and not an error — it is what says the role has to be reset.
/// A server that will not answer at all is a different thing and comes back as a failure.
pub fn opens(server: &Server, own: &Own, password: &Secret) -> Outcome<bool> {
    make::connects_as(server, &own.role, &own.database, password)
}
