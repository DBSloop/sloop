//! The copy of sloop's own passwords that a service can actually read.
//!
//! **The problem, in one sentence.** A Windows service runs as `LocalSystem`, Credential
//! Manager is per user, and so a daemon cannot read one thing the interactive session put in
//! the keyring — it installs cleanly and then fails at every connection, silently, for ever.
//!
//! **`R24` built half the answer and nobody noticed.** The unit carries the path to a key
//! file, the key file carries a passphrase at `0600`, and the encrypted store is the route
//! `R3` already wrote for headless Linux. What was missing is that the passphrase in that file
//! was freshly generated and **nothing had ever been sealed with it** — so the daemon would
//! read a passphrase that opened nothing, against a store whose route said `keyring` anyway.
//! `R25` was the first entry in which the daemon opened the store at all, which is why it is
//! the first that could see it.
//!
//! **What this does.** At install time, as the user running the command — whose keyring *is*
//! readable, because they are logged in — it resolves both of sloop's own passwords and seals
//! a copy of each into `<store>/secrets.sealed` under the key file's passphrase.
//!
//! ```text
//! superuser   sloop-server:<role>@127.0.0.1:<port>
//! database    sloop-database:<role>@127.0.0.1:<port>/<database>
//! ```
//!
//! **The record is not touched, and that is the point.** `server.toml` goes on saying
//! `keyring`, so the person who set this machine up carries on using it without knowing any of
//! this happened. The copy is reached by the fallback in [`crate::secret::resolve`], which is
//! open only to a process that was given a passphrase *file* — which nothing but a unit file
//! does.
//!
//! **Two passwords, and both are needed.** Opening sloop's own database takes the owning
//! role's; `record::reopen` starts the cluster first and that takes the superuser's. A copy of
//! one without the other is a service that gets further and still fails.

use std::path::Path;

use crate::failure::{Failure, Outcome};
use crate::secret::sealed;
use crate::server::record;
use crate::style;

/// What [`copy_for_the_service`] did, for the line `install` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Copied {
    /// Both passwords are now in the encrypted store.
    Both,
    /// There is no record, so there is nothing to copy. Every machine before `sloop setup`,
    /// and every CI runner that installs the service to prove the unit is accepted.
    NothingSetUp,
}

/// Put a copy of sloop's own passwords where the daemon can read them.
///
/// **Run as the installing user, on purpose.** This is the one moment at which both things are
/// true at once: the keyring is open, because a person is logged in and typed the command; and
/// the key file exists, because `install` has just written it. Neither is true again.
pub fn copy_for_the_service(store: &Path, key_file: &Path) -> Outcome<Copied> {
    let Some(ready) = record::reopen(store)? else {
        return Ok(Copied::NothingSetUp);
    };
    let Some((own, database_password)) = record::database(store)? else {
        return Ok(Copied::NothingSetUp);
    };

    let passphrase = read_passphrase(key_file)?;
    // **The daemon's file, not the person's.** See `registry::file::SERVICE_SEALED_FILE`:
    // these are sealed under the key file's passphrase, and the store beside them is sealed
    // under the one somebody typed.
    let path = store.join(crate::registry::file::SERVICE_SEALED_FILE);
    let vault = sealed::Vault::File(&path);

    // The superuser first: it is what starts the cluster, so a daemon that has only the other
    // one gets further and still fails.
    sealed::put_under(
        &vault,
        &record::credential_key_of(&ready.server),
        &ready.password,
        &passphrase,
    )?;
    sealed::put_under(
        &vault,
        &own.credential_key(&ready.server),
        &database_password,
        &passphrase,
    )?;

    Ok(Copied::Both)
}

/// Take the copies away again.
///
/// **`uninstall` removes the key file, so the copies become unreadable anyway** — but
/// unreadable is not gone, and a ciphertext left behind after an uninstall is the kind of
/// thing that turns up in a security review two years later. Best effort and named in the
/// report, the same way the key file itself is.
pub fn remove_the_copies(store: &Path, key_file: &Path) -> Option<String> {
    let path = store.join(crate::registry::file::SERVICE_SEALED_FILE);
    if !path.is_file() || !key_file.is_file() {
        return None;
    }

    let Ok(passphrase) = read_passphrase(key_file) else {
        return None;
    };
    let vault = sealed::Vault::File(&path);

    // **Only the two this module put there, even though the file is its own.** `R34` gave the
    // daemon a file of its own, so deleting it outright would be correct today — and would
    // stop being correct the moment anything else is ever copied into it. Taking out what was
    // put in cannot become wrong later.
    let Ok(Some(ready)) = record::reopen(store) else {
        return None;
    };
    let mut trouble = Vec::new();

    for key in [
        record::credential_key_of(&ready.server),
        match record::recorded_own(store) {
            Ok(Some(own)) => own.credential_key(&ready.server),
            _ => String::new(),
        },
    ] {
        if key.is_empty() {
            continue;
        }
        if let Err(why) = sealed::forget_under(&vault, &key, &passphrase) {
            trouble.push(why.message().to_owned());
        }
    }

    (!trouble.is_empty()).then(|| {
        format!(
            "the service's copy of a password is still in {}: {}",
            path.display(),
            trouble.join("; ")
        )
    })
}

/// The passphrase out of the key file, with its line ending off.
fn read_passphrase(key_file: &Path) -> Outcome<String> {
    let held = std::fs::read_to_string(key_file).map_err(|error| {
        Failure::usage(format!("could not read {}: {error}", key_file.display()))
            .hint("that file is what the service opens its copy of the password with")
    })?;

    Ok(held.trim_end_matches(['\r', '\n']).to_owned())
}

/// Say what happened, in the one line `install` wants.
pub fn announce(copied: Copied, store: &Path) {
    match copied {
        Copied::Both => crate::say!(
            "  {} {}",
            style::label("Keys"),
            style::dim(&format!(
                "copied into {} — a service cannot read your keyring",
                store.join(crate::registry::file::SEALED_FILE).display()
            ))
        ),
        Copied::NothingSetUp => crate::note!(
            "{}",
            style::dim(
                "this machine has not been set up, so the service has nothing to open yet \u{2014} \
                 run `sloop setup`, then `sloop service install` again"
            )
        ),
    }
}
