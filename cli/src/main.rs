//! `sloop` — register your databases once, then back them up, restore them, mirror them
//! and sync them.
//!
//! This build knows where things live. `sloop init` starts a project registry, and every
//! command that will one day read one already resolves which one it would read and says
//! so, so the order in `registry` is not a thing that only tests can see.

mod backup;
mod cli;
mod commands;
mod consent;
mod crypt;
mod engine;
mod exit;
mod failure;
mod install;
mod lock;
mod pathentry;
mod query;
mod registry;
mod report;
mod secret;
mod server;
mod service;
mod ssh;
mod style;
mod tools;
mod ui;
mod verify;
mod wordmark;

use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::{BackupsCommand, Cli, Command, DbCommand, KeyCommand, ServerCommand};
use crate::consent::Consent;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::locations::Locations;
use crate::registry::{Disk, Registries, projects, resolve};

fn main() -> ExitCode {
    // **Before `clap`, because this process is not sloop.** `ssh` runs sloop as its own
    // askpass helper, handing the prompt as arguments that are not sloop's arguments at all —
    // so parsing them would be a usage error about a flag nobody typed. An environment
    // variable rather than a hidden subcommand keeps the agreed top-level command list the
    // agreed command list. See `ssh::askpass`.
    if ssh::askpass::is_the_helper() {
        return ssh::askpass::respond().into();
    }

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

    // **Before a command runs, and before anything is printed.** A `--log-file` that cannot
    // be opened is a usage error now rather than a surprise halfway through a backup, and a
    // `--no-color` decided after the first line has been written is a line with colour in it.
    if let Err(failure) = report::settle(&report::Asked {
        quiet: cli.quiet,
        json: cli.json,
        no_color: cli.no_color,
        log_file: cli.log_file.as_deref(),
        dry_run: cli.dry_run,
    }) {
        failure.report();
        return failure.exit().into();
    }

    let named = cli
        .command
        .as_ref()
        .map_or("menu", crate::cli::Command::path);

    match run(&cli) {
        Ok(exit) => {
            report::finish(named, exit);
            exit.into()
        }
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
            let global = registry::adopt::global(&locations)?;
            let target = init_target(cli.project.as_deref(), &global)?;
            commands::init::run(&target, &global, &locations.home_dir()?)?;
            Ok(Exit::Success)
        }

        Some(Command::Setup {
            superuser_password_command,
            superuser_password_stdin,
            db_password_command,
            db_password_stdin,
        }) => {
            let global = registry::adopt::global(&locations)?;
            let settled = server::set_up(
                &global,
                &server::make::Asking {
                    command: superuser_password_command.as_deref(),
                    stdin: *superuser_password_stdin,
                },
                &server::own::Choosing {
                    command: db_password_command.as_deref(),
                    stdin: *db_password_stdin,
                },
            )?;
            server::announce(&settled.ready);
            server::announce_own(&settled);
            Ok(Exit::Success)
        }

        // **Before `uses_registry`, because a machine with no registry can still install a
        // server.** This is the command somebody reaches for on a fresh machine, and failing
        // it with "run `sloop setup`" would be refusing to help for the wrong reason.
        // Before `uses_registry`: a service is registered with the machine, not with a
        // registry, and `service run` is started by something that has no terminal at all.
        Some(Command::Service { command }) => commands::service::run(&locations, command),

        Some(Command::Server { command }) => {
            let global = registry::adopt::global(&locations)?;
            server_command(&global, command, cli.yes, cli.force)
        }

        // **Before `uses_registry`, because reset is what removes the registry.** Opening
        // it first would fail with "run `sloop setup`" on the way into the command whose
        // whole job is to undo Setup.
        Some(command @ (Command::Reset | Command::Uninstall)) => commands::reset::run(
            &locations,
            Consent::given(cli.yes, cli.force, cli.confirm.as_deref()),
            matches!(command, Command::Uninstall),
        ),

        Some(command) if command.uses_registry() => with_registry(cli, &locations, command),

        Some(command) => Ok(unimplemented(&format!("'{}'", command.path()), None)),
        None => menu(cli, &locations),
    }
}

