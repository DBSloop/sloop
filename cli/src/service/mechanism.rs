//! The three service managers, behind one shape.
//!
//! **Each of them is driven by its own command-line tool rather than by its API**, which is
//! this project's habit everywhere it meets the operating system: the download is `curl`,
//! the registry is PowerShell, a firewalled database is `ssh`. `systemctl`, `launchctl` and
//! `sc.exe` are on every machine that has the thing they control, they are what a person
//! would type to check sloop's work afterwards, and they keep the dependency list short
//! enough to read.
//!
//! **The one exception is the Windows daemon itself**, which cannot be a shelled-out
//! anything: the Service Control Manager expects the running process to answer it, and a
//! process that does not is killed. That half lives in [`super::daemon`].
//!
//! **Every call here reports what the tool said when it fails.** A service manager refusing
//! is nearly always about permission or about a unit that is already there, and both are
//! things the person running the command can fix — but only if they are told which.

use std::path::PathBuf;
use std::process::{Command, Output};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use super::unit::{Definition, LAUNCHD_LABEL, SERVICE_NAME};

/// Which of the three is holding sloop on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    /// Linux.
    Systemd,
    /// macOS.
    Launchd,
    /// Windows.
    WindowsService,
}

impl Mechanism {
    /// The one this machine uses, or `None` on something none of the three run on.
    #[must_use]
    pub const fn of_this_machine() -> Option<Self> {
        if cfg!(target_os = "linux") {
            Some(Self::Systemd)
        } else if cfg!(target_os = "macos") {
            Some(Self::Launchd)
        } else if cfg!(windows) {
            Some(Self::WindowsService)
        } else {
            None
        }
    }

    /// What to call it when telling somebody what happened.
    #[must_use]
    pub const fn spoken(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::WindowsService => "the Service Control Manager",
        }
    }

    /// The file this mechanism keeps its definition in, if it keeps one.
    ///
    /// Windows keeps the service in the registry rather than in a file, which is why this is
    /// an `Option` rather than three paths.
    #[must_use]
    pub fn definition_file(self) -> Option<PathBuf> {
        match self {
            Self::Systemd => Some(PathBuf::from(format!(
                "/etc/systemd/system/{SERVICE_NAME}.service"
            ))),
            Self::Launchd => Some(PathBuf::from(format!(
                "/Library/LaunchDaemons/{LAUNCHD_LABEL}.plist"
            ))),
            Self::WindowsService => None,
        }
    }

    /// The text of that file, for this definition.
    #[must_use]
    pub fn definition_text(self, definition: &Definition) -> Option<String> {
        match self {
            Self::Systemd => Some(definition.systemd_unit()),
            Self::Launchd => Some(definition.launchd_plist()),
            Self::WindowsService => None,
        }
    }

    /// The command somebody can run themselves to see what this machine thinks.
    ///
    /// Printed after every `install`, because the first question anybody asks about a new
    /// service is "did that work" and the second is "how do I check without your tool".
    #[must_use]
    pub const fn their_own_check(self) -> &'static str {
        match self {
            Self::Systemd => "systemctl status sloop",
            Self::Launchd => "sudo launchctl print system/io.github.dbsloop.sloop",
            Self::WindowsService => "sc query sloop",
        }
    }
}

/// What the machine says about the service right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The machine has never been told about it.
    ///
    /// Distinct from stopped, and the distinction is the point: a monitoring system that
    /// cannot tell "somebody stopped it" from "it was never installed" alerts the wrong
    /// person, and `R24`'s own *Done when* asks for the two to be apart.
    NotInstalled,
    /// Registered to start at boot, and not running now.
    Stopped,
    /// Running.
    Running,
    /// Registered, and the service manager says something none of the above covers.
    Unclear(String),
}

impl State {
    /// The word for a report.
    #[must_use]
    pub fn spoken(&self) -> String {
        match self {
            Self::NotInstalled => String::from("not installed"),
            Self::Stopped => String::from("installed, stopped"),
            Self::Running => String::from("running"),
            Self::Unclear(what) => format!("installed, and {what}"),
        }
    }

    /// Is it registered with the machine at all?
    #[must_use]
    pub const fn installed(&self) -> bool {
        !matches!(self, Self::NotInstalled)
    }

    /// Is it running *now*?
    ///
    /// Separate from [`State::installed`] on purpose, and `R25` is what needed them apart: an
    /// attachment reaches a service that is running at its next round, and one that is merely
    /// installed when somebody starts it. Two different sentences to say to the person who
    /// just typed `attach`.
    #[must_use]
    pub const fn running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Run one of the service managers, and turn a refusal into something actionable.
///
/// **`sudo` is not run for the user.** A command that silently elevates is a command whose
/// effects nobody consented to; what happens instead is that the refusal is reported with the
/// line they should have typed. That is rule 4's shape applied to privilege rather than to a
/// question.
pub fn run(program: &str, arguments: &[&str], doing: &str) -> Outcome<Output> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| {
            Failure::new(Exit::Failure, format!("could not run {program}: {error}")).hint(
                if cfg!(target_os = "linux") {
                    "systemd is what sloop registers with on Linux, and this machine has no \
                     systemctl. A container or a machine using another init has to run sloop \
                     from its own scheduler instead."
                } else {
                    "the program that manages services on this machine could not be started"
                },
            )
        })?;

    if output.status.success() {
        return Ok(output);
    }

    let said = what_it_said(&output);
    let denied = said.to_lowercase().contains("access is denied")
        || said.to_lowercase().contains("permission denied")
        || said.to_lowercase().contains("must be root")
        || said.to_lowercase().contains("operation not permitted");

    let mut failure = Failure::new(Exit::Failure, format!("{doing} failed: {said}"));
    failure = failure.hint(if denied {
        if cfg!(windows) {
            "starting something at boot is a machine-wide change. Run this from a terminal \
             opened with 'Run as administrator'."
        } else {
            "starting something at boot is a machine-wide change. Run this again with sudo."
        }
    } else {
        "the line above is what this machine's service manager said. `sloop service status` \
         reports what sloop can see of it"
    });
    Err(failure)
}

/// Whatever the tool had to say, wherever it said it.
///
/// `systemctl` writes its refusals to standard error and `sc.exe` writes some of them to
/// standard output, so a reader that picks one stream reports an empty reason about half the
/// time.
fn what_it_said(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let said = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    if said.is_empty() {
        format!("exit status {}", output.status)
    } else {
        said.lines().take(3).collect::<Vec<_>>().join("; ")
    }
}
