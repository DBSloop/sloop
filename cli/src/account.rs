//! The account sloop's own PostgreSQL runs as, and the group that shares its store.
//!
//! **PostgreSQL will not run as root, and never has.** `initdb`, `pg_ctl` and the postmaster
//! each exit on `geteuid() == 0` with no flag to say otherwise — a server that loads C
//! extensions and runs `COPY … FROM PROGRAM` as uid 0 is one statement away from owning the
//! machine, so the refusal is the point of it. Root is also the login most server images hand
//! out, which is why "run it as somebody else" is not an answer to somebody who has just
//! pasted the install line from the documentation.
//!
//! So a root run does what `pg_createcluster` and the official postgres image already do:
//! prepare everything as root, then hand the three programs that refuse an unprivileged
//! account to run under. Nothing else about the run changes — sloop itself stays root, and a
//! run that is not root does none of this and drops nothing.
//!
//! **Every question here is asked by running a program**, the way `service::elevation`
//! already asks `id -u`. This crate forbids `unsafe`, so `geteuid` and `getpwnam` are not
//! reachable from inside it, and one spawn behind a `OnceLock` is what is left.

#[cfg(test)]
#[path = "account_tests.rs"]
mod tests;

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::failure::{Failure, Outcome};

/// The account and group a root run creates when the machine has neither.
#[cfg(unix)]
pub const NAME: &str = "sloop";

/// The account an existing PostgreSQL install already has, used when [`NAME`] cannot be
/// created — which on macOS is every time, because it has no `useradd`.
#[cfg(unix)]
const FALLBACK: &str = "postgres";

/// The store directory, and everything a group member creates inside it.
///
/// **Setgid, and that bit is the whole of the sharing.** A backup written by one account has
/// to be readable by the next, and `2775` is what makes every directory created below this
/// one carry the same group without anybody setting a umask.
pub const SHARED_DIR: u32 = 0o2775;

/// A file sloop keeps in a shared store: `server.toml`, `secrets.sealed`.
///
/// Group-writable, because `setup` and `reset` are not always run by the account that ran
/// them first. What guards the sealed file is the passphrase, never its mode.
#[cfg_attr(not(unix), allow(dead_code))]
pub const SHARED_FILE: u32 = 0o0660;

/// Is this run root? `false` on Windows, which has no such refusal to work around.
///
/// Asked once. A menu that asked on every keypress would spawn a process per keypress.
#[must_use]
pub fn is_root() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(|| {
        if cfg!(windows) {
            false
        } else {
            said(Command::new("id").arg("-u")).is_some_and(|said| said.trim() == "0")
        }
    })
}

/// The account sloop's cluster runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// What it is called, for the sentence that tells somebody how to join its group.
    pub name: String,
    /// The account.
    pub uid: u32,
    /// Its primary group, which is also the group that shares the store.
    pub gid: u32,
}

/// The account to run the cluster as, making it when this machine has not got one.
///
/// **An account that is already here is used rather than replaced.** A second `sloop` beside
/// the first is how a machine ends up with a data directory nothing can open — and it is also
/// what makes this safe to call on every root run rather than only the first.
///
/// The order is deliberate: `sloop`, then one this machine can be given, then `postgres`.
/// macOS has no `useradd` — accounts there are rows in Directory Services — so the fallback
/// is what makes a Mac with PostgreSQL on it work without this module learning `dscl`.
#[cfg(unix)]
pub fn service() -> Outcome<Service> {
    // **Asked once, and by one caller at a time.** Two `useradd` runs racing each other end
    // with the loser failing on an account the winner has just made — which then reads as a
    // machine that cannot make one at all. It is not hypothetical: the cluster tests build
    // several clusters at once and it is what they did.
    static ANSWER: OnceLock<Service> = OnceLock::new();
    static MAKING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    if let Some(known) = ANSWER.get() {
        return Ok(known.clone());
    }

    // A poisoned lock is a thread that panicked while holding it; what it guards is `useradd`
    // and `useradd` is idempotent below, so carrying on is right and `unwrap` would not be.
    let _making = MAKING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(known) = ANSWER.get() {
        return Ok(known.clone());
    }

    let found = resolve()?;
    let _ = ANSWER.set(found.clone());
    Ok(found)
}

