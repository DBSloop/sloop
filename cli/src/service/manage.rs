//! Registering, starting, stopping, asking and removing — once per service manager.
//!
//! **Each of the five is written three times and that is the honest shape**, because the
//! three managers do not agree on what any of it means. systemd separates "enabled at boot"
//! from "running now" and needs a daemon reload after a unit file changes; launchd conflates
//! loading with starting and calls the whole thing a bootstrap; Windows has a start type in
//! one place and a current state in another. A single abstraction over that would be a lie
//! with three special cases inside it.
//!
//! **What they do agree on is the five questions**, and those are the function names below.

use std::path::Path;

use super::mechanism::{Mechanism, State, run};
use super::unit::{Definition, LAUNCHD_LABEL, SERVICE_NAME};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// Write the definition down and register it to start at boot.
pub fn install(mechanism: Mechanism, definition: &Definition) -> Outcome<()> {
    if let (Some(path), Some(text)) = (
        mechanism.definition_file(),
        mechanism.definition_text(definition),
    ) {
        write_definition(&path, &text)?;
    }

    match mechanism {
        Mechanism::Systemd => {
            // Without the reload, systemd answers about the unit it read at boot and a fresh
            // install reports "unit not found" from a file that is plainly on disk.
            run("systemctl", &["daemon-reload"], "reloading systemd")?;
            run(
                "systemctl",
                &["enable", SERVICE_NAME],
                "enabling the service",
            )?;
        }
        Mechanism::Launchd => {
            // **Unloaded first, because `bootstrap` refuses a daemon launchd already has.**
            // Installing over an install is what an upgrade is, and launchd answers that with
            // "service already loaded" rather than replacing it. Best effort: on a machine
            // where nothing is loaded this fails, and that is the ordinary first install.
            let _ = run(
                "launchctl",
                &["bootout", &format!("system/{LAUNCHD_LABEL}")],
                "unloading the previous daemon",
            );

            // `bootstrap` both loads it and starts it, and `RunAtLoad` in the plist is what
            // brings it back at the next boot.
            run(
                "launchctl",
                &[
                    "bootstrap",
                    "system",
                    &mechanism
                        .definition_file()
                        .unwrap_or_default()
                        .display()
                        .to_string(),
                ],
                "loading the launchd daemon",
            )?;
        }
        Mechanism::WindowsService => {
            // **`config` when it is already there, `create` when it is not.** `sc.exe create`
            // fails with 1073 against a service that exists, and installing over an install is
            // what an upgrade is -- the binary has moved and the definition has to follow it.
            // Deleting and recreating would work too and would lose the start type, the
            // description and any recovery settings an administrator had set.
            let already = state(mechanism).installed();
            let verb = if already { "config" } else { "create" };

            // `sc.exe` wants `key= value` with the space *after* the equals sign, which is
            // not a typo and not optional: `start=auto` is rejected and `start= auto` is not.
            run(
                "sc.exe",
                &[
                    verb,
                    SERVICE_NAME,
                    "binPath=",
                    &definition.windows_binary_path(),
                    "start=",
                    "auto",
                    "obj=",
                    "LocalSystem",
                    "DisplayName=",
                    "sloop",
                ],
                if already {
                    "updating the service"
                } else {
                    "creating the service"
                },
            )?;
            // Not fatal: the service exists and works without a description, and a machine
            // that refuses this one refuses it for reasons that do not stop anything.
            let _ = run(
                "sc.exe",
                &[
                    "description",
                    SERVICE_NAME,
                    "Scheduled database backups and monitoring. Opens no port.",
                ],
                "describing the service",
            );
        }
    }
    Ok(())
}

/// Start it now.
///
/// **Already running is success, not an error.** Every one of the three managers refuses a
/// start against something already started -- Windows with 1056, systemd more quietly -- and
/// all three callers want the same thing from it: that the service be running when this
/// returns. `install` over an install hits this every time, and so does anybody who runs
/// `start` twice.
pub fn start(mechanism: Mechanism) -> Outcome<()> {
    if state(mechanism) == State::Running {
        return Ok(());
    }

    match mechanism {
        Mechanism::Systemd => run("systemctl", &["start", SERVICE_NAME], "starting it").map(drop),
        // launchd's `kickstart` starts a loaded daemon; `bootstrap` would fail on one that is
        // already there, which is the ordinary case for `start`.
        Mechanism::Launchd => run(
            "launchctl",
            &["kickstart", &format!("system/{LAUNCHD_LABEL}")],
            "starting it",
        )
        .map(drop),
        Mechanism::WindowsService => {
            run("sc.exe", &["start", SERVICE_NAME], "starting it").map(drop)
        }
    }
}

/// Stop it now, leaving it registered for the next boot.
///
/// Already stopped is success, for the reason [`start`] gives: what the caller wants is the
/// state afterwards, and `sc stop` against a stopped service is an error saying it is in the
/// state that was asked for.
pub fn stop(mechanism: Mechanism) -> Outcome<()> {
    if !matches!(state(mechanism), State::Running) {
        return Ok(());
    }

    match mechanism {
        Mechanism::Systemd => run("systemctl", &["stop", SERVICE_NAME], "stopping it").map(drop),
        Mechanism::Launchd => run(
            "launchctl",
            &["kill", "SIGTERM", &format!("system/{LAUNCHD_LABEL}")],
            "stopping it",
        )
        .map(drop),
        Mechanism::WindowsService => {
            run("sc.exe", &["stop", SERVICE_NAME], "stopping it").map(drop)
        }
    }
}

