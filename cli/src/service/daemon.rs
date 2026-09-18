//! The far side of `install` — the process the service manager actually runs.
//!
//! **What it does is deliberately almost nothing, and that is `R24`'s boundary.** Sampling is
//! `R26`, the schedule is `R27a`, and neither can be written until something is running to
//! write them into. What is here is the part all of them need and none of them can add later
//! without rewriting the other two: a process the three service managers each recognise as
//! healthy, that stops when told, and that comes back at boot.
//!
//! **`R25` filled the round in, and it is one thing: read the attachment list.** Every turn of
//! the loop reads it fresh out of sloop's own PostgreSQL and marks what it read, which is what
//! makes `sloop service attach` reach a service that is already running. `R26` is what turns
//! that list into samples.
//!
//! **Windows is the reason this is a module rather than a loop.** systemd and launchd start a
//! program and watch the process; if it is alive, it is running, and `SIGTERM` ends it the way
//! it ends anything. The Service Control Manager does not work like that: it starts the
//! process and then *waits to be called back*, and a program that has not reported
//! `SERVICE_RUNNING` within about thirty seconds is killed as hung. So on Windows the daemon
//! hands its main thread to the SCM's dispatcher, reports running, and waits on a channel that
//! the control handler signals — while on Unix the same work is a loop.
//!
//! **Nothing here opens a port.** The heartbeat goes to standard error, which systemd routes
//! to the journal and launchd to its log, and the Windows service writes to the event log by
//! way of the SCM. A person asking what it is doing asks the CLI, on the same machine.

use std::path::PathBuf;
use std::time::Duration;

use crate::exit::Exit;
use crate::failure::Outcome;

/// How often the daemon wakes to do its round, when nothing said otherwise.
///
/// **`R26`'s default, and it is a default rather than a constant now**: `service install
/// --interval` writes the real one into the definition, because a service manager hands the
/// daemon nothing but a command line.
pub const INTERVAL_SECONDS: u64 = 60;

/// The shortest round this will accept.
///
/// **Rule 0d, from the other side.** A round opens a client process per attached database, so
/// one second across ten databases is ten processes a second for ever. Clamped rather than
/// refused: somebody who asked for one second wants readings as often as possible, and telling
/// them no would be the feature refusing.
const FLOOR_SECONDS: u64 = 5;

/// What `--interval` means, once it has been read.
#[must_use]
pub fn interval(seconds: u64) -> Duration {
    Duration::from_secs(seconds.max(FLOOR_SECONDS))
}

/// Run as a service, in whatever way this platform means by that.
///
/// **The `Outcome` is for Windows, and the signature cannot vary.** Handing the thread to the
/// Service Control Manager fails on a machine where this was run from a terminal rather than
/// started as a service, and that has to be reportable. The Unix branch has no such step and
/// so can only succeed -- which clippy correctly notices when it compiles for Linux, and which
/// is not a reason for the caller to have to know which platform it is on.
#[cfg_attr(not(windows), allow(clippy::unnecessary_wraps))]
pub fn run(store: Option<PathBuf>, seconds: u64) -> Outcome<Exit> {
    let store = store.unwrap_or_else(|| PathBuf::from("."));
    let every = interval(seconds);

    #[cfg(windows)]
    {
        windows::run(store, every)
    }

    #[cfg(not(windows))]
    {
        unix::run(&store, every);
        Ok(Exit::Success)
    }
}

/// systemd and launchd: be alive, and let `SIGTERM` do what `SIGTERM` does.
///
/// **No signal handler, and that is correct rather than lazy.** The default disposition of
/// `SIGTERM` is to terminate the process, which is exactly what `systemctl stop` and
/// `launchctl kill` want to happen — a handler here would only be a way to get that wrong.
/// There is nothing in flight to finish: a round either completed before the signal or had
/// not started.
#[cfg(not(windows))]
mod unix {
    use std::path::Path;
    use std::time::Duration;

    use crate::service::watch::Round;

