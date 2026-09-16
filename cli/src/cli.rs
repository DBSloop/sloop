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

    /// Answer yes to a question this would otherwise stop and ask.
    ///
    /// Never enough to destroy a named thing — that is `--confirm` — and never a way past a
    /// refusal, which is `--force`. `--help` has the three side by side.
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,

    /// Override a refusal that is there to protect something.
    ///
    /// Answers no questions. The refusal that can be overridden says so when it fires.
    #[arg(long, global = true)]
    pub force: bool,

    /// The name of the thing being destroyed, typed out.
    ///
    /// The only way to run a destructive command without a terminal, and it has to match
    /// exactly: a cron line then names what it destroys, so it cannot be repointed at
    /// something else by editing one flag.
    #[arg(long, value_name = "NAME", global = true)]
    pub confirm: Option<String>,

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
    #[command(after_long_help = BACKUP_NOTES)]
    Backup {
        /// Which registered database. Left out when `--all` is given.
        name: Option<String>,

        /// Back up every registered database, and do not stop at the first failure.
        #[arg(long, conflicts_with = "name")]
        all: bool,

        /// Keep every backup, in a directory named for the moment it was taken. The default.
        #[arg(long, group = "how")]
        sequential: bool,

        /// Keep one backup per database, at `backups/<engine>/<name>/latest`.
        #[arg(long, group = "how")]
        replace: bool,
    },

    /// Inspect and prune stored backups.
    Backups {
        #[command(subcommand)]
        command: BackupsCommand,
    },

    /// Restore a stored backup into a registered database.
    #[command(after_long_help = RESTORE_NOTES)]
    Restore {
        /// Which registered database to restore into.
        name: String,

        /// Which backup, by its directory's own name. The newest whole one by default.
        #[arg(long, value_name = "WHEN")]
        from: Option<String>,
    },

    /// Copy a database exactly, leaving the destination identical to the source.
    #[command(after_long_help = MIRROR_NOTES)]
    Mirror {
        /// The registered database to copy. Only ever read.
        source: String,

        /// The registered database to copy it over. Its contents are replaced.
        #[arg(long, value_name = "NAME", conflicts_with = "create")]
        to: Option<String>,

        /// Make the destination first, then copy into it. What to call it here.
        ///
        /// The database, the role that owns it and the grants all get made — the same work
        /// `sloop db create` does, with the engine taken from the source rather than asked
        /// for. On the source's own server unless --host says otherwise.
        #[arg(long, value_name = "NAME")]
        create: Option<String>,

        #[command(flatten)]
        new: NewDestination,

        /// Only this table. Repeatable, schema-qualified, and it takes a glob.
        ///
        /// `--table orders --table audit_*` or `--table public.audit_*`. A glob, never a
        /// regex: `*` is any run of characters and `?` is one. Left out, every table goes.
        #[arg(long, value_name = "PATTERN")]
        table: Vec<String>,

        /// Bring in the tables the named ones point at, rather than refusing without them.
        #[arg(long, requires = "table")]
        with_references: bool,
        /// Dump to a file first, so a copy that fails can be retried.
        ///
        /// Off by default: a copy is not a backup and leaves nothing behind. While the copy
        /// runs, the file is an unencrypted dump of the source on this machine's disk.
        #[arg(long)]
        safe: bool,
    },

    /// Merge a database into another, keeping rows the destination already had.
    #[command(after_long_help = SYNC_NOTES)]
    Sync {
        /// The registered database to copy from. Only ever read.
        source: String,

        /// The registered database to merge it into. Its rows are added to and replaced.
        #[arg(long, value_name = "NAME", conflicts_with = "create")]
        to: Option<String>,

        /// Make the destination first, then merge into it. What to call it here.
        ///
        /// The database, the user that owns it and the grants all get made — the same work
        /// `sloop db create` does, with the engine taken from the source rather than asked
        /// for. On the source's own server unless --host says otherwise.
        #[arg(long, value_name = "NAME")]
        create: Option<String>,

        #[command(flatten)]
        new: NewDestination,

        /// Only this table. Repeatable, schema-qualified, and it takes a glob.
        ///
        /// `--table orders --table audit_*` or `--table public.audit_*`. A glob, never a
        /// regex: `*` is any run of characters and `?` is one. Left out, every table goes.
        #[arg(long, value_name = "PATTERN")]
        table: Vec<String>,

        /// Bring in the tables the named ones point at, rather than refusing without them.
        #[arg(long, requires = "table")]
        with_references: bool,
        /// Dump the source to a file first, so a merge that fails can be replayed.
        ///
        /// Off by default: a copy is not a backup and leaves nothing behind. While the merge
        /// runs, the file is an unencrypted dump of the source on this machine's disk.
        #[arg(long)]
        safe: bool,
    },

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
    },

    /// Create a database and the role that owns it, then register it.
    #[command(after_long_help = CREATE_NOTES)]
    Create {
        /// What to call it here. The label you will type from now on.
        name: String,

        /// Which engine: postgres, mysql or mariadb.
        #[arg(long, value_name = "ENGINE")]
        engine: String,

        /// The server it goes on.
        #[arg(long, value_name = "HOST", default_value = "127.0.0.1")]
        host: String,

        /// Its port. The engine's own default when left out.
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,

        /// The account to create it with — `postgres` or `root` when left out.
        ///
        /// Used for this one connection and never stored, never logged, never in `ps`.
        #[arg(long, value_name = "USER")]
        superuser: Option<String>,

        /// Read that account's password from standard input.
        #[arg(long)]
        superuser_password_stdin: bool,

        /// Run this and read that account's password from its output.
        #[arg(long, value_name = "COMMAND")]
        superuser_password_command: Option<String>,

        /// The database's own name on the server. Asked for when left out.
        #[arg(long, value_name = "NAME")]
        database: Option<String>,

        /// The user to create and hand it to. Asked for when left out.
        #[arg(long, value_name = "USER")]
        role: Option<String>,

        /// Read the new user's password from standard input instead of generating one.
        #[arg(long, conflicts_with = "superuser_password_stdin")]
        role_password_stdin: bool,

        /// Run this and read the new user's password from its output.
        ///
        /// For a password manager, and for the unattended run whose standard input is
        /// already carrying the superuser's password.
        #[arg(long, value_name = "COMMAND", conflicts_with = "role_password_stdin")]
        role_password_command: Option<String>,
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
    Remove {
        /// Which one.
        name: String,
    },

    /// Drop a database on the server. Asks for its name, and keeps nothing.
    #[command(after_long_help = DROP_WARNING)]
    Drop {
        /// Which registered database to destroy.
        name: String,
    },
}

