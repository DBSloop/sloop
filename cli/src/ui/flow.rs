//! The questions each command asks when it is reached from the menu instead of from a flag.
//!
//! **One question at a time, and the answers are a list.** A flow is not a function that
//! runs six prompts in a row — that would be the nested prompts `CLAUDE.md` ruled out, and
//! `← Back` in the middle of one would have nowhere to go. It is [`Answers`], which the
//! screen holds, plus [`Job::next`], which looks at what has been answered and says what to
//! ask next. Going back drops the last answer and asks it again; everything before it is
//! still there, because it was never on a call stack to begin with.
//!
//! **Which also means the questions can branch.** The plan is rebuilt from the answers every
//! time, so `db add` asks for a URL or for five fields depending on what was picked two
//! questions ago, and `mirror` asks about a destination that exists or one it is about to
//! make. Nothing has to be undone when somebody changes their mind: the answer goes, and the
//! questions that depended on it were never asked.
//!
//! **What the menu asks for, and what the command asks for.** The menu collects exactly what
//! a flag would have carried. It never asks for a password and never asks anyone to type the
//! name of a database they are destroying — those are the command's own prompts, they run
//! once the alternate screen has been handed back, and they are the same prompts a person
//! gets from the shell. One implementation of rule 3, one of rule 5.

use crate::exit::Exit;
use crate::failure::Outcome;

use super::screen::Item;

/// Everything the menu can do, which is everything `R7`–`R16` built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// `db add`
    DbAdd,
    /// `db create`
    DbCreate,
    /// `db list`
    DbList,
    /// `db test`
    DbTest,
    /// `db edit`
    DbEdit,
    /// `db rename`
    DbRename,
    /// `db remove`
    DbRemove,
    /// `db drop`
    DbDrop,
    /// `backup <name>`
    Backup,
    /// `backup --all`
    BackupAll,
    /// `backups list`
    BackupsList,
    /// `restore`
    Restore,
    /// `backups prune`
    BackupsPrune,
    /// `mirror`
    Mirror,
    /// `sync`
    Sync,
    /// `key export`
    KeyExport,
    /// `key import`
    KeyImport,
    /// `doctor`
    Doctor,
    /// `setup` — the one job that runs before there is a registry to read.
    Setup,
    /// `server install` — install a database server on this machine.
    ServerInstall,
}

/// The names answers are filed under.
///
/// Constants rather than string literals at forty call sites: a typo in one of these would
/// be an answer nobody reads and a question asked twice, and neither shows up as a failure.
pub mod field {
    /// What sloop should call a database.
    pub const NAME: &str = "name";
    /// URL, or field by field.
    pub const HOW: &str = "how";
    /// The whole connection in one string.
    pub const URL: &str = "url";
    /// postgres, mysql or mariadb.
    pub const ENGINE: &str = "engine";
    /// The server.
    pub const HOST: &str = "host";
    /// Its port.
    pub const PORT: &str = "port";
    /// The database's own name on the server.
    pub const DATABASE: &str = "database";
    /// The role to connect as.
    pub const USER: &str = "user";
    /// The account a database is created with.
    pub const SUPERUSER: &str = "superuser";
    /// Where the password is kept.
    pub const ROUTE: &str = "route";
    /// The environment variable holding it.
    pub const ENV: &str = "env";
    /// The command that prints it.
    pub const FROM_COMMAND: &str = "from-command";
    /// Connect before saving?
    pub const TEST: &str = "test";
    /// Which single detail to change.
    pub const DETAIL: &str = "detail";
    /// What to call it instead.
    pub const RENAMED: &str = "renamed";
    /// Keep every backup, or keep one.
    pub const MODE: &str = "mode";
    /// Which database, or all of them.
    pub const WHICH: &str = "which";
    /// Hash every dump?
    pub const CHECK: &str = "check";
    /// Which stored backup.
    pub const WHEN: &str = "when";
    /// How many to keep.
    pub const KEEP: &str = "keep";
    /// Delete anything older than this.
    pub const OLDER: &str = "older";
    /// Delete unfinished directories too?
    pub const BROKEN: &str = "broken";
    /// Show what would go and delete nothing?
    pub const DRY: &str = "dry";
    /// The database being copied.
    pub const SOURCE: &str = "source";
    /// An existing destination, or one to make.
    pub const WHERE: &str = "where";
    /// The registered destination.
    pub const TO: &str = "to";
    /// The destination to make.
    pub const CREATE: &str = "create";
    /// Every table, or only some.
    pub const SCOPE: &str = "scope";
    /// The table patterns.
    pub const TABLES: &str = "tables";
    /// Bring in the tables they point at?
    pub const REFERENCES: &str = "references";
    /// Dump to a file first?
    pub const SAFE: &str = "safe";
    /// Ask no server anything?
    pub const OFFLINE: &str = "offline";
    /// Reached straight, or through an SSH server?
    pub const REACH: &str = "reach";
    /// The SSH server.
    pub const SSH_HOST: &str = "ssh-host";
    /// Its port.
    pub const SSH_PORT: &str = "ssh-port";
    /// Who to be on it.
    pub const SSH_USER: &str = "ssh-user";
    /// A particular private key.
    pub const SSH_IDENTITY: &str = "ssh-identity";
    /// Where the key's passphrase comes from, if sloop has to hold one.
    pub const SSH_ROUTE: &str = "ssh-route";
    /// The environment variable holding the passphrase.
    pub const SSH_ENV: &str = "ssh-env";
    /// The command that prints the passphrase.
    pub const SSH_FROM_COMMAND: &str = "ssh-from-command";
}

