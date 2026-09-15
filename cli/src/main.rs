//! `sloop` — register your databases once, then back them up, restore them, mirror them
//! and sync them.
//!
//! This build knows where things live. `sloop init` starts a project registry, and every
//! command that will one day read one already resolves which one it would read and says
//! so, so the order in `registry` is not a thing that only tests can see.

mod cli;
mod commands;
mod exit;
mod failure;
mod registry;
mod style;
mod wordmark;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::{Cli, Command};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::locations::Locations;
use crate::registry::{Disk, projects, resolve};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // `--help` and `--version` arrive here too. clap already knows which of the
            // two streams each belongs on; the exit code is ours, and a usage error is
            // frozen at 2 whatever clap's own default happens to be next year.
            let usage_error = error.use_stderr();
            let _ = error.print();
            return if usage_error {
                Exit::Usage.into()
            } else {
                Exit::Success.into()
            };
        }
    };

    match run(&cli) {
        Ok(exit) => exit.into(),
        Err(failure) => {
            failure.report();
            failure.exit().into()
        }
    }
}

fn run(cli: &Cli) -> Outcome<Exit> {
    let locations = Locations::from_process();

    match &cli.command {
        Some(Command::Init) => {
            if cli.global {
                return Err(Failure::usage("--global has nothing to initialise")
                    .hint("the global store appears on its own; sloop init starts a project"));
            }
            let global = locations.global_dir()?;
            let target = init_target(cli.project.as_deref(), &global)?;
            commands::init::run(&target, &global)?;
            Ok(Exit::Success)
        }

        Some(command) if command.uses_registry() => {
            let global = locations.global_dir()?;
            let world = Disk::new(&global);
            let cwd = working_directory()?;
            let resolution = resolve(
                &cwd,
                &world,
                cli.global,
                cli.project.as_deref(),
                environment_project().as_deref(),
            )?;

            Ok(unimplemented(
                &format!("'{}'", command.path()),
                Some(&[
                    format!("It would have read {}.", resolution.describe(&global)),
                    format!(
                        "A name would be looked for in {}.",
                        resolution.describe_lookup(&global)
                    ),
                ]),
            ))
        }

        Some(command) => Ok(unimplemented(&format!("'{}'", command.path()), None)),
        None => Ok(unimplemented("the interactive menu", None)),
    }
}

/// Where `sloop init` should put a registry.
///
/// `SLOOP_PROJECT` deliberately has no say here. It picks which project a command reads;
/// letting an exported variable decide where a new directory gets created is a different
/// and much less welcome thing.
fn init_target(flag: Option<&str>, global: &Path) -> Outcome<PathBuf> {
    let cwd = working_directory()?;

    let Some(argument) = flag.filter(|value| !value.is_empty()) else {
        return Ok(cwd);
    };

    let as_path = registry::normalize(&cwd.join(argument));
    if as_path.is_dir() {
        return Ok(as_path);
    }

    // A name already in the index, so re-running init on a known project works from
    // anywhere the same way every other command does.
    if let Some(known) = projects::read(global, argument) {
        return Ok(known);
    }

    Err(
        Failure::usage(format!("-C says {argument}, which is not a directory")).hint(
            "sloop init needs a directory that already exists, or the name of a project it knows",
        ),
    )
}

/// `SLOOP_PROJECT`, treating an empty value as unset the way a shell means it.
fn environment_project() -> Option<String> {
    std::env::var("SLOOP_PROJECT")
        .ok()
        .filter(|value| !value.is_empty())
}

fn working_directory() -> Outcome<PathBuf> {
    std::env::current_dir().map_err(|error| {
        Failure::usage(format!("cannot read the working directory: {error}"))
            .hint("this usually means it was removed while the shell was still in it")
    })
}

/// Say the honest thing and exit non-zero.
///
/// A stub that exits 0 is a stub that a script believes. This one does not use any of the
/// frozen codes either — none of them describes "nothing happened", and 1 already means
/// exactly that everywhere else.
fn unimplemented(what: &str, notes: Option<&[String]>) -> Exit {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "sloop: {what} is not implemented yet.");
    for note in notes.unwrap_or_default() {
        let _ = writeln!(stderr, "       {note}");
    }
    let _ = writeln!(
        stderr,
        "       This is an early build. https://github.com/DBSloop/sloop tracks what works."
    );
    Exit::Failure
}