/// `sloop` with nothing after it: the interactive menu.
///
/// Everything the shell *shows* is worked out here and handed over as plain data;
/// everything it *runs* goes through [`commands::menu::Machine`], which reopens the
/// registry per job. Neither half lets `ui` hold a borrow of the registry, which is what
/// lets a command that changes it be reached from inside the menu.
fn menu(cli: &Cli, locations: &Locations) -> Outcome<Exit> {
    let mut machine = commands::menu::Machine::new(
        locations,
        cli.global,
        cli.project.as_deref(),
        environment_project().as_deref(),
        cli.password_command.as_deref(),
    )?;

    let global = machine.global().to_path_buf();
    let resolution = machine.resolution().clone();

    // **Asked, not assumed, and asked without a prompt.** A machine that has been set up has
    // a record in `~/.sloop` naming its PostgreSQL, and reading it is the whole question —
    // which is how the owner's rule holds: a set-up machine is never asked anything about
    // setup, because nothing is left to ask.
    let set_up = crate::registry::store::Store::open(&global)?.is_some();

    // Nothing is registered anywhere on a machine with no registry to read. Asked only when
    // there is one, so a machine before Setup does not fail on the way to being told so.
    let holds = if set_up {
        ui::flow::Doing::databases(&machine).len()
    } else {
        0
    };

    let mut shell = ui::screen::Shell {
        home: locations.home_dir()?,
        working_in: resolution
            .registry_dir()
            .unwrap_or_else(|| global.clone())
            .display()
            .to_string(),
        found_by: resolution.why(),
        // **A first run is nothing registered and nowhere to register it**: no `.sloop`
        // at or above the working directory, and a global store that is still empty. Either
        // one on its own is an ordinary session — somebody with a global registry and no
        // project has already started, and so has somebody standing in a fresh project.
        fresh: resolution.registry_dir().is_none() && holds == 0,
        set_up,
        holds,
        cwd: machine.cwd().to_path_buf(),
        global: global.clone(),
    };

    let (exit, kept) = ui::run(&mut shell, &mut machine)?;

    // Out here, and only out here: the alternate screen has been handed back, so these
    // lines land in the scrollback the user keeps rather than in the one that vanishes.
    for keeping in &kept {
        match keeping {
            ui::screen::Kept::Started(report) => commands::init::announce(report, &global),
        }
    }

    Ok(exit)
}

/// Every command that reads a registry, once the registry has been read.
///
/// **Its own function because it is the whole tool.** Resolving which registry, opening
/// both, and settling what this run has permission to do are three things every one of
/// these commands needs and none of them should repeat — and `run` above stays short
/// enough to read in one go, which is the only way the two commands that *don't* read a
/// registry stay visible in it.
fn with_registry(cli: &Cli, locations: &Locations, command: &Command) -> Outcome<Exit> {
    let global = registry::adopt::global(locations)?;
    let world = Disk::new(&global, locations.home_dir()?);
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

    // One place, once: what this run was given permission to do. Every
    // command that asks a question or destroys something reads it from here,
    // so there is one implementation of rule 4 and one of rule 5 rather than
    // a copy per command drifting apart. See `consent`.
    let consent = Consent::given(cli.yes, cli.force, cli.confirm.as_deref());

    // **Made here so that it is dropped here.** Every forward this run opens is killed when
    // this value goes out of scope — a local in a function that returns, not a global whose
    // destructor may or may not run at process exit. A tunnel left open is a port on
    // somebody's machine that still reaches their database, so the guarantee that closes it
    // has to be one the language makes rather than one the code remembers.
    let tunnels = ssh::tunnel::Tunnels::new()?;

    dispatch(
        cli,
        World {
            registries,
            global: &global,
            password_command: cli.password_command.as_deref(),
            consent,
            resolution,
            tunnels: &tunnels,
        },
        command,
    )
}