/// What a yes-or-no question stores.
const YES: &str = "yes";
const NO: &str = "no";

/// What has been answered so far, in the order it was asked.
///
/// A list rather than a map, because the order is what `← Back` is made of: going back is
/// dropping the last entry, and the plan is then rebuilt without it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answers(Vec<(&'static str, String)>);

impl Answers {
    /// The raw answer, if it has been given.
    #[must_use]
    pub fn get(&self, field: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(named, _)| *named == field)
            .map(|(_, value)| value.as_str())
    }

    /// Has this been asked yet?
    #[must_use]
    pub fn asked(&self, field: &str) -> bool {
        self.get(field).is_some()
    }

    /// The answer, or the empty string.
    #[must_use]
    pub fn text(&self, field: &str) -> &str {
        self.get(field).unwrap_or_default()
    }

    /// The answer, or `None` when it was left blank — which is how every optional field is
    /// declined: Enter on an empty box means "you decide".
    #[must_use]
    pub fn some(&self, field: &str) -> Option<&str> {
        self.get(field)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// Was the answer yes?
    #[must_use]
    pub fn yes(&self, field: &str) -> bool {
        self.get(field) == Some(YES)
    }

    /// A number, or `None` when it was blank or not one.
    #[must_use]
    pub fn number<T: std::str::FromStr>(&self, field: &str) -> Option<T> {
        self.some(field).and_then(|value| value.parse().ok())
    }

    /// A blank-separated answer, as the list it stands for.
    #[must_use]
    pub fn words(&self, field: &str) -> Vec<String> {
        self.some(field)
            .map(|value| {
                value
                    .split([',', ' ', '\t'])
                    .map(str::trim)
                    .filter(|word| !word.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// File an answer. Replaces one already there, so re-asking a question is not a way to
    /// end up with two answers to it.
    pub fn put(&mut self, field: &'static str, value: impl Into<String>) {
        let value = value.into();
        if let Some(held) = self.0.iter_mut().find(|(named, _)| *named == field) {
            held.1 = value;
        } else {
            self.0.push((field, value));
        }
    }

    /// Drop the last answer. `false` when there was none, which is the signal to leave the
    /// flow rather than to stay in it.
    pub fn undo(&mut self) -> bool {
        self.0.pop().is_some()
    }
}

/// One question, and where its answer is filed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Where the answer goes.
    pub field: &'static str,
    /// What is being asked.
    pub question: String,
    /// How to ask it.
    pub how: How,
}

impl Step {
    /// The heading the list is drawn under.
    ///
    /// **Taken from the field rather than written per question**, because a heading that is
    /// invented at each of forty call sites is forty chances for two questions about the
    /// same thing to sit under two different words. The field already says what the answer
    /// is; this says it in the menu's voice.
    #[must_use]
    pub fn heading(&self) -> &'static str {
        use field as f;
        match self.field {
            f::NAME | f::WHICH | f::SOURCE => "DATABASE",
            f::RENAMED => "NEW NAME",
            f::HOW => "CONNECTION",
            f::URL => "URL",
            f::ENGINE => "ENGINE",
            f::HOST | f::PORT => "SERVER",
            f::DATABASE => "ON THE SERVER",
            f::USER => "USER",
            f::SUPERUSER => "CREATE IT AS",
            f::ROUTE | f::ENV | f::FROM_COMMAND => "PASSWORD",
            f::TEST => "BEFORE SAVING",
            f::DETAIL => "WHAT TO CHANGE",
            f::MODE => "HOW TO KEEP IT",
            f::CHECK => "HOW HARD TO LOOK",
            f::WHEN => "WHICH BACKUP",
            f::KEEP | f::OLDER | f::BROKEN | f::DRY => "WHAT TO CLEAR",
            f::WHERE | f::TO | f::CREATE => "DESTINATION",
            f::SCOPE | f::TABLES | f::REFERENCES => "HOW MUCH",
            f::SAFE => "SAFETY",
            f::OFFLINE => "HOW FAR TO LOOK",
            _ => "CHOOSE",
        }
    }
}

/// The two ways of asking. Deliberately only two: they are exactly what the shell already
/// draws, so a flow needs no new kind of screen and the scripted prompter in the tests
/// drives a flow exactly as it drives a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// Choose from a list.
    Pick {
        /// What each one is called, and what it means.
        items: Vec<Item>,
        /// What each one is filed as.
        values: Vec<String>,
    },
    /// Type it.
    Type {
        /// What is in the box before anything is typed.
        initial: String,
        /// The quiet line under it.
        help: String,
        /// Whether a blank answer is an answer.
        ///
        /// **Blank means "you decide" for an optional field and nothing at all for a
        /// required one**, and the two must not be the same keystroke. Pressing Enter on an
        /// empty port is the engine's default; pressing it on an empty name is a
        /// command run with no name, which fails at the far end of the flow with a usage
        /// error about a flag nobody passed.
        needed: bool,
    },
}

/// What the flow wants next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// This question.
    Ask(Box<Step>),
    /// Everything is answered; run it.
    Ready,
    /// It cannot be started at all, and this is why.
    Blocked(String),
}

