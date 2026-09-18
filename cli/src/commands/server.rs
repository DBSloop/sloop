//! `sloop server …` — the database servers on this machine, and sloop's own among them.
//!
//! `install` is `R19d`'s screen and the flag form of it, `list` is what it left behind, and
//! `connection` is `R19f`: where sloop's own database is, so the owner of the machine can
//! open it in DataGrip themselves. What that one prints and what it refuses to print is
//! documented on the function; the rest of this header is `install`'s.
//!
//! **The menu collects the engine; this collects the rest, and that split is not arbitrary.**
//! Resolving MariaDB's versions means reading an index, reading an index means `curl`, and
//! `curl` writes its progress to the terminal — which the menu is holding. So the question
//! that needs the network is asked here, on the terminal the menu has already handed back,
//! exactly as `db create`'s passwords are. See `ui::flow`'s header for the rule this follows.
//!
//! **Three questions and a confirmation, which is what the owner asked for:** *"this will
//! help user installing a db in just few steps instead of finding it's website and installing
//! instructions"*. Which engine, which version, and then one screen naming the archive, what
//! will prove it, how big it is, where it will go and which port it will take — because the
//! last thing somebody reads before four hundred megabytes starts moving should say all five.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use crate::engine::Engine;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::install;
use crate::server::connection::Unread;
use crate::style;
use crate::tools::acquire;
use crate::tools::catalogue::{self, Build, Choice};

/// What the flag surface carries, so that a session done by hand becomes a line somebody can
/// schedule — which is rule "every interactive run ends by printing the flag equivalent".
pub struct Installing<'a> {
    /// `postgres`, `mysql` or `mariadb`. `None` asks.
    pub engine: Option<&'a str>,
    /// The version as the project writes it. `None` asks.
    pub version: Option<&'a str>,
    /// `--yes`: do not ask before downloading.
    pub yes: bool,
}

/// Install a database server.
pub fn install(global: &Path, asked: &Installing<'_>) -> Outcome<Exit> {
    let terminal = std::io::stdin().is_terminal();

    // **Rule 4, and this is the sharpest case of it.** A scheduled run stopped on a question
    // about a download nobody will ever see is the worst thing this tool can do — so without
    // a terminal every answer has to have been given already, and the refusal names the exact
    // flags rather than leaving somebody to guess which one was missing.
    let fully_answered = asked.engine.is_some() && asked.version.is_some() && asked.yes;
    if !terminal && !fully_answered {
        return Err(Failure::new(
            Exit::Usage,
            "installing a database server downloads one, and there is no terminal to ask at",
        )
        .hint(
            "give every answer on the command line: `sloop server install <engine> --version \
             <version> --yes`",
        ));
    }

    let read = reader();

    let engine = match asked.engine {
        Some(named) => engine_named(named)?,
        None => ask_which_engine()?,
    };

    let choices = catalogue::choices(engine, &read)?;
    if choices.is_empty() {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "sloop has no {} to offer on this machine",
                engine.proper_name()
            ),
        )
        .hint(instead(engine)));
    }

    let choice = match asked.version {
        Some(wanted) => named_version(&choices, engine, wanted)?,
        None => ask_which_version(engine, &choices)?,
    };

    let build = catalogue::resolve(engine, &choice, &read)?;

    // **Before the screen, not after it.** Printing four lines about a download and then
    // saying "that is already here" reads as a tool that was not paying attention.
    install::already_installed(global, &build)?;

    let port = install::port::choose(engine.default_port(), &install::record::ports(global))?;
    announce(&build, global, port);

    // **The one `--dry-run` stop, at the moment this stops reading and starts changing
    // something.** Everything above it is reading: the index, which versions exist, which of
    // them was asked for, whether one is already installed, which port is free. Nothing below
    // it is — so a rehearsal has done every check that makes it worth having and has left
    // four hundred megabytes on the other end of the wire.
    if crate::report::would(&format!(
        "install {} into {} on port {port}",
        build.describe(),
        install::home_for(global, build.engine, &build.version).display()
    )) {
        return Ok(Exit::Success);
    }

    if !asked.yes && !confirmed("Download and install it now?")? {
        return Err(Failure::new(Exit::Usage, "nothing was installed").hint(instead(engine)));
    }

    let installed = install::run(global, &build)?;
    report(&installed);
    Ok(Exit::Success)
}

/// What `sloop server connection` was asked for.
pub struct Showing {
    /// `--show-password`: print the password too, not only where it is kept.
    pub password: bool,
    /// `--force`: print it even where it would not land on a terminal.
    pub force: bool,
}

