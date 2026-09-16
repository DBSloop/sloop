//! The OS keyring: Credential Manager on Windows, Keychain on macOS, Secret Service on
//! Linux.
//!
//! This is the default route and the one that needs no configuration. It is also the one
//! that is not always there — a Linux box with no desktop session has no Secret Service,
//! and a cron job has no session keyring at all. So the failure here is not a dead end: it
//! names the other three routes, because on that machine one of them is the answer.

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;

/// What the keyring files these under. Every entry sloop owns is under this one service,
/// so a person can find and remove all of them without guessing.
const SERVICE: &str = "sloop";

/// Read the password stored for `key`.
pub fn get(key: &str) -> Outcome<Secret> {
    get_from(SERVICE, key)
}

/// Store a password for `key`.
pub fn set(key: &str, secret: &Secret) -> Outcome<()> {
    set_in(SERVICE, key, secret)
}

/// Forget the password stored for `key`.
///
/// A key that was never there is not a failure — `db edit` clears the old key after
/// moving a password, and a record whose password lived elsewhere has nothing to clear.
pub fn delete(key: &str) -> Outcome<()> {
    delete_from(SERVICE, key)
}

pub(crate) fn get_from(service: &str, key: &str) -> Outcome<Secret> {
    let entry = open(service, key)?;
    match entry.get_password() {
        Ok(password) => Ok(Secret::new(password)),
        Err(keyring::Error::NoEntry) => Err(Failure::new(
            Exit::Usage,
            format!("the keyring has no password for {key}"),
        )
        .hint("run `sloop db add` for it, or point the registry at another password route")),
        Err(error) => Err(unavailable(&error)),
    }
}

pub(crate) fn set_in(service: &str, key: &str, secret: &Secret) -> Outcome<()> {
    open(service, key)?
        .set_password(secret.expose())
        .map_err(|error| unavailable(&error))
}

pub(crate) fn delete_from(service: &str, key: &str) -> Outcome<()> {
    match open(service, key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(unavailable(&error)),
    }
}

fn open(service: &str, key: &str) -> Outcome<keyring::Entry> {
    keyring::Entry::new(service, key).map_err(|error| unavailable(&error))
}

/// Turn a keyring failure into a sentence that names a way forward.
///
/// The error itself is included because it is a platform message about the platform, not
/// about the password — there is no value in it to leak.
fn unavailable(error: &keyring::Error) -> Failure {
    Failure::new(
        Exit::Usage,
        format!("the OS keyring is not usable here: {error}"),
    )
    .hint(
        "on a machine with no keyring — a headless Linux box, or a scheduled job — use \
         encrypted-file, ${A_VARIABLE}, or command:<command line> instead",
    )
}