/// What `db create --help` says under the flags: where each password comes from, and
/// which of them sloop keeps.
const CREATE_NOTES: &str = "\
Two passwords are involved and they are treated completely differently.

The account you create *with* — postgres, root, an admin role — is used for one
connection and then forgotten. It is never written to the registry, never logged, and
never put in a command line where `ps` would show it. Type it at the prompt, pipe it
in with --superuser-password-stdin, or have a password manager print it with
--superuser-password-command \"op read op://vault/pg/root\".                   

The password of the user being *created* is yours to choose, and generated only when you
do not. At a terminal it is asked for, hidden, with Enter meaning \"generate one\"; a
generated one is 28 characters of letters and digits, so it pastes into any config file
without an escaping rule, and it is printed once so you can put it in your application.
--role-password-stdin pipes one in and --role-password-command runs something that prints
one — a password manager, say.

There is no flag that takes the password itself, and there will not be one: every
argument of every process on this machine is readable in `ps`, so a password that arrives
that way has already been read by anyone who wanted it.

Where that is: the OS keyring, or the Argon2id-encrypted file on a machine that has no
keyring running and has set SLOOP_PASSPHRASE. The run says which one it used.

A generated password is only ever printed at a terminal. With no terminal there is
nowhere safe to print it — a scheduled run would put it in a log — so that run has to
supply one with --role-password-stdin.