/// What the menu needs from the world outside it, and what running a job needs.
///
/// **A trait, so `ui` never holds a registry.** That is the same reason it was plain data in
/// `R18`: a menu that borrowed the registry for the length of a session could not contain a
/// command that changes it, and `db add` changes it on the second screen.
pub trait Doing {
    /// Every registered database, nearest scope first.
    fn databases(&self) -> Vec<String>;

    /// Every stored backup of `name` that a restore could use, newest first, by the name of
    /// its own directory — which is what `restore --from` takes.
    fn backups_of(&self, name: &str) -> Vec<String>;

    /// Run it, with the terminal already handed back so the command's own output and its
    /// own prompts land where the user can see them.
    fn run(&mut self, job: Job, answers: &Answers) -> Outcome<Exit>;
}

impl Job {
    /// Does this need something in the registry before it can be started?
    const fn needs_a_database(self) -> bool {
        matches!(
            self,
            Self::DbTest
                | Self::DbEdit
                | Self::DbRename
                | Self::DbRemove
                | Self::DbDrop
                | Self::Backup
                | Self::BackupAll
                | Self::BackupsList
                | Self::Restore
                | Self::BackupsPrune
                | Self::Mirror
                | Self::Sync
        )
    }

    /// Does it copy one database into another? Both take the same questions.
    const fn is_a_copy(self) -> bool {
        matches!(self, Self::Mirror | Self::Sync)
    }

