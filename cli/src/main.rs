//! `sloop` — register your databases once, then back them up, restore them, mirror them
//! and sync them.
//!
//! This build knows where things live. `sloop init` starts a project registry, and every
//! command that will one day read one already resolves which one it would read and says
//! so, so the order in `registry` is not a thing that only tests can see.

mod backup;
mod cli;
mod commands;
mod engine;
mod exit;
mod failure;
mod registry;
mod secret;
mod style;
mod tools;
mod wordmark;

use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::{Cli, Command, DbCommand};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::locations::Locations;
use crate::registry::{Disk, Registries, projects, resolve};

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

            // Reading the registries here is what makes R3 visible from outside: a file
            // that will not parse fails now, with the file named, instead of surprising
            // someone in the middle of a backup. Both scopes, because R2's rule is that a
            // bare name searches the project and then the global store.
            let registries = Registries::open(resolution.clone(), &global)?;

            if let Some(Command::Doctor { offline }) = &cli.command {
                // Only offer to install when there is somebody there to answer. Without a
                // terminal `doctor` is a report and nothing else, which is what a health
                // check in a pipeline wants it to be.
                let interactive = std::io::stdin().is_terminal();
                return Ok(commands::doctor::run(
                    &global,
                    interactive,
                    &commands::doctor::Registered {
                        registries: &registries,
                        from: resolution.describe(&global),
                        password_command: cli.password_command.as_deref(),
                        offline: *offline,
                    },
                ));
            }

            if let Some(Command::Backup { name, all }) = &cli.command {
                return commands::backup::run(
                    &commands::backup::Context {
                        registries: &registries,
                        global: &global,
                        password_command: cli.password_command.as_deref(),
                    },
                    name.as_deref(),
                    *all,
                );
            }

            if let Some(Command::Db { command }) = &cli.command {
                let mut context = commands::db::Context {
                    registries,
                    password_command: cli.password_command.as_deref(),
                    global: &global,
                };
                return db(&mut context, command);
            }

            Ok(unimplemented(
                &format!("'{}'", command.path()),
                Some(&[
                    format!("It would have read {}.", resolution.describe(&global)),
                    format!(
                        "A name would be looked for in {}.",
                        resolution.describe_lookup(&global)
                    ),
                    describe_registry(&registries, cli.password_command.as_deref()),
                ]),
            ))
        }

        Some(command) => Ok(unimplemented(&format!("'{}'", command.path()), None)),
        None => Ok(unimplemented("the interactive menu", None)),
    }
}

/// Hand a `db` subcommand its arguments.
///
/// One place, so that the shape of each command is declared in `cli` and taken apart
/// here, and nowhere in `commands::db` has to know what clap looks like.
fn db(context: &mut commands::db::Context<'_>, command: &DbCommand) -> Outcome<Exit> {
    match command {
        DbCommand::Add {
            name,
            url,
            fields,
            password,
            test,
            force,
        } => commands::db::add(
            context,
            name,
            url.as_deref(),
            fields,
            password,
            *test,
            *force,
        ),
        DbCommand::List => Ok(commands::db::list(context)),
        DbCommand::Test { name } => commands::db::test(context, name.as_deref()),
        DbCommand::Edit {
            name,
            url,
            fields,
            password,
            test,
        } => commands::db::edit(context, name, url.as_deref(), fields, password, *test),
        DbCommand::Rename { from, to } => commands::db::rename(context, from, to),
        DbCommand::Remove { name, yes } => commands::db::remove(context, name, *yes),
        DbCommand::Drop {
            name,
            confirm,
            no_backup,
        } => commands::db::drop(context, name, confirm.as_deref(), *no_backup),
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

/// What the registries hold, and where each password would be fetched from.
///
/// Routes only. Not one of these phrases can contain a password, because a route is a
/// direction and never a value — which is the property that lets this be printed at all.
fn describe_registry(registries: &Registries, password_command: Option<&str>) -> String {
    if registries.is_empty() {
        return "It has no databases in it yet.".to_owned();
    }

    let listed: Vec<String> = registries
        .all()
        .map(|(_, name, database)| {
            let route = database.password.overridden_by(password_command);
            format!("{name} via {}", route.describe())
        })
        .collect();

    let plural = if registries.len() == 1 {
        "database"
    } else {
        "databases"
    };
    format!(
        "It holds {} {plural}: {}.",
        registries.len(),
        listed.join(", ")
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