Two names exist on the server and sloop asks for both: what the database itself is
called, and which user owns it. The label you typed is offered as the default for the
first and that name for the second, so pressing Enter twice gives you the obvious thing —
but you are shown it rather than given it. --database and --role say them up front, and
with no terminal those flags are the only way to say them.

  sloop db create orders --engine postgres
      Asks what the database and its user should be called, offering `orders` for both.
      Then creates them on 127.0.0.1, hands the database to the user, grants what makes
      it usable, and registers it as `orders`.

  sloop db create orders --engine postgres --database orders_live --role orders_app
      The same, with both names given and nothing asked.

An existing user is reused and keeps the password it has. An existing database is
refused — `sloop db add` is how you register one that is already there.";

/// What `db drop --help` says under the flags, because a flag list does not convey this.
const DROP_WARNING: &str = "\
This destroys a database on the server. It is not `db remove`, which only forgets that
sloop knew about it.

It keeps nothing, and it cannot be undone. Run `sloop backup <name>` first if you want a
copy — sloop will not quietly take one for you and leave you a dump no command can read.

What happens, in order:
  1. sloop connects and confirms the database is really there.
  2. It says how many tables and rows are about to go.
  3. The name has to be typed.
  4. It ends every other connection to the database, and says how many.
  5. It drops it.

--confirm <DATABASE> is that typing done up front, for a script — there is deliberately
no flag meaning \"yes, whichever database that was\".";

/// What `backup --help` says under the flags, because the layout and the codes are the
/// two things somebody scheduling this needs and neither fits in a flag description.
const BACKUP_NOTES: &str = "\
Where it goes, under the registry that holds the database:

  backups/<engine>/<label>/<utc-timestamp>/
      dump.age        what pg_dump or mysqldump wrote, encrypted to this registry's
                      backup key — `dump`, with no extension, where there is no key
      manifest.json   the server's version, the size, the SHA-256, how long it took,
                      every table's exact row count, and when — in UTC, in local
                      time, and with the offset between them

The directory is stamped in UTC so the names sort chronologically and survive a clock
going back an hour. Everything printed is in local time.

The manifest is written last, so a directory without one is a run that died halfway
rather than a backup.

--all runs to the end whatever happens. It exits with the failures' own code when they
agree on one — 3 when servers were unreachable, 4 when dumps failed — and 1 when they
do not, which means read the output.

Two ways to write one, and both behave the same way in a crontab:

  sloop backup app                  a new directory per run, named for the moment
  sloop backup app --replace        one directory, overwritten: .../app/latest

--replace is for the operator who wants the newest copy at a path a script can name
once and keep. The new dump is written beside the old one and swapped in only when it
is finished, so a replace that is interrupted leaves the previous backup complete and
restorable.

Retention does not apply to --replace — one copy at a fixed path has no newest seven
— and `sloop backups prune` says so rather than passing over it in silence.";

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

