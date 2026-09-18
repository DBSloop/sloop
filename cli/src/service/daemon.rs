//! The far side of `install` — the process the service manager actually runs.
//!
//! **What it does is deliberately almost nothing, and that is `R24`'s boundary.** Sampling is
//! `R26`, the schedule is `R27a`, and neither can be written until something is running to
//! write them into. What is here is the part all of them need and none of them can add later
//! without rewriting the other two: a process the three service managers each recognise as
//! healthy, that stops when told, and that comes back at boot.
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

/// How often the daemon wakes to do its round.
///
/// `R26` will make this the sampling interval and give it a setting; until there is something
/// to sample, it is how often the heartbeat proves the process is alive.
pub const INTERVAL: Duration = Duration::from_secs(60);

/// What one turn of the loop does.
///
/// Empty on purpose: `R26` fills it in, and having the call site already here means the
/// platform halves below never need touching again to get it.
fn one_round(store: &PathBuf) {
    let _ = store;
}

/// Run as a service, in whatever way this platform means by that.
pub fn run(store: Option<PathBuf>) -> Outcome<Exit> {
    let store = store.unwrap_or_else(|| PathBuf::from("."));

    #[cfg(windows)]
    {
        windows::run(store)
    }

    #[cfg(not(windows))]
    {
        unix::run(&store);
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

    pub fn run(store: &Path) {
        crate::note!("sloop service started, watching {}", store.display());
        loop {
            super::one_round(&store.to_path_buf());
            std::thread::sleep(super::INTERVAL);
        }
    }
}

/// The Service Control Manager's side of the conversation.
///
/// The dispatcher takes this thread and does not give it back until the service stops, which
/// is why `service run` can be nothing else.
#[cfg(windows)]
mod windows {
    use std::path::PathBuf;
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

    /// Hand this thread to the SCM. It comes back when the service has stopped.
    pub fn run(store: PathBuf) -> Outcome<Exit> {
        let _ = STORE.set(store);

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
        if let Err(error) = serve(&store) {
            // Nowhere useful to print: the SCM has this process's streams. The service simply
            // reports that it stopped, and the exit code is what the machine sees.
            let _ = error;
        }
    }

    fn serve(store: &PathBuf) -> Result<(), windows_service::Error> {
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

        loop {
            super::one_round(store);
            if stopped.recv_timeout(super::INTERVAL).is_ok() {
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
