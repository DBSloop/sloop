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

    /// Take the password from this command's output instead of wherever the registry
    /// says.
    ///
    /// For a team password manager: `--password-command "op read op://vault/db/password"`.
    /// The password comes back through a pipe, so it never appears in `ps`.
    #[arg(long, value_name = "COMMAND", global = true)]
    pub password_command: Option<String>,

    /// Left empty on purpose: no command opens the interactive menu.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Everything `sloop` can be asked to do.
///
/// `db add` carries a dozen optional fields and makes this enum lopsided, which clippy is
/// right about in general and wrong about here: exactly one of these is ever built, once,
/// from the command line, and boxing it would trade a clear derive for a pointer nothing
/// in this program is fast enough to notice.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start a project registry in this directory, or in the one `-C` names.
    Init,

    /// Report which client tools are present, and what each registered role may do.
    Doctor {
        /// Do not connect to anything. Reports the client tools and the documented
        /// privilege minimums, and asks no server about its grants.
        #[arg(long)]
        offline: bool,
    },

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
    #[command(after_long_help = ADD_EXAMPLES)]
    Add {
        /// What to call it here. Not the database's own name — the label you will type.
        name: String,

        /// The whole connection in one string: `postgres://app@db:5432/orders`.
        ///
        /// Anything you also pass as a flag wins over what the URL said, so a URL with a
        /// password in it can be corrected without retyping the rest.
        #[arg(long, value_name = "URL")]
        url: Option<String>,

        #[command(flatten)]
        fields: Fields,

        #[command(flatten)]
        password: PasswordSource,

        /// Connect before saving, and refuse to save if it does not work.
        #[arg(long)]
        test: bool,

        /// Replace an entry already registered under this name.
        #[arg(long)]
        force: bool,
    },

    /// Show every registered database. Never shows a secret.
    List,

    /// Open a connection to a registered database and report what happened.
    Test {
        /// Which one. Every registered database when this is left out.
        name: Option<String>,
    },

    /// Change a registered database's details.
    Edit {
        /// Which one.
        name: String,

        /// Replace every connection field at once, from a URL.
        #[arg(long, value_name = "URL")]
        url: Option<String>,

        #[command(flatten)]
        fields: Fields,

        #[command(flatten)]
        password: PasswordSource,

        /// Connect before saving, and refuse to save if it does not work.
        #[arg(long)]
        test: bool,
    },

    /// Give a registered database a different name.
    Rename {
        /// What it is called now.
        from: String,
        /// What to call it instead.
        to: String,
    },

    /// Forget a registered database. The server is not touched.
    Remove,

    /// Drop a database on the server. Backs it up first and asks for its name.
    Drop,
}

/// The connection, field by field.
///
/// Shared by `add` and `edit` so the two can never drift into accepting different things,
/// which is the difference the "Done when" of R7 is about.
#[derive(Debug, clap::Args)]
pub struct Fields {
    /// postgres, mysql or mariadb.
    #[arg(long, value_name = "ENGINE")]
    pub engine: Option<String>,

    /// Host name or address.
    #[arg(long, value_name = "HOST")]
    pub host: Option<String>,

    /// Port. Defaults to the engine's own.
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,

    /// The database's name on the server.
    #[arg(long, value_name = "NAME")]
    pub database: Option<String>,

    /// The role to connect as.
    #[arg(long, value_name = "ROLE")]
    pub user: Option<String>,
}

/// Where the password will come from, from now on.
///
/// **Never a `--password` flag, and there never will be one.** Everything in `argv` is
/// readable by every process on the machine, so a password that arrives that way is a
/// password that has already leaked. These four are the routes; the value itself is typed
/// at a prompt or piped in.
#[derive(Debug, clap::Args)]
pub struct PasswordSource {
    /// Keep the password in the OS keyring. The default.
    #[arg(long, group = "route")]
    pub keyring: bool,

    /// Keep it in the Argon2id-encrypted file, for a machine with no keyring.
    #[arg(long, group = "route")]
    pub encrypted_file: bool,

    /// Read it from this environment variable at run time, for automation.
    #[arg(long, value_name = "VARIABLE", group = "route")]
    pub env: Option<String>,

    /// Run this and read the password from its output, for a team password manager.
    #[arg(long, value_name = "COMMAND", group = "route")]
    pub password_from: Option<String>,

    /// Take the password from standard input instead of asking for it.
    ///
    /// The only way to register a stored password without a terminal. One trailing
    /// newline is removed and nothing else is touched.
    #[arg(long)]
    pub password_stdin: bool,
}

/// The half of `db add` that a paragraph explains better than a flag list.
const ADD_EXAMPLES: &str = "\
Examples:
  sloop db add orders --url postgres://app@db.internal:5432/orders
      Asks for the password and files it in the OS keyring.

  sloop db add orders --engine postgres --host db.internal --database orders --user app
      The same registration, field by field. The two are indistinguishable afterwards.

  sloop db add ci --url mysql://ci@db/app --env CI_DB_PASSWORD
      Nothing is stored: the password is read from that variable on every run.

  printf %s \"$PW\" | sloop db add nightly --url postgres://bk@db/app --password-stdin
      For a machine with no terminal to ask at.

A password never goes in a flag. `ps` shows every argument of every process on the
machine, so a password passed that way has already been read by anyone who wanted it.";

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
    /// `uninstall` removes the binary and `init` creates the thing the others read, so
    /// neither needs one.
    ///
    /// `doctor` is in the list and did not used to be: R6a gave it a second half that
    /// checks each registered role's privileges against the live connection, and it can
    /// only do that by reading the same registry every other command reads.
    #[must_use]
    pub const fn uses_registry(&self) -> bool {
        matches!(
            self,
            Self::Doctor { .. }
                | Self::Db { .. }
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
            Self::Doctor { .. } => "doctor",
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
            Self::Add { .. } => "db add",
            Self::List => "db list",
            Self::Test { .. } => "db test",
            Self::Edit { .. } => "db edit",
            Self::Rename { .. } => "db rename",
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
  0 success   2 usage      3 connect   4 dump   5 restore
              6 mismatch   7 locked    8 doctor found a problem

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