/// The destination `mirror --create` is about to make, when it is making one.
///
/// **The same flags `db create` takes, minus `--engine`.** One vocabulary rather than two:
/// somebody who has created a database with `sloop db create` already knows every word of
/// this. The engine is missing because it is not a choice — a copy is of something, and the
/// destination is whatever the source is.
///
/// Every one of them `requires = "create"`, so passing `--role` to a plain `mirror --to`
/// is a usage error naming the flag that would have made it mean something, rather than a
/// value quietly ignored.
#[derive(Debug, clap::Args)]
pub struct NewDestination {
    /// The server to make it on. The source's own server when left out.
    #[arg(
        long,
        value_name = "HOST",
        requires = "create",
        conflicts_with = "to",
        help_heading = "Making the destination"
    )]
    pub host: Option<String>,

    /// Its port. The source's port when the host is the source's, the engine's default
    /// otherwise.
    #[arg(
        long,
        value_name = "PORT",
        requires = "create",
        conflicts_with = "to",
        help_heading = "Making the destination"
    )]
    pub port: Option<u16>,

    /// The account to create it with — `postgres` or `root` when left out.
    ///
    /// Used for this one connection and never stored, never logged, never in `ps`.
    #[arg(
        long,
        value_name = "USER",
        requires = "create",
        conflicts_with = "to",
        help_heading = "Making the destination"
    )]
    pub superuser: Option<String>,

    /// Read that account's password from standard input.
    #[arg(
        long,
        requires = "create",
        conflicts_with = "to",
        help_heading = "Making the destination"
    )]
    pub superuser_password_stdin: bool,

    /// Run this and read that account's password from its output.
    #[arg(
        long,
        value_name = "COMMAND",
        requires = "create",
        conflicts_with = "to",
        help_heading = "Making the destination"
    )]
    pub superuser_password_command: Option<String>,

    /// The new database's own name on the server. Asked for when left out.
    #[arg(
        long,
        value_name = "NAME",
        requires = "create",
        conflicts_with = "to",
        help_heading = "Making the destination"
    )]
    pub database: Option<String>,

    /// The user to create and hand it to. Asked for when left out.
    #[arg(
        long,
        value_name = "USER",
        requires = "create",
        conflicts_with = "to",
        help_heading = "Making the destination"
    )]
    pub role: Option<String>,

    /// Read the new user's password from standard input instead of generating one.
    #[arg(
        long,
        requires = "create",
        conflicts_with_all = ["to", "superuser_password_stdin"],
        help_heading = "Making the destination"
    )]
    pub role_password_stdin: bool,

    /// Run this and read the new user's password from its output.
    #[arg(
        long,
        value_name = "COMMAND",
        requires = "create",
        conflicts_with_all = ["to", "role_password_stdin"],
        help_heading = "Making the destination"
    )]
    pub role_password_command: Option<String>,
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
    #[command(after_long_help = LIST_NOTES)]
    List {
        /// Only this database's backups. Every database when left out.
        name: Option<String>,

        /// Hash every dump and compare it with its manifest.
        ///
        /// Reads every byte of every backup, so it is a check somebody asks for rather
        /// than something a listing does on the way past. Exits `6` on a mismatch.
        #[arg(long)]
        check: bool,
    },

    /// Delete backups past a retention limit, after showing which ones.
    #[command(after_long_help = PRUNE_NOTES)]
    Prune {
        /// Only this database's backups. Every database when left out.
        name: Option<String>,

        /// Keep this many of the newest backups of each database.
        #[arg(long, value_name = "N")]
        keep: Option<usize>,

        /// Remove backups older than this — `12h`, `30d`, `6w`.
        #[arg(long, value_name = "AGE")]
        older_than: Option<String>,

        /// Show what would go, and remove nothing.
        #[arg(long)]
        dry_run: bool,

        /// Remove unfinished and damaged directories too.
        ///
        /// Left alone by default: a half-written dump is all somebody has if the disk
        /// filled up mid-backup, and deciding that for them is not this command's to do.
        #[arg(long)]
        include_broken: bool,
    },
}

/// What `backups list --help` says under the flags.
const LIST_NOTES: &str = "A backup is a directory with a manifest in it. The manifest is
written last, so a directory without one is a run that did not finish — those are listed
as unfinished and never counted as something a restore could use.

Times are local, always. The directory names are UTC so that they sort.";

/// What `backups prune --help` says under the flags.
const PRUNE_NOTES: &str = "One rule at least, or both:

  sloop backups prune --keep 7                keep the newest seven of each
  sloop backups prune --older-than 30d        throw away last month
  sloop backups prune --keep 7 --older-than 30d

With both, `--keep` is a floor: a backup has to be past the newest seven *and* older than
thirty days before it goes. That is the one a crontab wants — never leave me with fewer
than seven, whatever the dates say.

Nothing is deleted before you have seen the list. `--dry-run` stops there.";