/// Which command, now that everything every command needs has been worked out.
///
/// **Split from [`with_registry`] because the two do different jobs.** Above is the part
/// that can fail before a command is even chosen — an unreadable registry, a project that is
/// not there. This is the switchboard, and it stays a flat list so that adding a command is
/// adding one entry rather than finding somewhere to put it.
fn dispatch(cli: &Cli, world: World<'_>, command: &Command) -> Outcome<Exit> {
    if let Some(Command::Doctor { offline }) = &cli.command {
        return Ok(doctor(cli, &world, *offline));
    }

    if let Some(Command::Backup {
        name,
        all,
        sequential: _,
        replace,
    }) = &cli.command
    {
        return backup(&mut world.backing_up(), name.as_deref(), *all, *replace);
    }

    if let Some(Command::Backups { command }) = &cli.command {
        return backups(&world.listing_backups(), command);
    }

    if let Some(Command::Restore { name, from }) = &cli.command {
        return commands::restore::run(&world.restoring(), name, from.as_deref());
    }

    if let Some(Command::Mirror {
        source,
        to,
        create,
        new,
        table,
        with_references,
        safe,
    }) = &cli.command
    {
        return commands::mirror::run(
            &mut world.mirroring(),
            &commands::mirror::Mirroring {
                source,
                to: to.as_deref(),
                create: create.as_deref(),
                safe: *safe,
                new: new.into(),
                only: commands::tables::Selection {
                    patterns: table,
                    with_references: *with_references,
                },
            },
        );
    }

    if let Some(Command::Sync {
        source,
        to,
        create,
        new,
        table,
        with_references,
        safe,
    }) = &cli.command
    {
        return commands::sync::run(
            &mut world.syncing(),
            &commands::sync::Syncing {
                source,
                to: to.as_deref(),
                create: create.as_deref(),
                safe: *safe,
                new: new.into(),
                only: commands::tables::Selection {
                    patterns: table,
                    with_references: *with_references,
                },
            },
        );
    }

    if let Some(Command::Query { name, sql }) = &cli.command {
        return commands::query::run(
            &world.querying(),
            &commands::query::Asking {
                name: name.as_deref(),
                sql: sql.as_deref(),
            },
        );
    }

    if let Some(Command::Key { command }) = &cli.command {
        return key(&mut world.keeping_a_key(), command);
    }

    if let Some(Command::Db { command }) = &cli.command {
        return db(&mut world.registering(), command);
    }

    Ok(unimplemented(
        &format!("'{}'", command.path()),
        Some(&[
            format!(
                "It would have read {}.",
                world.resolution.describe(world.global)
            ),
            format!(
                "A name would be looked for in {}.",
                world.resolution.describe_lookup(world.global)
            ),
            describe_registry(&world.registries, world.password_command),
        ]),
    ))
}

/// Everything a registry-reading command is handed, before it is handed to one.
///
/// **One struct because every command wanted the same four things** and each was writing them
/// out again: the registries, where the global store is, what `--password-command` said, and
/// what this run has permission to do. Each command's own `Context` is a subset of these, so
/// the conversions below are the only place that shape is written down twice.
struct World<'a> {
    registries: Registries,
    global: &'a Path,
    password_command: Option<&'a str>,
    consent: Consent<'a>,
    resolution: registry::Resolution,
    /// Every SSH forward this run holds, shared by every command it dispatches.
    tunnels: &'a ssh::tunnel::Tunnels,
}

impl<'a> World<'a> {
    fn backing_up(self) -> commands::backup::Context<'a> {
        commands::backup::Context {
            registries: self.registries,
            global: self.global,
            password_command: self.password_command,
            tunnels: self.tunnels,
        }
    }

    fn listing_backups(self) -> commands::backups::Context<'a> {
        commands::backups::Context {
            registries: self.registries,
            global: self.global,
            consent: self.consent,
        }
    }

    fn restoring(self) -> commands::restore::Context<'a> {
        commands::restore::Context {
            registries: self.registries,
            global: self.global,
            password_command: self.password_command,
            consent: self.consent,
            tunnels: self.tunnels,
        }
    }

    fn mirroring(self) -> commands::mirror::Context<'a> {
        commands::mirror::Context {
            registries: self.registries,
            global: self.global,
            password_command: self.password_command,
            consent: self.consent,
            tunnels: self.tunnels,
        }
    }

    fn syncing(self) -> commands::sync::Context<'a> {
        commands::sync::Context {
            registries: self.registries,
            global: self.global,
            password_command: self.password_command,
            consent: self.consent,
            tunnels: self.tunnels,
        }
    }

    fn keeping_a_key(self) -> commands::key::Context<'a> {
        commands::key::Context {
            registries: self.registries,
            global: self.global,
        }
    }

    fn querying(self) -> commands::query::Context<'a> {
        commands::query::Context {
            registries: self.registries,
            password_command: self.password_command,
            global: self.global,
            tunnels: self.tunnels,
        }
    }

    fn registering(self) -> commands::db::Context<'a> {
        commands::db::Context {
            registries: self.registries,
            password_command: self.password_command,
            global: self.global,
            consent: self.consent,
            tunnels: self.tunnels,
        }
    }
}

