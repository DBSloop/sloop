//! The command surface.
//!
//! Every command `sloop` will ever answer to is declared here and nowhere else, so
//! `--help` and the interactive menu can never drift apart. The bodies arrive one task at
//! a time; the shape does not change underneath them.

use clap::{Parser, Subcommand};

use crate::style;

const LONG_ABOUT: &str = "\
Register your databases once — then back them up, restore them, mirror them and sync \
them, from a menu or from a flag, on Windows, Linux and macOS.

Your credentials never leave this machine. No telemetry, no analytics, no update pings: \
there is no HTTP client anywhere in this binary's dependency graph, so there is nothing \
in it that could phone home even if it wanted to.";

/// Register your databases once — then back up, restore, mirror and sync them.
#[derive(Debug, Parser)]
#[command(
    name = "sloop",
    bin_name = "sloop",
    version,
    long_about = LONG_ABOUT,
    after_help = after_help(),
    after_long_help = after_long_help(),
    styles = style::clap_styles(),
    max_term_width = 100,
)]
pub struct Cli {
    /// Use the global registry, whichever directory you are standing in.
    #[arg(long, global = true, conflicts_with = "project")]
    pub global: bool,

    /// Work on this project: a directory, or a name `sloop init` recorded.
    ///
    /// `SLOOP_PROJECT` says the same thing from the environment, and this outranks it.
    #[arg(short = 'C', long = "project", value_name = "PATH|NAME", global = true)]
    pub project: Option<String>,

    /// Left empty on purpose: no command opens the interactive menu.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Everything `sloop` can be asked to do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start a project registry in this directory, or in the one `-C` names.
    Init,

    /// Report which client tools are present, missing or too old.
    Doctor,

    /// Register and manage database connections.
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },

    /// Back up a registered database.
    Backup,

    /// Inspect and prune stored backups.
    Backups {
        #[command(subcommand)]
        command: BackupsCommand,
    },

    /// Restore a stored backup into a registered database.
    Restore,

    /// Copy a database exactly, leaving the destination identical to the source.
    Mirror,

    /// Merge a database into another, keeping rows the destination already had.
    Sync,

    /// Move the backup encryption key between machines.
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },

    /// Remove sloop from this machine.
    Uninstall,
}

/// `sloop db …` — everything that touches the registry.
#[derive(Debug, Subcommand)]
pub enum DbCommand {
    /// Register a database, by URL or field by field.
    Add,
    /// Show every registered database. Never shows a secret.
    List,
    /// Open a connection to a registered database and report what happened.
    Test,
    /// Change a registered database's details.
    Edit,
    /// Give a registered database a different name.
    Rename,
    /// Forget a registered database. The server is not touched.
    Remove,
    /// Drop a database on the server. Backs it up first and asks for its name.
    Drop,
}

/// `sloop backups …` — everything that reads what has already been taken.
#[derive(Debug, Subcommand)]
pub enum BackupsCommand {
    /// Show stored backups, newest first, in local time.
    List,
    /// Delete backups past the retention limit, after showing which ones.
    Prune,
}

/// `sloop key …` — moving the one secret that restoring needs.
#[derive(Debug, Subcommand)]
pub enum KeyCommand {
    /// Write the private key out, so losing this machine is not losing the backups.
    Export,
    /// Take a private key exported from another machine.
    Import,
}

impl Command {
    /// Does this command read a registry?
    ///
    /// The ones that do resolve which registry before they do anything else, so `-C` at a
    /// project that does not exist fails as usage rather than halfway through the work.
    /// `doctor` inspects the machine, `uninstall` removes the binary and `init` creates
    /// the thing the others read, so none of the three needs one.
    #[must_use]
    pub const fn uses_registry(&self) -> bool {
        matches!(
            self,
            Self::Db { .. }
                | Self::Backup
                | Self::Backups { .. }
                | Self::Restore
                | Self::Mirror
                | Self::Sync
                | Self::Key { .. }
        )
    }

    /// What the user typed, for the messages that have to name it back to them.
    #[must_use]
    pub fn path(&self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Doctor => "doctor",
            Self::Db { command } => command.path(),
            Self::Backup => "backup",
            Self::Backups { command } => command.path(),
            Self::Restore => "restore",
            Self::Mirror => "mirror",
            Self::Sync => "sync",
            Self::Key { command } => command.path(),
            Self::Uninstall => "uninstall",
        }
    }
}

impl DbCommand {
    #[must_use]
    pub fn path(&self) -> &'static str {
        match self {
            Self::Add => "db add",
            Self::List => "db list",
            Self::Test => "db test",
            Self::Edit => "db edit",
            Self::Rename => "db rename",
            Self::Remove => "db remove",
            Self::Drop => "db drop",
        }
    }
}

impl BackupsCommand {
    #[must_use]
    pub fn path(&self) -> &'static str {
        match self {
            Self::List => "backups list",
            Self::Prune => "backups prune",
        }
    }
}

impl KeyCommand {
    #[must_use]
    pub fn path(&self) -> &'static str {
        match self {
            Self::Export => "key export",
            Self::Import => "key import",
        }
    }
}

/// The one line worth adding to `-h`.
fn after_help() -> String {
    style::dim("Run `sloop` with no command for the interactive menu.")
}

/// `--help` has room to answer the three questions people actually arrive with: how do I
/// start it, which of those two copy commands do I want, and what does it exit with.
fn after_long_help() -> String {
    format!(
        "{menu}

{copying}
  mirror   An exact copy. The destination ends up identical to the source, and
           anything that existed only in the destination is gone.
  sync     A merge. Rows are added, rows already there are replaced, and rows
           that exist only in the destination are kept.

{codes}
  0 success   2 usage   3 connect   4 dump   5 restore   6 mismatch   7 locked

{guarantee}
  cargo tree --manifest-path cli/Cargo.toml | grep -Ei 'reqwest|hyper|ureq|curl'

  Nothing comes back, and CI fails the build on the day something does.",
        menu = after_help(),
        copying = style::heading("mirror and sync are not the same command"),
        codes = style::heading("Exit codes, frozen at 1.0"),
        guarantee = style::heading("Check the guarantee in ten seconds"),
    )
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn the_command_surface_is_valid() {
        Cli::command().debug_assert();
    }

    /// The tree the site, the menu and the README all describe. A command added without
    /// a thought about those three is a command that goes undocumented.
    #[test]
    fn the_top_level_commands_are_the_agreed_ones() {
        let found: Vec<_> = Cli::command()
            .get_subcommands()
            .map(|sub| sub.get_name().to_owned())
            .filter(|name| name != "help")
            .collect();

        assert_eq!(
            found,
            [
                "init",
                "doctor",
                "db",
                "backup",
                "backups",
                "restore",
                "mirror",
                "sync",
                "key",
                "uninstall",
            ]
        );
    }

    #[test]
    fn every_command_says_what_it_does() {
        fn walk(command: &clap::Command) {
            for sub in command.get_subcommands() {
                if sub.get_name() == "help" {
                    continue;
                }
                assert!(
                    sub.get_about().is_some(),
                    "`{}` has no description",
                    sub.get_name()
                );
                walk(sub);
            }
        }

        walk(&Cli::command());
    }
}