/// Find the account, or make it, without the caching [`service`] wraps this in.
#[cfg(unix)]
fn resolve() -> Outcome<Service> {
    if let Some(found) = look_up(NAME) {
        return Ok(found);
    }

    if cfg!(target_os = "linux") {
        // **What it did, not what it said.** A `useradd` that failed because another run of
        // sloop made the account a moment ago has done exactly what was wanted, so the
        // account is looked for before its complaint is believed — and its complaint is only
        // raised when there is still no account to show for it.
        let attempted = make_the_account();
        if let Some(made) = look_up(NAME) {
            return Ok(made);
        }
        attempted?;
    }

    if let Some(fallback) = look_up(FALLBACK) {
        return Ok(fallback);
    }

    Err(Failure::usage(format!(
        "PostgreSQL will not run a cluster as root, and this machine has neither a {NAME} \
         account nor a {FALLBACK} one to run it as"
    ))
    .hint(format!(
        "create one and run this again: `useradd --system --user-group --shell \
         /usr/sbin/nologin {NAME}` on Linux, `sysadminctl -addUser {NAME}` on macOS"
    )))
}

/// Never called: Windows runs the cluster as whoever is logged in, as it always has.
#[cfg(windows)]
pub fn service() -> Outcome<Service> {
    Err(
        Failure::usage("this build asked for a service account, and Windows does not use one")
            .report_a_bug(),
    )
}

/// `id` on one account, or `None` when there is no such account.
#[cfg(unix)]
fn look_up(name: &str) -> Option<Service> {
    let uid = number(Command::new("id").arg("-u").arg(name))?;
    let gid = number(Command::new("id").arg("-g").arg(name))?;
    Some(Service {
        name: name.to_owned(),
        uid,
        gid,
    })
}

/// Create the account and the group of the same name.
///
/// **A system account**: no password, no login shell, no home of its own — it exists to own a
/// data directory and to be the uid a postmaster runs under. `--user-group` is what makes the
/// group that shares the store, and `-f` on `groupadd` is what makes this idempotent on a
/// machine where the group is there and the account is not.
#[cfg(unix)]
fn make_the_account() -> Outcome<()> {
    ran(Command::new("groupadd").args(["-f", NAME]))?;
    ran(Command::new("useradd").args([
        "--system",
        "--gid",
        NAME,
        "--no-create-home",
        "--shell",
        "/usr/sbin/nologin",
        NAME,
    ]))
}

/// Hand a child the service account to run as.
///
/// The gid goes on before the uid — the standard library sets them in that order, which is
/// the only order that works, since dropping the uid first would take away the right to set
/// the gid.
#[cfg(unix)]
pub fn run_as(command: &mut Command, service: &Service) {
    use std::os::unix::process::CommandExt as _;
    command.uid(service.uid).gid(service.gid);
}

/// Windows has no uid to drop to, so this is what "run it as you are" looks like.
#[cfg(windows)]
#[allow(clippy::missing_const_for_fn)]
pub fn run_as(_command: &mut Command, _service: &Service) {}

/// Give one path to the service account.
#[cfg(unix)]
pub fn give(path: &Path, service: &Service) -> Outcome<()> {
    std::os::unix::fs::chown(path, Some(service.uid), Some(service.gid)).map_err(|error| {
        Failure::usage(format!(
            "could not give {} to {}: {error}",
            path.display(),
            service.name
        ))
    })
}

/// Windows: nothing to give and nobody to give it to.
///
/// It keeps the shape its caller expects rather than the shape it needs on its own — a
/// `chown` that can fail on one platform and cannot exist on the other is still one call
/// site, and two signatures for it would mean two call sites.
#[cfg(windows)]
#[allow(clippy::unnecessary_wraps)]
pub fn give(_path: &Path, _service: &Service) -> Outcome<()> {
    Ok(())
}