/// Take it off the machine.
///
/// **Every step is best effort and the last one still runs.** A half-installed service is
/// exactly the state somebody runs `uninstall` to escape, so a stop that fails because it was
/// already stopped must not prevent the unit file being removed.
pub fn uninstall(mechanism: Mechanism) -> Vec<String> {
    let mut trouble = Vec::new();

    let _ = stop(mechanism);

    match mechanism {
        Mechanism::Systemd => {
            let _ = run("systemctl", &["disable", SERVICE_NAME], "disabling it");
        }
        Mechanism::Launchd => {
            let _ = run(
                "launchctl",
                &["bootout", &format!("system/{LAUNCHD_LABEL}")],
                "unloading it",
            );
        }
        Mechanism::WindowsService => {
            if let Err(failure) = run("sc.exe", &["delete", SERVICE_NAME], "deleting the service") {
                trouble.push(failure.message().to_owned());
            }
        }
    }

    if let Some(path) = mechanism.definition_file()
        && path.exists()
        && let Err(error) = std::fs::remove_file(&path)
    {
        trouble.push(format!("{} is still there: {error}", path.display()));
    }

    if mechanism == Mechanism::Systemd {
        // So systemd stops answering about a unit whose file has gone.
        let _ = run("systemctl", &["daemon-reload"], "reloading systemd");
    }

    trouble
}

/// What the machine says right now.
///
/// **Never an error.** "I could not find out" is itself an answer a status command has to be
/// able to give, and a monitoring system reading an exit code needs the three states apart
/// rather than a failure when the service happens to be absent.
#[must_use]
pub fn state(mechanism: Mechanism) -> State {
    match mechanism {
        Mechanism::Systemd => {
            // `is-active` exits non-zero for everything that is not running, so the exit
            // code is no use and the word it prints is: `active`, `inactive`, `failed`, or
            // `unknown` for a unit systemd has never heard of.
            let Some(said) = spoken("systemctl", &["is-active", SERVICE_NAME]) else {
                return State::NotInstalled;
            };
            match said.trim() {
                "active" => State::Running,
                // `unknown` is what systemd says about a unit it has never been told about,
                // and an empty answer means `systemctl` printed nothing at all.
                "unknown" | "" => State::NotInstalled,
                "inactive" | "failed" if !unit_file_exists(mechanism) => State::NotInstalled,
                "inactive" => State::Stopped,
                other => State::Unclear(other.to_owned()),
            }
        }
        Mechanism::Launchd => {
            if !unit_file_exists(mechanism) {
                return State::NotInstalled;
            }
            // `print` on a loaded daemon includes a `state = running` line; on one that is
            // loaded but not running it says something else, and on one launchd has never
            // heard of it fails outright.
            let Some(said) = spoken("launchctl", &["print", &format!("system/{LAUNCHD_LABEL}")])
            else {
                return State::Stopped;
            };
            if said.contains("state = running") {
                State::Running
            } else {
                State::Stopped
            }
        }
        Mechanism::WindowsService => {
            let Some(said) = spoken("sc.exe", &["query", SERVICE_NAME]) else {
                return State::NotInstalled;
            };
            if said.contains("1060") || said.to_lowercase().contains("does not exist") {
                return State::NotInstalled;
            }
            if said.contains("RUNNING") {
                State::Running
            } else if said.contains("STOPPED") {
                State::Stopped
            } else if let Some(line) = said.lines().find(|line| line.contains("STATE")) {
                State::Unclear(line.trim().to_owned())
            } else {
                State::Stopped
            }
        }
    }
}

/// Is the service registered to come back at the next boot?
///
/// **This is what stands in for rebooting the machine.** A test cannot restart a CI runner,
/// so what it can check is the thing a restart would depend on: systemd's unit is enabled,
/// launchd's plist says `RunAtLoad`, and Windows' start type is `AUTO_START`.
#[must_use]
pub fn starts_at_boot(mechanism: Mechanism) -> bool {
    match mechanism {
        Mechanism::Systemd => spoken("systemctl", &["is-enabled", SERVICE_NAME])
            .is_some_and(|said| said.trim() == "enabled"),
        Mechanism::Launchd => mechanism.definition_file().is_some_and(|path| {
            std::fs::read_to_string(path).is_ok_and(|plist| {
                plist.contains("<key>RunAtLoad</key>") && plist.contains("<true/>")
            })
        }),
        Mechanism::WindowsService => {
            spoken("sc.exe", &["qc", SERVICE_NAME]).is_some_and(|said| said.contains("AUTO_START"))
        }
    }
}

/// Whatever a query tool printed, on either stream, or `None` when it could not be run.
fn spoken(program: &str, arguments: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .ok()?;
    let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
    said.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(said)
}

fn unit_file_exists(mechanism: Mechanism) -> bool {
    mechanism
        .definition_file()
        .is_some_and(|path| path.exists())
}

/// Write a unit file, making its directory if the machine has not got one.
///
/// **World-readable on purpose**, because that is what every one of these files is: the
/// secret is in the key file the definition points at, never in the definition.
fn write_definition(path: &Path, text: &str) -> Outcome<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            Failure::new(
                Exit::Failure,
                format!("could not create {}: {error}", parent.display()),
            )
        })?;
    }

    std::fs::write(path, text).map_err(|error| {
        let failure = Failure::new(
            Exit::Failure,
            format!("could not write {}: {error}", path.display()),
        );
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            failure.hint(
                "that directory belongs to the machine rather than to you. Run this again \
                 with sudo.",
            )
        } else {
            failure
        }
    })
}
