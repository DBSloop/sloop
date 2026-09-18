//! `sloop service …` — the five things somebody does to a background service.
//!
//! **`status` asks the machine, never a row in a database.** sloop has a `service` table and
//! it would have been easy to write "installed" into it and read that back — but a row saying
//! installed while systemd has never heard of the unit is worse than having no row, and the
//! case where the two disagree is exactly the case somebody is running `status` to understand.
//! So the service manager is the source of truth for whether it exists and whether it is
//! running, every time, and the table is left to `R25` where it records what is *attached*,
//! which the machine genuinely does not know.

use crate::cli::ServiceCommand;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::locations::Locations;
use crate::service::mechanism::{Mechanism, State};
use crate::service::unit::Definition;
use crate::service::{daemon, key, manage};
use crate::style;

/// Run one of them.
pub fn run(locations: &Locations, command: &ServiceCommand) -> Outcome<Exit> {
    // The daemon first, and before anything that could print: on Windows this hands the
    // thread to the Service Control Manager, which owns the process's streams from then on.
    if let ServiceCommand::Run { store } = command {
        return daemon::run(store.clone());
    }

    let Some(mechanism) = Mechanism::of_this_machine() else {
        return Err(Failure::new(
            Exit::Failure,
            "sloop does not know how this machine starts things at boot",
        )
        .hint(
            "systemd, launchd and the Windows Service Control Manager are the three it \
             knows. Run sloop from this machine's own scheduler instead.",
        ));
    };

    match command {
        ServiceCommand::Install { no_start, user } => {
            install(locations, mechanism, *no_start, user.as_deref())
        }
        ServiceCommand::Uninstall => Ok(uninstall(mechanism)),
        ServiceCommand::Start => {
            already(mechanism, true)?;
            manage::start(mechanism)?;
            crate::say!("{} {}", style::heading("Started."), settle(mechanism));
            Ok(Exit::Success)
        }
        ServiceCommand::Stop => {
            already(mechanism, true)?;
            manage::stop(mechanism)?;
            crate::say!(
                "{} {}",
                style::heading("Stopped."),
                style::dim("it still starts again at the next boot")
            );
            Ok(Exit::Success)
        }
        ServiceCommand::Status => Ok(status(mechanism)),
        ServiceCommand::Run { .. } => unreachable!("handled above"),
    }
}

/// Register it, and start it unless told not to.
fn install(
    locations: &Locations,
    mechanism: Mechanism,
    no_start: bool,
    user: Option<&str>,
) -> Outcome<Exit> {
    let program = std::env::current_exe().map_err(|error| {
        Failure::new(
            Exit::Failure,
            format!("could not work out where sloop itself is: {error}"),
        )
    })?;

    let store = crate::registry::adopt::global(locations)?;

    // Windows services run as `LocalSystem` and are told where the store is instead; naming
    // an account there would be accepted and then ignored, which is worse than refusing it.
    if user.is_some() && mechanism == Mechanism::WindowsService {
        return Err(Failure::usage(
            "--user is not for Windows: a sloop service runs as LocalSystem",
        )
        .hint("it is told where the store is instead, which install does by itself"));
    }

    let account = match mechanism {
        Mechanism::WindowsService => None,
        _ => Some(user.map_or_else(whoami, str::to_owned)),
    };

    crate::say!(
        "{} {}",
        style::heading("Installing"),
        style::dim(&format!("with {}", mechanism.spoken()))
    );

    let key_file = key::write(account.as_deref())?;
    let definition = Definition {
        program,
        store,
        account,
        key_file,
    };

    manage::install(mechanism, &definition)?;
    if let Some(path) = mechanism.definition_file() {
        crate::say!("  {} {}", style::label("Wrote"), path.display());
    }

    if no_start {
        crate::say!("  {}", style::dim("left stopped, as asked"));
    } else if mechanism != Mechanism::Launchd {
        // launchd's `bootstrap` has already started it; asking again is an error there.
        manage::start(mechanism)?;
    }

    crate::say!("");
    crate::say!("{} {}", style::heading("Done."), settle(mechanism));
    crate::say!(
        "  {}",
        style::dim(&format!(
            "check it yourself with `{}`",
            mechanism.their_own_check()
        ))
    );
    Ok(Exit::Success)
}

/// Take it off, and say what would not go.
fn uninstall(mechanism: Mechanism) -> Exit {
    if !manage::state(mechanism).installed() {
        crate::say!(
            "{}",
            style::dim("there is no sloop service on this machine")
        );
        return Exit::Success;
    }

    crate::say!(
        "{} {}",
        style::heading("Removing"),
        style::dim(&format!("from {}", mechanism.spoken()))
    );

    let mut trouble = manage::uninstall(mechanism);
    trouble.extend(key::remove());

    for what in &trouble {
        crate::note!("{}", style::dim(&format!("{what} — remove it by hand")));
    }

    crate::say!("");
    crate::say!(
        "{} {}",
        style::heading("Done."),
        style::dim("nothing sloop recorded was deleted — attachments and history stay")
    );
    Exit::Success
}

/// Installed, running, and coming back at boot — each one separately, because they are
/// separate questions and a monitoring system needs them apart.
fn status(mechanism: Mechanism) -> Exit {
    let state = manage::state(mechanism);

    crate::say!("{}", style::heading("Service"));
    crate::say!("  {} {}", style::label("State"), state.spoken());
    crate::say!("  {} {}", style::label("Managed by"), mechanism.spoken());

    if state.installed() {
        crate::say!(
            "  {} {}",
            style::label("At boot"),
            if manage::starts_at_boot(mechanism) {
                "starts by itself"
            } else {
                "does not start by itself"
            }
        );
        if let Some(path) = mechanism.definition_file() {
            crate::say!("  {} {}", style::label("Defined in"), path.display());
        }
        crate::say!(
            "  {}",
            style::dim(&format!(
                "this machine's own answer: `{}`",
                mechanism.their_own_check()
            ))
        );
    } else {
        crate::say!(
            "  {}",
            style::dim("`sloop service install` registers it with this machine")
        );
    }

    // **Not an error, whatever the answer.** `status` is what a monitoring system runs, and a
    // command that exits non-zero because the thing it reports on is stopped cannot be used
    // to find out that it is stopped. That is why this returns a code rather than an
    // `Outcome`: there is no path through it that can fail.
    Exit::Success
}

/// Refuse the commands that only make sense against something already installed.
fn already(mechanism: Mechanism, needed: bool) -> Outcome<()> {
    if needed && !manage::state(mechanism).installed() {
        return Err(Failure::usage("there is no sloop service on this machine")
            .hint("`sloop service install` registers it first"));
    }
    Ok(())
}

/// What the machine says now, in one clause, for the end of a command that changed it.
fn settle(mechanism: Mechanism) -> String {
    match manage::state(mechanism) {
        State::Running => String::from("it is running."),
        State::Stopped => String::from("it is installed and stopped."),
        State::NotInstalled => String::from("this machine still does not have it."),
        State::Unclear(what) => format!("the machine says: {what}"),
    }
}

/// Whoever is running this, for the account the unit names.
///
/// `id -un` rather than `$USER`: a sudo'd shell keeps `$USER` as the original user on some
/// systems and rewrites it on others, and the unit has to name the account that will actually
/// own the store.
fn whoami() -> String {
    std::process::Command::new("id")
        .arg("-un")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| std::env::var("SUDO_USER").ok())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| String::from("root"))
}
