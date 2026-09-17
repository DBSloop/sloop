//! `sloop server install` — the command behind `R19d`'s screen, and the flag form of it.
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

    let workspace = install::home(global).join("download");
    let read = reader(&workspace);

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
    if !asked.yes && !confirmed("Download and install it now?")? {
        return Err(Failure::new(Exit::Usage, "nothing was installed").hint(instead(engine)));
    }

    let installed = install::run(global, &build)?;
    report(&installed);
    Ok(Exit::Success)
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
fn reader(workspace: &Path) -> impl Fn(&str) -> Outcome<String> + use<'_> {
    move |url: &str| {
        std::fs::create_dir_all(workspace).map_err(|error| {
            Failure::usage(format!("could not create {}: {error}", workspace.display()))
        })?;

        let into: PathBuf = workspace.join("index.json");
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

/// Which engine, asked as a numbered list.
fn ask_which_engine() -> Outcome<Engine> {
    crate::say!("{}", style::heading("Which engine?"));

    let mut rows = Vec::new();
    for listed in catalogue::ENGINES {
        if let Some(engine) = listed.engine {
            rows.push(engine);
            crate::say!(
                "  {}  {}  {}",
                style::paint(&format!("{}.", rows.len())),
                listed.name,
                style::dim(listed.blurb)
            );
        } else {
            // **On the screen and not choosable**, which is exactly what `R19d` asked for:
            // a menu that omits MongoDB reads as a tool that has never heard of it.
            crate::say!(
                "  {}  {}  {}",
                style::dim("--"),
                style::dim(listed.name),
                style::dim(listed.blurb)
            );
        }
    }

    let picked = ask_a_number("Engine", rows.len())?;
    Ok(rows[picked])
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