    /// The next question, or that there are none left.
    #[must_use]
    pub fn next(self, answers: &Answers, world: &dyn Doing) -> Next {
        let known = world.databases();

        if self.needs_a_database() && known.is_empty() {
            return Next::Blocked(
                "Nothing is registered here yet. Tell sloop about a database first, or make \
                 a new one — both are on the Databases screen."
                    .to_owned(),
            );
        }

        if self == Self::Restore
            && let Some(name) = answers.some(field::NAME)
            && world.backups_of(name).is_empty()
        {
            return Next::Blocked(format!(
                "There are no backups of {name} to put back. Take one first — \
                 Backups, then Back one up now."
            ));
        }

        for step in self.plan(answers, &known, world) {
            if answers.asked(step.field) {
                continue;
            }
            // **A list with nothing on it is not a question.** Every plan above is built
            // from the world, so this should be unreachable; it is here because the one
            // way this screen can be useless is the one way nobody would notice until it
            // was in front of somebody.
            if let How::Pick { values, .. } = &step.how
                && values.is_empty()
            {
                return Next::Blocked(format!(
                    "There is nothing to choose for {}. Register another database first.",
                    step.field
                ));
            }
            return Next::Ask(Box::new(step));
        }
        Next::Ready
    }

    /// Every question this job asks, given what has been answered so far.
    ///
    /// Rebuilt each time rather than walked through once, which is what makes going back
    /// free: drop an answer and the questions that hung off it were never asked.
    fn plan(self, answers: &Answers, known: &[String], world: &dyn Doing) -> Vec<Step> {
        match self {
            // Setup asks nothing here. The one question it can have — the superuser
            // password of a PostgreSQL it did not install — belongs to the command itself,
            // asked on the real terminal, because that is the only place rule 3 has one
            // implementation.
            // `server install` asks nothing here either, and for a reason of its own:
            // resolving which versions exist means reading an index, reading an index means
            // `curl`, and `curl` writes its progress to a terminal the menu is holding. So
            // engine, version and the confirmation are all the command's own questions, on
            // the terminal it has been handed — which is also what lets the engine list show
            // MongoDB and SQL Server as rows nobody can choose, the way `R19d` asked.
            Self::Setup
            | Self::ServerInstall
            | Self::DbList
            | Self::KeyExport
            | Self::KeyImport => Vec::new(),

            Self::DbAdd => registering(answers),
            Self::DbCreate => making(),
            Self::DbTest => vec![one_or_all("Which one should sloop try?", known)],
            Self::DbEdit => editing(answers, known),

            Self::DbRename => vec![
                pick_one("Which one should be renamed?", known),
                typed(
                    field::RENAMED,
                    "What should sloop call it instead?",
                    "",
                    "the label only. The database's own name on the server does not change",
                ),
            ],

            Self::DbRemove => vec![pick_one(
                "Which one should sloop forget? The database itself is untouched.",
                known,
            )],

            Self::DbDrop => vec![pick_one(
                "Which database should be deleted from the server?",
                known,
            )],

            Self::Backup => vec![
                pick_one("Which one should sloop back up?", known),
                keeping(),
            ],
            Self::BackupAll => vec![keeping()],

            Self::BackupsList => vec![
                one_or_all("Whose backups?", known),
                yes_or_no(
                    field::CHECK,
                    "Hash every dump and check it against its manifest?",
                    false,
                    "reads every byte of every backup, so it takes as long as the backups are big",
                ),
            ],

            Self::Restore => vec![
                pick_one("Which database should be put back?", known),
                which_backup(answers, world),
            ],

            Self::BackupsPrune => vec![
                one_or_all("Whose backups should be cleared out?", known),
                box_for(
                    field::KEEP,
                    "How many of the newest should be kept?",
                    "7",
                    "blank to keep them all and go by age alone",
                    false,
                ),
                optional(
                    field::OLDER,
                    "Delete anything older than?",
                    "12h, 30d, 6w. Blank to go by count alone",
                ),
                yes_or_no(
                    field::BROKEN,
                    "Delete unfinished and damaged ones too?",
                    false,
                    "a half-written dump is all somebody has if the disk filled up mid-backup",
                ),
                yes_or_no(
                    field::DRY,
                    "Show what would go and delete nothing?",
                    true,
                    "run it again and answer no when the list looks right",
                ),
            ],

            Self::Mirror | Self::Sync => copying(self, answers, known),

            Self::Doctor => vec![yes_or_no(
                field::OFFLINE,
                "Ask the servers what each role may do?",
                true,
                "no means the client tools only, and nothing is connected to",
            )],
        }
    }