/// `sloop key …` — moving the one secret that restoring needs.
#[derive(Debug, Subcommand)]
pub enum KeyCommand {
    /// Write the private key out, so losing this machine is not losing the backups.
    #[command(after_long_help = KEY_EXPORT_NOTES)]
    Export,
    /// Take a private key exported from another machine.
    #[command(after_long_help = KEY_IMPORT_NOTES)]
    Import,
}

/// What `key export --help` says under the flags.
const KEY_EXPORT_NOTES: &str =
    "The key goes to standard output, on one line, and nothing else goes there:

  sloop key export > backup-key.txt

Everything else — the public key, and what losing this one costs — goes to standard
error, so a redirect gets a file that `age` itself would accept.

Keep it somewhere sloop cannot reach. Every encrypted backup this registry takes needs
it, there is no second copy, and no reset. If this registry has no key yet, this creates
one.";

/// What `key import --help` says under the flags.
const KEY_IMPORT_NOTES: &str = "The key is read from standard input:

  sloop key import < backup-key.txt

With a terminal it is asked for instead, and not echoed.

A registry holds one key. Importing the key it already has is fine — that is what moving
to a new machine looks like — but a *different* key is refused, because every backup
already taken can only be read with the one it was written for.";

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
                | Self::Backup { .. }
                | Self::Backups { .. }
                | Self::Restore { .. }
                | Self::Mirror { .. }
                | Self::Sync { .. }
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
            Self::Backup { .. } => "backup",
            Self::Backups { command } => command.path(),
            Self::Restore { .. } => "restore",
            Self::Mirror { .. } => "mirror",
            Self::Sync { .. } => "sync",
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
            Self::Create { .. } => "db create",
            Self::List => "db list",
            Self::Test { .. } => "db test",
            Self::Edit { .. } => "db edit",
            Self::Rename { .. } => "db rename",
            Self::Remove { .. } => "db remove",
            Self::Drop { .. } => "db drop",
        }
    }
}

impl BackupsCommand {
    #[must_use]
    pub fn path(&self) -> &'static str {
        match self {
            Self::List { .. } => "backups list",
            Self::Prune { .. } => "backups prune",
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

{consent}
  -y, --yes         Answers a question this would have stopped to ask.
      --force       Overrides a refusal that is there to protect something.
      --confirm X   The name, typed out, for something being destroyed.

  They are three different things and none stands in for another. A bare --yes can
  never destroy a named thing, so a scheduled line names what it destroys and cannot
  be repointed at something else by editing one flag. --force answers no questions.

  With no terminal to ask at, a command that needs an answer exits 2 and names the
  flag that would have given it, rather than hanging on a question nobody will read.

{codes}
  0 success   2 usage      3 connect   4 dump   5 restore
              6 mismatch   7 locked    8 doctor found a problem

{guarantee}
  cargo tree --manifest-path cli/Cargo.toml | grep -Ei 'reqwest|hyper|ureq|curl'

  Nothing comes back, and CI fails the build on the day something does.",
        menu = after_help(),
        copying = style::heading("mirror and sync are not the same command"),
        consent = style::heading("Saying yes, and what each way of saying it cannot do"),
        codes = style::heading("Exit codes, frozen at 1.0"),
        guarantee = style::heading("Check the guarantee in ten seconds"),
    )
}

/// What `mirror --help` says under the flags: what it touches, and what it does not.
const MIRROR_NOTES: &str = "\
An exact copy. Afterwards the destination holds what the source holds and nothing else:
a table that existed only in the destination is gone.

What is replaced is the destination's *contents*. The database itself, its owner and its
credentials are untouched, so an application configured against that connection string
keeps working.

The source is only ever read. Not one statement sloop sends to it changes anything,
not even to collect statistics.

Nothing is left behind. The dump goes from one client straight into the other through a
pipe and never becomes a file — a copy is not a backup. The cost is that a copy which
fails halfway has nothing to retry from, which is what --safe is for.

  sloop mirror live --to staging
  sloop mirror live --to staging --safe    # dump to a file first, deleted once verified