/// Where sloop's own database is, and — asked for plainly — the password that opens it.
///
/// **`R19f`'s first piece.** The owner of the machine can now open `sloop_database` in
/// DataGrip, or in `psql`, or in anything else that speaks PostgreSQL, from what this prints
/// and without reading any source.
///
/// **Three places the password is not allowed to go, and two of them are absolute.** Never
/// into `--log-file`, which is by construction: it goes through [`crate::report::secret`],
/// which has no logging path in it. Never into `--json`, and never into a `--quiet` run,
/// because both say this run is not being read by a person and a password is only ever
/// printed for a person to read — those are refused here in so many words. And never onto a
/// stream that is not a terminal, which is rule 4's shape applied to output: a secret written
/// into a pipe is a secret in whatever that pipe was. That last one is a refusal somebody can
/// override, because `--force` is exactly the flag for overriding a refusal that is there to
/// protect something. What counts as somebody reading is
/// [`crate::server::connection::nobody_is_reading`], which Setup asks the same question of.
pub fn connection(global: &Path, showing: &Showing) -> Outcome<Exit> {
    let Some(connection) = crate::server::connection::Connection::of(global)? else {
        return Err(Failure::new(
            Exit::Usage,
            "sloop keeps its state in its own PostgreSQL, and this machine has not been set up",
        )
        .hint("run `sloop setup` once — it is safe to run again afterwards"));
    };

    if showing.password {
        refuse_unless_somebody_is_reading(showing.force)?;
    }

    crate::report::result(connection.document());
    connection.say();
    crate::say!();
    crate::say!(
        "  {}",
        style::dim("Open it with psql, DataGrip, or anything else that speaks PostgreSQL.")
    );

    if !showing.password {
        crate::say!(
            "  {}",
            style::dim(
                "`sloop server connection --show-password` prints the password on this \
                 terminal."
            )
        );
        return Ok(Exit::Success);
    }

    // Read only now. A run that was not asked for the password has no business touching the
    // keyring, and on a locked keychain that would be a prompt nobody asked for.
    let (own, password) = crate::server::record::database(global)?.ok_or_else(|| {
        Failure::new(
            Exit::Usage,
            "the record names sloop's database but not how to open it",
        )
        .hint("run `sloop setup` again — it settles the password and writes the record")
    })?;

    crate::server::connection::say_password(
        &own.role,
        &password,
        "It is not written to any file, and this line is never written to --log-file.",
    );
    Ok(Exit::Success)
}

/// Refuse to print a password where nobody is reading it, unless somebody said to anyway.
///
/// **Only one of the three is overridable.** A pipe is somebody's own choice about where
/// their own password goes, and `--force` is the flag for exactly that. `--json` and
/// `--quiet` are not choices about a password at all — they say this run is not being read,
/// and printing a secret into a run nobody is reading is what rule 3 is about.
fn refuse_unless_somebody_is_reading(force: bool) -> Outcome<()> {
    let Some(why) = crate::server::connection::nobody_is_reading() else {
        return Ok(());
    };

    if force && why == Unread::NotATerminal {
        return Ok(());
    }

    Err(Failure::new(
        Exit::Usage,
        format!(
            "a password is only ever printed for somebody to read, and {}",
            why.describe()
        ),
    )
    .hint(match why {
        Unread::Json => {
            "drop --json. sloop's password is never part of a document something else reads"
        }
        Unread::Quiet => "drop --quiet — it would silence the one line that was asked for",
        Unread::NotATerminal => {
            "a secret written into a pipe is a secret in whatever that pipe was. `--force` \
             says to print it anyway"
        }
    }))
}

/// Every server sloop has installed on this machine.
pub fn list(global: &Path) -> Outcome<Exit> {
    let held = install::record::read(global)?;

    if held.is_empty() {
        crate::say!(
            "{} sloop has not installed a database server on this machine.",
            style::heading("None:")
        );
        crate::say!(
            "  {}",
            style::dim(
                "`sloop server install` walks through engine, version and one confirmation."
            )
        );
        return Ok(Exit::Success);
    }

    crate::say!(
        "{} {} sloop installed",
        style::heading("Servers:"),
        held.len()
    );
    for installed in &held {
        let answering = install::is_answering(global, installed);
        crate::say!(
            "  {}  {}  {}",
            style::paint(&installed.describe()),
            installed.url(),
            if answering {
                style::in_hue(style::Hue::Ok, "answering")
            } else {
                style::dim("not running")
            }
        );
        crate::say!(
            "  {}",
            style::dim(&format!("  files at {}", installed.home.display()))
        );
    }

    Ok(Exit::Success)
}