    pub fn run(store: &Path, every: Duration) {
        crate::note!(
            "sloop service started, reading {} every {} seconds",
            store.display(),
            every.as_secs()
        );

        // Made once, outside the loop: it holds the SSH forwards, what the last round read and
        // what the last one complained about — which is what keeps a journal readable.
        let mut round = Round::at(store);
        loop {
            round.turn();
            std::thread::sleep(every);
        }
    }
}

/// The Service Control Manager's side of the conversation.
///
/// The dispatcher takes this thread and does not give it back until the service stops, which
/// is why `service run` can be nothing else.
#[cfg(windows)]
mod windows {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::Duration;

    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    use crate::exit::Exit;
    use crate::failure::{Failure, Outcome};
    use crate::service::unit::SERVICE_NAME;
    use crate::service::watch::Round;

    define_windows_service!(ffi_service_main, service_main);

    /// Where `service_main` reads the store from.
    ///
    /// **A `OnceLock` rather than an argument, because the signature is not ours.**
    /// `define_windows_service!` generates a function of a shape the Service Control Manager
    /// dictates, so the store has to be put somewhere both sides can see. It is set before the
    /// dispatcher is handed the thread, and the dispatcher is what calls `service_main`, so it
    /// is always written before it is read.
    ///
    /// The value itself came in on the ordinary command line: `sc.exe` records
    /// `binPath= "...\sloop.exe" service run --store "..."`, and the SCM starts that process
    /// with those arguments like any other. The `Vec<OsString>` the SCM passes to
    /// `service_main` is a different thing -- the arguments of `sc start`, which sloop never
    /// uses and which a service restarted by recovery settings would not get anyway.
    static STORE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

    /// How long a round waits, for the same reason [`STORE`] is here.
    static EVERY: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();

    /// Hand this thread to the SCM. It comes back when the service has stopped.
    pub fn run(store: PathBuf, every: Duration) -> Outcome<Exit> {
        let _ = STORE.set(store);
        let _ = EVERY.set(every);

        service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|error| {
            Failure::new(
                Exit::Failure,
                format!("this is not running as a Windows service: {error}"),
            )
            .hint(
                "`service run` is what the Service Control Manager starts. To use sloop from \
                 a terminal, run the command you want directly.",
            )
        })?;
        Ok(Exit::Success)
    }

    fn service_main(_arguments: Vec<std::ffi::OsString>) {
        let store = STORE.get().cloned().unwrap_or_else(|| PathBuf::from("."));
        let every = EVERY
            .get()
            .copied()
            .unwrap_or_else(|| super::interval(super::INTERVAL_SECONDS));
        if let Err(error) = serve(&store, every) {
            // Nowhere useful to print: the SCM has this process's streams. The service simply
            // reports that it stopped, and the exit code is what the machine sees.
            let _ = error;
        }
    }

    fn serve(store: &Path, every: Duration) -> Result<(), windows_service::Error> {
        let (stopping, stopped) = mpsc::channel();

        let handle =
            service_control_handler::register(SERVICE_NAME, move |control| match control {
                // Answering `Interrogate` is how a service says "still here" to a manager that
                // asks; refusing it is one of the ways a service gets declared hung.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = stopping.send(());
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            })?;

        let running = |state: ServiceState, accepts: ServiceControlAccept| ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accepts,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        };

        // Within thirty seconds of starting or the SCM kills this process as hung.
        handle.set_service_status(running(
            ServiceState::Running,
            // `SHUTDOWN` as well as `STOP`, so a machine being restarted stops sloop properly
            // rather than pulling the floor out from under it.
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ))?;

        // Outside the loop, for the reason the Unix half makes one there: the state that keeps
        // the event log from filling with the same line every minute lives in it.
        let mut round = Round::at(store);
        loop {
            round.turn();
            if stopped.recv_timeout(every).is_ok() {
                break;
            }
        }

        handle.set_service_status(running(
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
        ))?;
        Ok(())
    }
}