The destination does not have to exist. --create makes it first — the database, the role
that owns it, and the grants that make the role able to use it — and then copies into it.
The engine is not asked for: it is the source's, because a copy is of something.

  sloop mirror live --create staging
  sloop mirror live --create staging --host db2.internal --role staging_app

It asks the same two questions `sloop db create` asks — what the database itself is
called and which user owns it — with the name after --create offered for both. --database
and --role say them up front instead.

The account that creates it is used for one connection and kept nowhere. The new user's
password is generated and filed where this machine keeps secrets, exactly as
`sloop db create` does it — and the new database is registered, so the next command is
`sloop backup staging`.

--table narrows it to some of the tables, repeatable and schema-qualified, with a glob
like `audit_*`. Scoped that way a mirror no longer means the destination ends up
identical: it drops and recreates only the tables you named and leaves the rest alone,
and it says so before it starts. A table named without the tables its foreign keys point
at is refused with those named; --with-references brings them in instead.

  sloop mirror live --to staging --table audit_* --table public.sessions

Mirroring a database over itself is refused — on host, port and database together, and
localhost counts as 127.0.0.1.

Exit 6 means the copy finished and then a table did not add up. A live source that
changes while the dump runs is reported as drift rather than as a failure.";

/// What `sync --help` says under the flags: what it keeps, and what it will not do.
const SYNC_NOTES: &str =
    "A merge, not a copy. Afterwards the destination holds everything the source holds, plus
whatever it already had that the source does not:

  rows the source has and the destination does not    inserted
  rows both have                                      replaced with the source's
  rows only the destination has                       kept, and reported

  sloop sync live --to staging
  sloop sync live --to staging --safe    # dump the source first, deleted once it adds up

Tables are merged parents first, in an order worked out from their foreign keys. Two
tables whose keys point at each other have no such order, and sync refuses rather than
loading half of them.

A table with no primary key is skipped and named. There is no way to tell which row is
which without one, so a merge would insert every row again on the second run.

Sequences and auto-increment counters are reset afterwards, above the rows that are now
there — otherwise the next insert on the destination collides with a row this brought in.

The destination does not have to exist. --create makes it first — the database, the user
that owns it, and the grants that make that user able to use it — and then merges into it.
A merge into a database that was made a moment ago is a copy, and the numbers say so.

  sloop sync live --create staging

--table narrows it to some of the tables, on the same terms `mirror` uses — repeatable,
schema-qualified, a glob rather than a regex, and refused for a table whose parents are
not in the list unless --with-references brings them in. The tables that are merged keep
their order among themselves.

The source is only ever read. Not one statement sloop sends to it changes anything.

Exit 6 means the merge finished and a table did not add up: the destination has to end
with the rows it started with plus the ones that were new, and one that does not means
something else was writing to it while this ran.";

/// What `restore --help` says under the flags, because the order things happen in is the
/// thing somebody about to run this needs to know.
const RESTORE_NOTES: &str = "\
What happens, in order:
  1. The backup is read and its dump hashed against what the manifest recorded.
  2. The destination is contacted, and what it holds now is printed — so you see what is
     about to be replaced before you agree to replace it.
  3. The key is fetched, if the backup is encrypted. Before anything is cleared, so a
     restore never empties a database and then finds it cannot read the dump.
  4. The database's name has to be typed. --confirm <DATABASE> is that, up front.
  5. The destination's contents are cleared — every schema that is not PostgreSQL's own,
     or every table, view, routine and event on MySQL. The database itself is never
     dropped: its owner, its grants and its connection string are left alone.
  6. The dump is loaded, and the result is counted against the manifest table by table.

  sloop restore app                           the newest whole backup
  sloop restore app --from 20260916T031500Z   that one
  sloop restore app --from latest             the copy --replace keeps

Exit 6 means it finished and then a table did not add up — read the table above it. A
backup that fails its own checksum is refused; --force restores it anyway, which is the
right call when it is the only backup there is.";

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