/// What to say when there is nothing to offer, or when somebody said no.
fn instead(engine: Engine) -> String {
    let why = catalogue::why_nothing_is_offered(engine);
    let how = acquire::how_to_install(engine);
    if why.is_empty() {
        how
    } else {
        format!("{why}\n{how}")
    }
}

/// How the indexes are read: the system's own `curl`, to a file, which is then read back.
///
/// **The same download path as the archive, on purpose.** There is one place in sloop that
/// reaches the network and this is it, so an index cannot quietly acquire a second one — and
/// `cargo tree` still shows nothing that could speak HTTP.
///
/// **Into the system's temporary directory rather than the store**, because an index is not
/// part of an install. It is nine kilobytes, read once and deleted — and putting it under
/// `<global>/servers/` meant a `--dry-run` that changed nothing still left a directory
/// behind, which is not what a rehearsal promises.
fn reader() -> impl Fn(&str) -> Outcome<String> {
    move |url: &str| {
        let into: PathBuf = std::env::temp_dir().join(format!(
            "sloop-index-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));

        acquire::download(url, &into)?;
        let text = std::fs::read_to_string(&into).map_err(|error| {
            Failure::usage(format!("could not read {}: {error}", into.display()))
        })?;
        let _ = std::fs::remove_file(&into);
        Ok(text)
    }
}

/// The engine somebody named on the command line.
fn engine_named(named: &str) -> Outcome<Engine> {
    let wanted = named.trim().to_ascii_lowercase();

    if let Some(engine) = Engine::ALL.into_iter().find(|engine| {
        engine.scheme() == wanted || engine.proper_name().to_ascii_lowercase() == wanted
    }) {
        return Ok(engine);
    }

    // **Named as not yet supported, rather than as a typo.** Somebody who asked for MongoDB
    // did not misspell anything, and a usage error that reads like they did is a worse answer
    // than the true one.
    if let Some(listed) = catalogue::ENGINES
        .iter()
        .find(|listed| !listed.supported() && listed.name.to_ascii_lowercase() == wanted)
    {
        return Err(Failure::new(
            Exit::Usage,
            format!("sloop does not speak {} yet", listed.name),
        )
        .hint(format!(
            "{} — it is on the list, named as not yet",
            listed.blurb
        )));
    }

    Err(
        Failure::new(Exit::Usage, format!("{named} is not an engine sloop knows"))
            .hint("postgres, mysql or mariadb"),
    )
}

/// The version somebody named on the command line, matched against what is really published.
///
/// **A version that is not on the list is a refusal, never a guess.** Building the URL out of
/// whatever was typed would download a 404 page and then fail its checksum, which is a worse
/// way to say the same thing and four hundred megabytes more expensive.
fn named_version(choices: &[Choice], engine: Engine, wanted: &str) -> Outcome<Choice> {
    if let Some(found) = choices.iter().find(|choice| choice.version == wanted) {
        return Ok(found.clone());
    }

    // MariaDB's list is majors, so `--version 11.8.2` should find `11.8` rather than nothing.
    if let Some(found) = choices
        .iter()
        .find(|choice| wanted.starts_with(&format!("{}.", choice.version)))
    {
        return Ok(found.clone());
    }

    Err(Failure::new(
        Exit::Usage,
        format!(
            "{} {wanted} is not one sloop can install on this machine",
            engine.proper_name()
        ),
    )
    .hint(format!(
        "it offers {}",
        choices
            .iter()
            .map(|choice| choice.version.clone())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// The engine screen, as rows: what is drawn, and which of them can be chosen.
///
/// **Its own function so the thing `R19d` actually asked for is testable.** *"The ones sloop
/// does not yet speak, named as not yet supported"* is a property of what is on the screen,
/// and a screen that only exists inside a `say!` can be checked by looking at it once and
/// never again.
///
/// Every listed engine gets a row. A supported one gets the next number; an unsupported one
/// gets `--`, which is not a number anybody can type — so the numbering counts only what can
/// be chosen and nobody can pick MongoDB by typing `4`.
fn engine_rows() -> Vec<(Option<Engine>, String)> {
    let mut chosen = 0;

    catalogue::ENGINES
        .iter()
        .map(|listed| {
            let marker = if listed.supported() {
                chosen += 1;
                format!("{chosen}.")
            } else {
                "--".to_owned()
            };
            (
                listed.engine,
                format!("{marker:>3}  {}  {}", listed.name, listed.blurb),
            )
        })
        .collect()
}

/// Which engine, asked as a numbered list.
fn ask_which_engine() -> Outcome<Engine> {
    crate::say!("{}", style::heading("Which engine?"));

    let rows = engine_rows();
    for (engine, line) in &rows {
        // A row nobody can choose is drawn quietly, the way a heading is on the menu screen.
        crate::say!(
            "  {}",
            if engine.is_some() {
                line.clone()
            } else {
                style::dim(line)
            }
        );
    }

    let choosable: Vec<Engine> = rows.iter().filter_map(|(engine, _)| *engine).collect();
    let picked = ask_a_number("Engine", choosable.len())?;
    Ok(choosable[picked])
}

/// How many versions a screen shows before it becomes a wall of numbers nobody reads.
///
/// The newest handful is what somebody picking a server out of a menu means; the rest are
/// still reachable, by name, with `--version`.
const SHOW: usize = 8;

/// Which version, asked as a numbered list, newest first.
fn ask_which_version(engine: Engine, choices: &[Choice]) -> Outcome<Choice> {
    crate::say!();
    crate::say!(
        "{} {}",
        style::heading("Which version?"),
        style::dim(&format!("newest first, {} of them", choices.len()))
    );

    let showing = choices.len().min(SHOW);

    for (index, choice) in choices.iter().take(showing).enumerate() {
        crate::say!(
            "  {}  {} {}",
            style::paint(&format!("{}.", index + 1)),
            format_args!("{} {}", engine.proper_name(), choice.version),
            style::dim(&choice.note)
        );
    }
    if choices.len() > showing {
        crate::say!(
            "  {}",
            style::dim(&format!(
                "and {} older — `sloop server install {} --version <version>`",
                choices.len() - showing,
                engine.scheme()
            ))
        );
    }

    let picked = ask_a_number("Version", showing)?;
    Ok(choices[picked].clone())
}

/// Everything somebody should know before four hundred megabytes starts moving.
fn announce(build: &Build, global: &Path, port: u16) {
    let into = install::home_for(global, build.engine, &build.version);

    crate::say!();
    crate::say!("{} {}", style::heading("Installing:"), build.describe());
    crate::say!("  {}  {}", style::label("From"), build.url);
    crate::say!(
        "  {}  {}",
        style::label("Proved by"),
        build.proof.describe()
    );
    if let crate::tools::proof::Proof::Pinned { bytes, .. } = &build.proof {
        crate::say!("  {}  {} MB", style::label("Size"), bytes / 1_000_000);
    }
    crate::say!("  {}  {}", style::label("Into"), into.display());
    crate::say!(
        "  {}  {} {}",
        style::label("Port"),
        port,
        style::dim(if port == build.engine.default_port() {
            "the usual one, and free on this machine"
        } else {
            "the usual one was taken, so nothing already here is disturbed"
        })
    );
    crate::say!(
        "  {}",
        style::dim(
            "The download is the system's own curl. Nothing about this machine is sent \
             anywhere."
        )
    );
}

/// What was installed, and the line that makes it repeatable.
fn report(installed: &install::Installed) {
    crate::say!();
    crate::say!(
        "{} {}",
        style::heading("Installed:"),
        style::paint(&installed.describe())
    );
    crate::say!("  {}  {}", style::label("Answers on"), installed.url());
    crate::say!(
        "  {}  {}",
        style::label("Files at"),
        installed.home.display()
    );
    crate::say!(
        "  {}",
        style::dim(&format!(
            "{}'s password was generated and kept where this machine keeps secrets — it was \
             never written to a file and never passed on a command line",
            installed.superuser
        ))
    );
    crate::say!();
    crate::say!(
        "  {}",
        style::dim("Register a database on it with `sloop db create <name>`.")
    );
}

/// Ask for a number between 1 and `how_many`, and keep asking until it is one.
fn ask_a_number(what: &str, how_many: usize) -> Outcome<usize> {
    loop {
        crate::report::ask(&format!("{what} [1-{how_many}]: "));

        let line = read_a_line()?;
        let typed = line.trim();
        if typed.is_empty() {
            return Err(Failure::new(
                Exit::Usage,
                "nothing was chosen, so nothing was installed",
            ));
        }

        match typed.parse::<usize>() {
            Ok(number) if (1..=how_many).contains(&number) => return Ok(number - 1),
            _ => crate::note!("{} {typed} is not one of them.", style::error_prefix()),
        }
    }
}

/// Ask a yes-or-no question. Anything that is not a yes is a no.
fn confirmed(question: &str) -> Outcome<bool> {
    crate::report::ask(&format!("{question} [y/N] "));
    Ok(acquire::is_yes(&read_a_line()?))
}

/// One line from the terminal.
fn read_a_line() -> Outcome<String> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| {
            Failure::new(Exit::Usage, format!("could not read the answer: {error}"))
        })?;
    let _ = std::io::stdout().flush();
    Ok(line)
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