    /// What has been answered so far, as a label and a value each.
    ///
    /// An associated function rather than a method: the labels are the fields' own, so the
    /// same answers read back the same way whichever job collected them — and that is the
    /// property worth having, not a per-job summary that could disagree with itself.
    ///
    /// **The screen reads back what it was told.** A flow six questions long is otherwise
    /// six screens with no memory of each other, and somebody four questions in has no way
    /// to check what they said on the first without leaving and starting again. Only the
    /// answers that have actually been given appear, so the list grows as the flow does.
    #[must_use]
    pub fn so_far(answers: &Answers) -> Vec<(&'static str, String)> {
        use field as f;

        // Label by label, in the order they are asked, because a summary that reorders
        // itself as it fills in is a summary nobody can glance at.
        let labels: &[(&str, &'static str)] = &[
            (f::NAME, "name"),
            (f::SOURCE, "source"),
            (f::WHICH, "database"),
            (f::TO, "into"),
            (f::CREATE, "making"),
            (f::RENAMED, "renamed"),
            (f::DETAIL, "changing"),
            (f::URL, "url"),
            (f::ENGINE, "engine"),
            (f::HOST, "host"),
            (f::PORT, "port"),
            (f::DATABASE, "database"),
            (f::USER, "user"),
            (f::SUPERUSER, "as"),
            (f::ROUTE, "password"),
            (f::ENV, "variable"),
            (f::FROM_COMMAND, "from"),
            (f::MODE, "keeping"),
            (f::WHEN, "backup"),
            (f::KEEP, "keep"),
            (f::OLDER, "older than"),
            (f::TABLES, "tables"),
        ];

        let mut said = Vec::new();
        for (field, label) in labels {
            let Some(value) = answers.get(field) else {
                continue;
            };
            // A field answered with a blank is a field the user declined, and every one of
            // them means the same thing.
            let value = if value.trim().is_empty() {
                match *field {
                    f::WHICH => "every one of them".to_owned(),
                    f::WHEN => "the newest".to_owned(),
                    _ => "sloop decides".to_owned(),
                }
            } else {
                value.to_owned()
            };
            said.push((*label, value));
        }
        said
    }
}

/// `db add`: a URL, or five fields, and then where the password lives.
fn registering(answers: &Answers) -> Vec<Step> {
    let mut plan = vec![
        typed(
            field::NAME,
            "What should sloop call it?",
            "",
            "the label you will type from now on, not the database's own name",
        ),
        choose(
            field::HOW,
            "How would you like to give the connection?",
            &[
                (
                    "One line",
                    "a URL: postgres://app@db.internal:5432/orders",
                    "url",
                ),
                (
                    "Field by field",
                    "engine, host, port, database, user",
                    "fields",
                ),
            ],
        ),
    ];

    if answers.text(field::HOW) == "url" {
        plan.push(typed(
            field::URL,
            "The connection URL?",
            "postgres://user@host:5432/database",
            "a password in it is taken out and filed properly, never left in the registry",
        ));
    } else {
        plan.extend([
            engines(),
            typed(field::HOST, "Which server?", "127.0.0.1", ""),
            optional(
                field::PORT,
                "Which port?",
                "blank for the engine's own default",
            ),
            typed(
                field::DATABASE,
                "What is the database called on the server?",
                "",
                "",
            ),
            typed(field::USER, "Which user should sloop connect as?", "", ""),
        ]);
    }

    plan.extend(password_route(answers));
    plan.extend(reaching(answers));
    plan.push(yes_or_no(
        field::TEST,
        "Try the connection before saving it?",
        true,
        "a registration that cannot connect is one you find out about at 3am otherwise",
    ));
    plan
}

/// How the database is reached: straight at it, or through an SSH server — `R19e`.
///
/// **Asked last, after the connection**, because the answer changes what the host above
/// meant, and a question that rewrites an earlier answer is a question to ask afterwards.
/// The screen says so in as many words: over SSH the host is the address the *server* sees,
/// which is the one thing everybody gets backwards once.
fn reaching(answers: &Answers) -> Vec<Step> {
    let mut plan = vec![choose(
        field::REACH,
        "How does this machine reach it?",
        &[
            ("Straight at it", "the usual answer", "direct"),
            (
                "Through an SSH server",
                "for a database whose port is closed to everything outside its server",
                "ssh",
            ),
        ],
    )];

    if answers.text(field::REACH) != "ssh" {
        return plan;
    }

    plan.extend([
        typed(
            field::SSH_HOST,
            "Which SSH server?",
            "",
            "the machine sloop logs in to. The host above is the address THAT machine \
             sees, which is usually 127.0.0.1",
        ),
        optional(field::SSH_PORT, "Which SSH port?", "blank for 22"),
        optional(
            field::SSH_USER,
            "Who should sloop be on it?",
            "blank to leave it to ~/.ssh/config and then your own username",
        ),
        optional(
            field::SSH_IDENTITY,
            "Which private key?",
            "blank for the agent and ~/.ssh/config, which is the usual answer",
        ),
        choose(
            field::SSH_ROUTE,
            "Does that key need a passphrase sloop has to supply?",
            &[
                (
                    "No — an agent holds it, or it has none",
                    "sloop never sees a secret at all. The usual answer",
                    "agent",
                ),
                (
                    "Keep it in this machine's keyring",
                    "Credential Manager, Keychain or Secret Service",
                    "keyring",
                ),
                (
                    "Keep it in an encrypted file",
                    "Argon2id, for a machine with no keyring running",
                    "file",
                ),
                (
                    "Read it from an environment variable",
                    "nothing is stored; it is read on every run",
                    "env",
                ),
                (
                    "Run something that prints it",
                    "a password manager: op read op://vault/ssh/passphrase",
                    "command",
                ),
            ],
        ),
    ]);

    match answers.text(field::SSH_ROUTE) {
        "env" => plan.push(typed(
            field::SSH_ENV,
            "Which environment variable holds the passphrase?",
            "",
            "the name only, without the $",
        )),
        "command" => plan.push(typed(
            field::SSH_FROM_COMMAND,
            "Which command prints the passphrase?",
            "",
            "run on every connection. Its output is taken as the passphrase",
        )),
        _ => {}
    }

    plan
}

/// `db create`: everything a flag would have carried. The two names on the server and both
/// passwords are the command's own questions, asked once the screen is handed back.
fn making() -> Vec<Step> {
    vec![
        typed(
            field::NAME,
            "What should sloop call it?",
            "",
            "the label you will type from now on",
        ),
        engines(),
        typed(
            field::HOST,
            "Which server should it go on?",
            "127.0.0.1",
            "",
        ),
        optional(
            field::PORT,
            "Which port?",
            "blank for the engine's own default",
        ),
        optional(
            field::SUPERUSER,
            "Which account should sloop create it with?",
            "blank for the engine's usual one — postgres, or root. Its password is asked for \
             next and never stored",
        ),
    ]
}

/// `db edit`: one detail at a time. Changing two things is running it twice, which is a
/// smaller thing to explain than a screen full of boxes that are mostly already right.
fn editing(answers: &Answers, known: &[String]) -> Vec<Step> {
    let mut plan = vec![
        pick_one("Which one should be changed?", known),
        choose(
            field::DETAIL,
            "What would you like to change?",
            &[
                ("The server", "host and port", "host"),
                ("The database", "its own name on the server", "database"),
                ("The user", "the role sloop connects as", "user"),
                ("The password", "and where it is kept", "password"),
                ("The engine", "postgres, mysql or mariadb", "engine"),
                (
                    "How it is reached",
                    "straight at it, or through an SSH server",
                    "reach",
                ),
            ],
        ),
    ];

    match answers.text(field::DETAIL) {
        "host" => plan.extend([
            typed(field::HOST, "Which server?", "", ""),
            optional(field::PORT, "Which port?", "blank to leave the port alone"),
        ]),
        "database" => plan.push(typed(
            field::DATABASE,
            "What is it called on the server?",
            "",
            "",
        )),
        "user" => plan.push(typed(
            field::USER,
            "Which user should sloop connect as?",
            "",
            "",
        )),
        "engine" => plan.push(engines()),
        "password" => plan.extend(password_route(answers)),
        "reach" => plan.extend(reaching(answers)),
        _ => {}
    }

    plan.push(yes_or_no(
        field::TEST,
        "Try the connection before saving the change?",
        true,
        "",
    ));
    plan
}

/// `mirror` and `sync` ask the same questions; only the word for what they do differs.
fn copying(job: Job, answers: &Answers, known: &[String]) -> Vec<Step> {
    debug_assert!(job.is_a_copy());
    let verb = if job == Job::Mirror {
        "copied"
    } else {
        "merged"
    };

    let source = answers.some(field::SOURCE);
    let destinations: Vec<String> = known
        .iter()
        .filter(|name| Some(name.as_str()) != source)
        .cloned()
        .collect();

    let mut plan = vec![pick(
        field::SOURCE,
        &format!("Which database should be {verb}? It is only ever read."),
        known,
    )];
    plan.extend(destination(job, answers, &destinations));
    plan.extend(how_much(answers));
    plan.push(yes_or_no(
        field::SAFE,
        "Dump the source to a file first?",
        false,
        "it can be retried if the copy fails, but while it runs the file is an unencrypted          dump on this disk",
    ));
    plan
}

/// Where a copy is going: one that is already registered, or one about to be made.
fn destination(job: Job, answers: &Answers, destinations: &[String]) -> Vec<Step> {
    // **What is offered depends on what is there.** With one database registered there is
    // no other one to copy into, and a list holding nothing is a worse answer than a
    // question that never mentions it.
    let known_one = (
        "A database sloop knows",
        "one that is already registered",
        "known",
    );
    let a_new_one = (
        "A new one",
        "sloop makes the database, its user and the grants first",
        "new",
    );
    let only_a_new_one = (
        "A new one",
        "sloop makes the database, its user and the grants first. Nothing else is          registered here to copy into",
        "new",
    );

    let mut plan = vec![if destinations.is_empty() {
        choose(field::WHERE, "Where should it go?", &[only_a_new_one])
    } else {
        choose(field::WHERE, "Where should it go?", &[known_one, a_new_one])
    }];

    if answers.text(field::WHERE) == "known" {
        plan.push(pick(
            field::TO,
            if job == Job::Mirror {
                "Which one should be replaced?"
            } else {
                "Which one should it be merged into?"
            },
            destinations,
        ));
        return plan;
    }

    plan.extend([
        typed(
            field::CREATE,
            "What should sloop call the new one?",
            "",
            "the label you will type from now on",
        ),
        optional(
            field::HOST,
            "Which server should it go on?",
            "blank for the source's own server",
        ),
        optional(
            field::PORT,
            "Which port?",
            "blank for the source's port, or the engine's default",
        ),
        optional(
            field::SUPERUSER,
            "Which account should sloop create it with?",
            "blank for the engine's usual one. Its password is asked for next and never stored",
        ),
    ]);
    plan
}

/// Every table, or only the ones named.
fn how_much(answers: &Answers) -> Vec<Step> {
    let mut plan = vec![choose(
        field::SCOPE,
        "How much of it?",
        &[
            ("Everything", "every table in the database", "all"),
            ("Only some tables", "named, or matched with * and ?", "some"),
        ],
    )];

    if answers.text(field::SCOPE) == "some" {
        plan.extend([
            typed(
                field::TABLES,
                "Which tables?",
                "",
                "separated by spaces or commas. `orders`, `public.audit_*`, `log_?`",
            ),
            yes_or_no(
                field::REFERENCES,
                "Bring in the tables those ones point at?",
                true,
                "no means sloop refuses rather than copying a table whose foreign keys have                  nowhere to land",
            ),
        ]);
    }
    plan
}

/// Where a password comes from, and the one extra question two of the four routes need.
fn password_route(answers: &Answers) -> Vec<Step> {
    let mut plan = vec![choose(
        field::ROUTE,
        "Where should the password be kept?",
        &[
            (
                "This machine's keyring",
                "Credential Manager, Keychain or Secret Service. The usual answer",
                "keyring",
            ),
            (
                "An encrypted file",
                "Argon2id, for a machine with no keyring running",
                "file",
            ),
            (
                "An environment variable",
                "nothing is stored; it is read on every run",
                "env",
            ),
            (
                "Something that prints it",
                "a password manager: op read op://vault/db/password",
                "command",
            ),
        ],
    )];

    match answers.text(field::ROUTE) {
        "env" => plan.push(typed(
            field::ENV,
            "Which environment variable holds it?",
            "",
            "the name only, without the $",
        )),
        "command" => plan.push(typed(
            field::FROM_COMMAND,
            "Which command prints it?",
            "",
            "run on every connection. Its output is taken as the password, newline removed",
        )),
        _ => {}
    }
    plan
}

/// Which stored backup to put back.
fn which_backup(answers: &Answers, world: &dyn Doing) -> Step {
    let mut items = vec![Item::new(
        "The newest one",
        "the most recent backup that is whole",
    )];
    let mut values = vec![String::new()];

    for taken in world.backups_of(answers.text(field::NAME)) {
        items.push(Item::new(&taken, ""));
        values.push(taken);
    }

    Step {
        field: field::WHEN,
        question: "Which backup?".to_owned(),
        how: How::Pick { items, values },
    }
}

/// Keep every backup, or keep one.
fn keeping() -> Step {
    choose(
        field::MODE,
        "How should it be kept?",
        &[
            (
                "Keep every backup",
                "a directory named for the moment it was taken. The default",
                "sequential",
            ),
            (
                "Keep only the newest",
                "one backup per database, replaced each time",
                "replace",
            ),
        ],
    )
}

/// The three engines, every time they are asked for.
fn engines() -> Step {
    choose(
        field::ENGINE,
        "Which engine?",
        &[
            ("PostgreSQL", "pg_dump and pg_restore", "postgres"),
            ("MySQL", "mysqldump and the mysql client", "mysql"),
            ("MariaDB", "mariadb-dump, which is not mysqldump", "mariadb"),
        ],
    )
}

/// A list of registered databases.
fn pick_one(question: &str, known: &[String]) -> Step {
    pick(field::NAME, question, known)
}

/// A list of registered databases, with every one of them at the top.
fn one_or_all(question: &str, known: &[String]) -> Step {
    let mut items = vec![Item::new("Every one of them", "")];
    let mut values = vec![String::new()];

    for name in known {
        items.push(Item::new(name, ""));
        values.push(name.clone());
    }

    Step {
        field: field::WHICH,
        question: question.to_owned(),
        how: How::Pick { items, values },
    }
}

/// A list of names, filed under `field`.
fn pick(field: &'static str, question: &str, names: &[String]) -> Step {
    Step {
        field,
        question: question.to_owned(),
        how: How::Pick {
            items: names.iter().map(|name| Item::new(name, "")).collect(),
            values: names.to_vec(),
        },
    }
}

/// A list of choices, each with a phrase and the value it stands for.
fn choose(field: &'static str, question: &str, options: &[(&str, &str, &str)]) -> Step {
    Step {
        field,
        question: question.to_owned(),
        how: How::Pick {
            items: options
                .iter()
                .map(|(title, blurb, _)| Item::new(title, blurb))
                .collect(),
            values: options
                .iter()
                .map(|(_, _, value)| (*value).to_owned())
                .collect(),
        },
    }
}

/// Yes or no, with the likelier answer first so Enter is usually right.
fn yes_or_no(field: &'static str, question: &str, default_yes: bool, blurb: &str) -> Step {
    let yes = ("Yes", blurb, YES);
    let no = ("No", "", NO);
    let options = if default_yes { [yes, no] } else { [no, yes] };
    choose(field, question, &options)
}

/// Something to type, which has to be typed.
fn typed(field: &'static str, question: &str, initial: &str, help: &str) -> Step {
    box_for(field, question, initial, help, true)
}

/// Something to type, where leaving it blank is an answer.
fn optional(field: &'static str, question: &str, help: &str) -> Step {
    box_for(field, question, "", help, false)
}

fn box_for(field: &'static str, question: &str, initial: &str, help: &str, needed: bool) -> Step {
    Step {
        field,
        question: question.to_owned(),
        how: How::Type {
            initial: initial.to_owned(),
            help: if help.is_empty() {
                "Enter accepts it. Esc goes back a question.".to_owned()
            } else {
                format!("{help}. Esc goes back a question.")
            },
            needed,
        },
    }
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod tests;
