//! `sloop_database`, owned by `sloop_db_admin` — the database sloop keeps its own state in.
//!
//! **`R19c1` found a server; this puts sloop's own database on it.** What goes *inside* —
//! the tables, the migrations — is `R19c3`. What is here is the database, the role that owns
//! it, the password that role gets, and the one property that makes a second run ordinary:
//! nothing is typed.
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

use std::path::Path;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;
use crate::style;

use super::{LOOPBACK, Server, make};

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

/// Make sure `sloop_database` and `sloop_db_admin` exist, and hand back the way in.
///
/// **Idempotent, and that is the `Done when`.** A second run finds both already there, reads
/// the password back from wherever this machine keeps secrets, proves the connection, and
/// returns — with nothing typed and nothing created twice.
pub fn ensure(global: &Path, server: &Server, superuser: &Secret) -> Outcome<(Own, Secret)> {
    let own = Own::sloops();
    let sealed = global.join(crate::registry::file::SEALED_FILE);

    // What a previous run left, if anything. A password sloop cannot read back is the same
    // as no password at all: the role is reset below rather than guessed at.
    let known = super::record::database_route(global)?
        .and_then(|route| {
            crate::secret::resolve(
                &route,
                &crate::secret::Lookup {
                    key: &own.credential_key(server),
                    sealed_file: &sealed,
                },
            )
            .ok()
        })
        .map(|resolved| resolved.secret);

    let role_exists = exists(
        server,
        superuser,
        &format!("SELECT 1 FROM pg_roles WHERE rolname = '{ROLE}';"),
    )?;

    // A password only where one is needed: an existing role whose password sloop can still
    // read is left exactly as it is, because resetting it would break anything else that had
    // been given it.
    let password = match (role_exists, known) {
        (true, Some(known)) if opens(server, &own, &known)? => known,
        (true, _) => {
            let fresh = Secret::new(crate::secret::generated_password()?);
            alter_role_password(server, superuser, &own, &fresh)?;
            crate::say!(
                "  {} {ROLE} already existed; its password was reset, because sloop could not \
                 read the old one back",
                style::label("Note")
            );
            fresh
        }
        (false, _) => {
            let fresh = Secret::new(crate::secret::generated_password()?);
            create_role(server, superuser, &own, &fresh)?;
            crate::say!("  {} {ROLE}", style::label("Created"));
            fresh
        }
    };

    if exists(
        server,
        superuser,
        &format!("SELECT 1 FROM pg_database WHERE datname = '{DATABASE}';"),
    )? {
        crate::say!("  {} {DATABASE}", style::label("Already there"));
    } else {
        // **Not in a DO block, and not with the checks above folded into it.** `CREATE
        // DATABASE` cannot run inside a transaction, which is what a `DO` block is.
        make::run_sql(
            server,
            Some(superuser),
            &format!("CREATE DATABASE \"{DATABASE}\" OWNER \"{ROLE}\";"),
        )
        .map_err(|failure| failure.hint(format!("{ROLE} exists; {DATABASE} could not be made")))?;
        crate::say!("  {} {DATABASE}", style::label("Created"));
    }

    // Proof rather than hope, and as the owning role rather than as the superuser: what every
    // later run does is exactly this connection, so it is made once here while somebody is
    // watching.
    if !opens(server, &own, &password)? {
        return Err(Failure::new(
            Exit::Connect,
            format!("{} would not accept {ROLE}'s password", own.url(server)),
        )
        .hint("the role and the database exist; it is the password that did not open them"));
    }

    super::record::remember_database(global, server, &own, &password)?;
    Ok((own, password))
}

/// Does this one-row query find anything?
fn exists(server: &Server, superuser: &Secret, query: &str) -> Outcome<bool> {
    let said = make::query(server, Some(superuser), query)?;
    Ok(said.trim() == "1")
}

/// Create the role, with its password on standard input and nowhere else.
fn create_role(server: &Server, superuser: &Secret, own: &Own, password: &Secret) -> Outcome<()> {
    make::run_sql(
        server,
        Some(superuser),
        &format!(
            "CREATE ROLE \"{}\" WITH LOGIN PASSWORD {};",
            own.role,
            make::literal(password)?
        ),
    )
}

/// Give an existing role a new password, the same way.
fn alter_role_password(
    server: &Server,
    superuser: &Secret,
    own: &Own,
    password: &Secret,
) -> Outcome<()> {
    make::run_sql(
        server,
        Some(superuser),
        &format!(
            "ALTER ROLE \"{}\" WITH LOGIN PASSWORD {};",
            own.role,
            make::literal(password)?
        ),
    )
}

/// Does this password open sloop's own database, as the role that owns it?
///
/// A wrong password is an answer and not an error — it is what says the role has to be reset.
/// A server that will not answer at all is a different thing and comes back as a failure.
pub fn opens(server: &Server, own: &Own, password: &Secret) -> Outcome<bool> {
    make::connects_as(server, &own.role, &own.database, password)
}
