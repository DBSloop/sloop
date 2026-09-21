//! The file the daemon reads its passphrase from.
//!
//! **Why a file at all, when everything else uses the keyring.** A Windows service runs as
//! `LocalSystem` and Credential Manager is per user, so a daemon installed that way cannot see
//! one thing the interactive session stored — it would install cleanly and fail at the first
//! connection. That is settled in `docs/OWNER-DECISIONS.md` and the answer is the route `R3`
//! already built for headless Linux: the Argon2id encrypted store, unlocked by a passphrase.
//! The passphrase has to reach a process nobody is logged in beside, so it lives in a file,
//! and the whole of the protection is that file's permissions.
//!
//! **It is not in the unit file, and that is the one thing here that cannot be got wrong.** A
//! systemd unit is world-readable by design — `systemctl cat sloop` prints it to anybody with
//! a shell — and a launchd plist in `/Library/LaunchDaemons` is too. So the unit carries the
//! *path*; this file carries the secret; and rule 3 survives the one place where breaking it
//! would leave no trace.

use std::path::PathBuf;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::style;

/// Where the passphrase lives, per platform.
///
/// Beside the machine's other machine-wide state rather than in a home directory, because the
/// account that reads it is not the account that installed it.
#[must_use]
pub fn path() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("ProgramData")
            .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
        root.join("sloop").join("service.key")
    } else {
        PathBuf::from("/etc/sloop/service.key")
    }
}

/// Write a new passphrase, readable by the service account and by nobody else.
///
/// A key file, and whether this run is the one that made it.
///
/// **`made` is what lets a failed install undo itself.** `R34`: `install` writes the key
/// before the step that copies the passwords, and that step is the one that used to fail on a
/// server. What it left behind was a key file nothing owned — no unit was registered, so
/// `service uninstall` said there was no service and `reset` never looked at it — and the
/// next attempt reused its passphrase and failed the same way. A key this run did not make is
/// left exactly as it was: it may be opening a store for a daemon that is running right now.
#[derive(Debug, Clone)]
pub struct Written {
    /// Where it is.
    pub path: PathBuf,
    /// True when this run created it, and so may remove it again.
    pub made: bool,
}

/// Remove a key file this run made, after something later went wrong.
pub fn unmake(written: &Written) {
    if !written.made {
        return;
    }
    let _ = std::fs::remove_file(&written.path);
}

/// **Already there is left alone.** Rewriting it would lock the daemon out of the store it was
/// already opening, so re-running `install` keeps the passphrase that is working.
pub fn write(account: Option<&str>) -> Outcome<Written> {
    let path = path();
    if path.exists() {
        crate::say!(
            "  {} {}",
            style::label("Key"),
            style::dim(&format!("{} is already there, kept", path.display()))
        );
        return Ok(Written { path, made: false });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            let failure = Failure::new(
                Exit::Failure,
                format!("could not create {}: {error}", parent.display()),
            );
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                failure.hint("that directory belongs to the machine. Run this again with sudo.")
            } else {
                failure
            }
        })?;
    }

    let passphrase = crate::secret::generated_password()?;
    std::fs::write(&path, &passphrase).map_err(|error| {
        Failure::new(
            Exit::Failure,
            format!("could not write {}: {error}", path.display()),
        )
    })?;

    restrict(&path, account)?;
    crate::say!("  {} {}", style::label("Key"), path.display());
    Ok(Written { path, made: true })
}

/// Take the file away again.
///
/// Named in the report rather than done silently: a passphrase file left behind after an
/// uninstall is the kind of thing that turns up in a security review two years later.
pub fn remove() -> Option<String> {
    let path = path();
    if !path.exists() {
        return None;
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {
            crate::say!("  {} {}", style::label("Removed"), path.display());
            None
        }
        Err(error) => Some(format!("{} is still there: {error}", path.display())),
    }
}

/// Owner-only, on whichever platform this is.
#[cfg(unix)]
fn restrict(path: &std::path::Path, account: Option<&str>) -> Outcome<()> {
    use std::os::unix::fs::PermissionsExt;

    // `0o600` before the owner is changed, not after: between `write` and `chown` the file
    // exists, and a window in which it is readable is a window.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        Failure::new(
            Exit::Failure,
            format!("could not set permissions on {}: {error}", path.display()),
        )
    })?;

    if let Some(account) = account {
        // `chown` rather than a crate: this is one call, on one file, and `chown` is on every
        // machine that has the accounts it is being pointed at.
        let done = std::process::Command::new("chown")
            .arg(account)
            .arg(path)
            .output();
        let handed_over = done.is_ok_and(|output| output.status.success());
        if !handed_over {
            return Err(Failure::new(
                Exit::Failure,
                format!("could not give {} to {account}", path.display()),
            )
            .hint(
                "the service reads its passphrase from that file and runs as that account. \
                 Run this again with sudo, or use --user to name an account that exists.",
            ));
        }
    }
    Ok(())
}

/// Windows has no mode bits; it has an ACL, and `%ProgramData%` hands out a readable one.
///
/// **Inheritance is switched off first.** Everything under `%ProgramData%` is readable by
/// `Users` through an inherited entry, so granting `SYSTEM` alone would change nothing — the
/// inherited grant is still there. `icacls /inheritance:r` drops them, and the two grants that
/// follow are the account the service runs as and the administrators who installed it.
#[cfg(windows)]
fn restrict(path: &std::path::Path, account: Option<&str>) -> Outcome<()> {
    let _ = account;
    let target = path.display().to_string();

    let done = std::process::Command::new("icacls.exe")
        .args([
            &target,
            "/inheritance:r",
            "/grant:r",
            "SYSTEM:(F)",
            "/grant:r",
            "*S-1-5-32-544:(F)",
        ])
        .output();

    let restricted = done.is_ok_and(|output| output.status.success());
    if !restricted {
        return Err(Failure::new(
            Exit::Failure,
            format!("could not restrict who can read {target}"),
        )
        .hint(
            "that file holds the passphrase the service opens its store with, so sloop will \
             not leave it readable. Run this from a terminal opened with 'Run as \
             administrator'.",
        ));
    }
    Ok(())
}