/// Hand a `server` subcommand its arguments.
fn server_command(global: &Path, command: &ServerCommand, yes: bool, force: bool) -> Outcome<Exit> {
    match command {
        ServerCommand::Install { engine, version } => commands::server::install(
            global,
            &commands::server::Installing {
                engine: engine.as_deref(),
                version: version.as_deref(),
                yes,
            },
        ),
        ServerCommand::List => commands::server::list(global),
        ServerCommand::Connection { show_password } => commands::server::connection(
            global,
            &commands::server::Showing {
                password: *show_password,
                force,
            },
        ),
    }
}

/// Hand a `backups` subcommand its arguments.
///
/// The same reason `db` has one below: clap's shape is taken apart here, so nothing in
/// `commands::backups` has to know what a derived enum looks like.
fn backups(context: &commands::backups::Context<'_>, command: &BackupsCommand) -> Outcome<Exit> {
    match command {
        BackupsCommand::List { name, check } => {
            commands::backups::list(context, name.as_deref(), *check)
        }
        BackupsCommand::Prune {
            name,
            keep,
            older_than,
            dry_run,
            include_broken,
        } => commands::backups::prune(
            context,
            &commands::backups::Pruning {
                name: name.as_deref(),
                keep: *keep,
                older_than: older_than.as_deref(),
                dry_run: *dry_run,
                include_broken: *include_broken,
            },
        ),
    }
}

/// Hand `backup` its arguments.
fn backup(
    context: &mut commands::backup::Context<'_>,
    name: Option<&str>,
    all: bool,
    replace: bool,
) -> Outcome<Exit> {
    // `--sequential` is the default, so it is read for what it says rather than for what it
    // changes: a cron line that spells the mode out stays correct whatever a later release
    // makes the default.
    let mode = if replace {
        commands::backup::Mode::Replace
    } else {
        commands::backup::Mode::Sequential
    };
    commands::backup::run(context, name, all, mode)
}

/// Hand `doctor` its arguments.
///
/// It cannot fail, and says so: a health check that refused to report because something was
/// wrong would be a health check nobody could use. What it found is in the exit code.
fn doctor(cli: &Cli, world: &World<'_>, offline: bool) -> Exit {
    // Only offer to install when there is somebody there to answer. Without a terminal
    // `doctor` is a report and nothing else, which is what a health check in a pipeline
    // wants it to be.
    let interactive = std::io::stdin().is_terminal();
    commands::doctor::run(
        world.global,
        interactive,
        &commands::doctor::Registered {
            registries: &world.registries,
            from: world.resolution.describe(world.global),
            password_command: cli.password_command.as_deref(),
            offline,
            tunnels: world.tunnels,
        },
    )
}

/// Hand a `key` subcommand its arguments.
fn key(context: &mut commands::key::Context<'_>, command: &KeyCommand) -> Outcome<Exit> {
    match command {
        KeyCommand::Export => commands::key::export(context),
        KeyCommand::Import => commands::key::import(context),
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
            ssh,
            test,
        } => commands::db::add(context, name, url.as_deref(), fields, password, ssh, *test),
        DbCommand::Create {
            name,
            engine,
            host,
            port,
            superuser,
            superuser_password_stdin,
            superuser_password_command,
            database,
            role,
            role_password_stdin,
            role_password_command,
        } => commands::db::create(
            context,
            &commands::db::Creating {
                name,
                engine,
                host,
                port: *port,
                superuser: superuser.as_deref(),
                superuser_password_stdin: *superuser_password_stdin,
                superuser_password_command: superuser_password_command.as_deref(),
                database: database.as_deref(),
                role: role.as_deref(),
                role_password_stdin: *role_password_stdin,
                role_password_command: role_password_command.as_deref(),
            },
        ),
        DbCommand::List => Ok(commands::db::list(context)),
        DbCommand::Test { name } => commands::db::test(context, name.as_deref()),
        DbCommand::Edit {
            name,
            url,
            fields,
            password,
            ssh,
            test,
        } => commands::db::edit(context, name, url.as_deref(), fields, password, ssh, *test),
        DbCommand::Rename { from, to } => commands::db::rename(context, from, to),
        DbCommand::Remove { name } => commands::db::remove(context, name),
        DbCommand::Drop { name } => commands::db::drop(context, name),
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