/// Put one path in the store's group, at the mode a shared store uses.
///
/// **A failure here is reported and not fatal, which is rule 0d.** A store on a filesystem
/// that will not carry a group — a bind mount, a container layer, an exported share — still
/// works for the account that made it, and refusing to set up a machine because a `chmod`
/// came back with `EPERM` would take a working tool away from somebody to enforce a
/// convenience for somebody else. The line says what did not happen.
pub fn share(path: &Path, gid: u32, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        if let Err(error) = std::os::unix::fs::chown(path, None, Some(gid)) {
            crate::report::notice(&crate::style::dim(&format!(
                "Note: {} kept its group — {error}",
                path.display()
            )));
            return;
        }
        if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            crate::report::notice(&crate::style::dim(&format!(
                "Note: {} kept its permissions — {error}",
                path.display()
            )));
        }
    }

    // Windows shares a store by its own inheritance rules, which this entry does not change.
    #[cfg(not(unix))]
    {
        let _ = (path, gid, mode);
    }
}

/// Make the store directory, and share it when it is the one a whole machine uses.
///
/// **Called wherever the store is first written to**, so that a machine-wide store is group
/// readable from the moment it exists rather than from the moment somebody remembers to fix
/// it. A per-user store is created exactly as it always was and nothing is shared.
pub fn prepare_store(dir: &Path) -> Outcome<()> {
    std::fs::create_dir_all(dir)
        .map_err(|error| Failure::usage(format!("could not create {}: {error}", dir.display())))?;

    if !is_root() || !crate::registry::locations::is_machine_store(dir) {
        return Ok(());
    }

    let service = service()?;
    share(dir, service.gid, SHARED_DIR);
    Ok(())
}

/// Let the store's group read and write one file sloop keeps in it.
///
/// **The setgid bit on the directory is what says this is a shared store**, so nothing has to
/// be threaded down here to tell this function which kind it is writing into. A per-user
/// store has no setgid bit and no group to share with, and the file keeps the mode it was
/// written with.
pub fn share_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let Some(parent) = path.parent() else {
            return;
        };
        let Ok(parent) = std::fs::metadata(parent) else {
            return;
        };
        if parent.permissions().mode() & 0o2000 == 0 {
            return;
        }

        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(SHARED_FILE));
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// The line that lets one more account use a shared store.
///
/// Said by `setup` once, because a group somebody was never told about is a group nobody
/// joins — and the second half matters as much as the first: a shell that was already open
/// when the account was added is a shell that does not have the group yet.
#[must_use]
pub fn how_to_join(service: &Service) -> String {
    format!(
        "Another account uses this store once it is in the {} group: `sudo usermod -aG {} <name>`, then a new login.",
        service.name, service.name
    )
}

/// The group a path belongs to, by name.
///
/// **Asked of the filesystem rather than assumed**, because the group is whichever one the
/// store was made with — `sloop` on a machine that had none, `postgres` on a Mac that could
/// not make one. Naming the wrong group in a message that tells somebody how to get access
/// is worse than naming none.
///
/// GNU `stat` and BSD `stat` spell the same question differently and neither understands the
/// other, so both are asked. Only ever on a path somebody has just been refused.
#[must_use]
pub fn group_of(path: &Path) -> Option<String> {
    if cfg!(windows) {
        return None;
    }

    said(Command::new("stat").arg("-c").arg("%G").arg(path))
        .or_else(|| said(Command::new("stat").arg("-f").arg("%Sg").arg(path)))
        .map(|said| said.trim().to_owned())
        .filter(|said| !said.is_empty())
}

/// Run a program and hand back what it said, or `None` when it would not run at all.
fn said(command: &mut Command) -> Option<String> {
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The one number a program printed.
#[cfg(unix)]
fn number(command: &mut Command) -> Option<u32> {
    parse_number(&said(command)?)
}

/// The number in what `id` printed.
///
/// Separated from the spawn so it can be checked from a host that has no `id` — and because
/// an account that does not exist is `id` printing nothing on stdout and failing, which is
/// already `None` before this sees it.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_number(said: &str) -> Option<u32> {
    said.trim().parse().ok()
}

/// Run a program that is expected to succeed, and say what it was if it did not.
#[cfg(unix)]
fn ran(command: &mut Command) -> Outcome<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| Failure::usage(format!("could not run {program}: {error}")))?;

    if output.status.success() {
        return Ok(());
    }

    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.trim();
    Err(Failure::usage(format!(
        "{program} would not make the {NAME} account{}",
        if said.is_empty() {
            String::new()
        } else {
            format!(": {said}")
        }
    ))
    .hint(format!(
        "make it by hand and run this again: `useradd --system --user-group --shell \
         /usr/sbin/nologin {NAME}`"
    )))
}
