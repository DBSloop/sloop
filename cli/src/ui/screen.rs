//! Which screen you are on, what is drawn on it, and what choosing a thing on it does.
//!
//! **A screen is a value, not a function call.** That is the whole design, and it is what
//! `← Back` is built out of: the render loop keeps a `Vec<Screen>` behind the one it is
//! showing, going forward pushes the current screen onto it *as it stood*, and going back
//! pops it out again. Nothing is recomputed on the way back, so nothing can be lost on the
//! way back — the highlight is where you left it and the text you typed is still typed.
//!
//! Nested prompts would have given the same tree and none of that. A prompt that calls a
//! prompt has no way to return anywhere but forward, which is why `CLAUDE.md` asked for a
//! stack instead.
//!
//! **Every leaf opens a flow.** The tree is the same tree `R18` drew; what `R19` changed is
//! what happens at the end of a branch. A leaf is now a [`Job`], the screen behind it holds
//! that job's [`Answers`], and `← Back` inside one drops the last answer rather than leaving
//! the whole flow — which is the same rule as everywhere else, one question deep instead of
//! one screen deep. See `flow`.

use std::path::{Path, PathBuf};

use crate::commands::init::Initialised;
use crate::failure::{Failure, Outcome};

use super::flow::{Answers, Doing, How, Job, Next, Step};
use super::live::Told;
use super::paint::{Banner, Header, Line};
use crate::exit::Exit;
use crate::mark::Mark;
use crate::style::Hue;

/// Where the render loop goes after a screen has been answered.
#[derive(Debug)]
pub enum Flow {
    /// Forward. The screen being left is pushed onto the history exactly as it stands.
    To(Screen),
    /// Back one. At the root there is nothing behind, so this leaves.
    Back,
    /// All the way out to the menu, forgetting the way in.
    Home,
    /// Draw this screen again — something typed into it did not work.
    Stay,
    /// Replace this screen without pushing it.
    ///
    /// **For a flow, which keeps its own history.** Every answer makes a new screen, and
    /// pushing each one would put the same move on the stack twice: `← Back` would drop an
    /// answer, and the next one would walk into the copy of the screen that still had it.
    /// One undo, in the flow's own terms. See [`Screen::stepped_back`].
    Same(Screen),
    /// Hand the terminal back, run this, and come back to the menu.
    Run(Leaf, Box<Answers>),
    /// Put the terminal back.
    Quit,
}

/// What the doors on the home screen say about the world behind them.
///
/// **Worked out once and kept, never per keystroke.** The home screen is redrawn on every
/// arrow key, and a status that scanned the backup store or asked the service control
/// manager each time would be a directory walk and a system call per keypress. It is taken
/// when the session opens and again after every job, which is exactly when it can have
/// changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Standing {
    /// How many databases this registry holds.
    pub databases: usize,
    /// How many backups are stored, and how long ago the newest was taken.
    pub backups: Option<(usize, String)>,
    /// What the background service is doing, and whether that is a good thing.
    pub service: Option<(String, bool)>,
}

/// Everything a screen needs to know about the world outside it.
///
/// Plain owned data on purpose. A screen that held a `&Registries` would make the whole
/// screen stack borrow the registry for the length of the session, and `init` — which is
/// the one thing in this shell that changes the world — could then never be run from
/// inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// The global store.
    pub global: PathBuf,
    /// The user's home directory: the line the walk up the tree stops at, and so the one
    /// directory `Init` cannot be pointed at.
    pub home: PathBuf,
    /// Where `sloop` was run.
    pub cwd: PathBuf,
    /// The registry this session is reading.
    pub working_in: String,
    /// Why that one and not the other, in a phrase.
    pub found_by: String,
    /// What the doors say, as of the last time anything could have changed it.
    pub standing: Standing,
    /// True when there is nothing registered anywhere and no project above the cwd.
    pub fresh: bool,
    /// Whether this machine has been through Setup.
    ///
    /// **The one thing that outranks every other opening screen.** `R19c` made sloop keep its
    /// state in a PostgreSQL of its own, so a machine without one has no registry to read and
    /// nothing else in this tree can do anything at all.
    pub set_up: bool,
}

impl Shell {
    /// The screen a session opens on.
    ///
    /// **Setup first, and it is not a choice.** `R19c` moved sloop's state into a PostgreSQL
    /// of its own, so a machine that has not been set up has no registry to read — every
    /// other screen in this tree would be a menu whose every item fails. Setup replaces
    /// `Init` as the first screen of such a machine, which is `R19c5`'s whole scope and
    /// supersedes `R18`'s *"first run offers `Init` and nothing else"*.
    ///
    /// **And a machine that is set up is never shown it.** The owner's rule, in one branch:
    /// the record in `~/.sloop` is the answer, and a machine that has one has already
    /// answered.
    #[must_use]
    pub const fn opening(&self) -> Screen {
        if !self.set_up {
            Screen::Setup {
                cursor: 0,
                trouble: None,
            }
        } else if self.fresh {
            Screen::FirstRun { cursor: 0 }
        } else {
            Screen::Home { cursor: 0 }
        }
    }

    /// Take the registry that `init` just made as this session's own.
    fn moved_into(&mut self, registry: &Path) {
        self.working_in = registry.display().to_string();
        "the project just started here".clone_into(&mut self.found_by);
        self.fresh = false;
    }
}

/// One screen, holding everything that would otherwise be lost by leaving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// This machine has not been set up, and nothing else can happen until it has.
    Setup {
        /// Where the highlight is.
        cursor: usize,
        /// Why the last attempt did not finish. Setup is re-runnable, so this is a state to
        /// come back from rather than an end.
        trouble: Option<String>,
    },
    /// Nothing is registered anywhere yet.
    FirstRun {
        /// Where the highlight is.
        cursor: usize,
    },
    /// Where a new project registry should go. The one thing typed in this shell.
    NewProject {
        /// The directory, as it currently reads in the box.
        at: String,
        /// Why the last attempt did not take.
        trouble: Option<String>,
    },
    /// What `init` just did.
    Started {
        /// The registry it made, for the summary printed after the screen is given back.
        report: Box<Initialised>,
        /// Where the highlight is.
        cursor: usize,
    },
    /// The wordmark, and the six doors.
    Home {
        /// Where the highlight is.
        cursor: usize,
    },
    /// What is behind one door.
    Door {
        /// Which one, indexing [`DOORS`].
        door: usize,
        /// Where the highlight is.
        cursor: usize,
    },
    /// A job that finished, and what it did.
    ///
    /// **The screen the user actually reads.** Nothing drawn inside the alternate buffer
    /// survives it, so the live screen a job ran on is gone the moment it returns — this is
    /// what is left, and it holds the whole transcript rather than a summary of it.
    Done {
        /// Which command it was.
        leaf: Leaf,
        /// Every step it took and every line it printed.
        told: Vec<Told>,
        /// The flag form of what just ran, so a session done by hand becomes a line somebody
        /// can schedule. `R20`, moved onto the screen now that there is no scrollback to
        /// print it into.
        same: Option<String>,
        /// How long it took.
        took: String,
        /// Where the highlight is.
        cursor: usize,
    },
    /// A job that did not work, and the ways out of it.
    ///
    /// **The owner's rule 9.** A failure used to leave one sentence on the flow screen and
    /// no way forward but running the same answers again. This keeps the answers, says what
    /// happened, and offers the three things somebody actually wants: try it again, change
    /// what you said, or go back.
    Failed {
        /// Which command it was.
        leaf: Leaf,
        /// What was answered, so that trying again does not mean typing it again.
        answers: Box<Answers>,
        /// Every step it took, up to the one that did not.
        told: Vec<Told>,
        /// What went wrong, in one sentence.
        said: String,
        /// What to do about it, when the failure named something.
        hint: Option<String>,
        /// The code it would have exited with.
        exit: Exit,
        /// Where the highlight is.
        cursor: usize,
    },
    /// A command, part-way through the questions it asks.
    ///
    /// **The answers live here rather than in the loop**, so the screen stack keeps them
    /// exactly as it keeps a highlight: leave the flow and come back to it and every answer
    /// is still given. `← Back` inside one drops the last answer instead of the whole flow.
    Doing {
        /// Which command.
        leaf: Leaf,
        /// What has been answered so far.
        answers: Box<Answers>,
        /// Where the highlight is on the question being asked.
        cursor: usize,
        /// What the last attempt to run it said.
        trouble: Option<String>,
    },
}

/// What a screen looks like to the render loop.
pub enum Face {
    /// A list to pick from.
    Menu(Menu),
    /// One line to type.
    Ask(Ask),
}

/// A list of things to pick from. The way out is added by the loop, never by a screen —
/// which is how *"`← Back` on every menu"* stays true of a menu nobody has looked at yet.
///
/// **No headings, and none of the structure that carried them.** The owner's rule 2:
/// *"headings are looking bad needs to remove though i asked to add"*. What the headings were
/// for — thirty commands on one screen needing somewhere to belong — is answered by the
/// doors instead, so a list is now a list. See `docs/OWNER-DECISIONS.md`, "Headings out,
/// doors in".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// The line above the list.
    pub question: String,
    /// What is on it, in the order it is drawn.
    pub items: Vec<Item>,
}

impl Menu {
    /// A list, and the line above it.
    #[must_use]
    pub fn of(question: &str, items: Vec<Item>) -> Self {
        Self {
            question: question.to_owned(),
            items,
        }
    }
}

/// One line in a list: what it is called, what it is doing, and the command that does it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// What it is called.
    pub title: String,
    /// The middle column: what it does, or what it is currently doing.
    pub note: String,
    /// Which colour that carries. Green for a thing that is working, amber for one that
    /// wants attention, grey for a phrase that is only a description.
    pub hue: Hue,
    /// The flag form that does the same thing, for the items that are commands.
    ///
    /// **The owner asked for it beside every one of them** — *"each task show the command
    /// that how it can be done by command"*, and again in rule 11, *"showing command in right
    /// side seems good, don't remove"*. It is reference and never an instruction: the menu
    /// does the whole job, and this is how the same job is written into a crontab.
    pub command: String,
    /// Words the filter matches on that are not drawn anywhere.
    ///
    /// **This is what makes the doors safe.** A front page of six lines was tried before and
    /// reopened, because `mirror` was not written on it and so could not be found by eye or
    /// by typing. The door that holds `mirror` carries the word here, so typing it still
    /// lands on the right row.
    pub finds: String,
}

impl Item {
    /// Something to pick, with a phrase saying what it means.
    #[must_use]
    pub fn new(title: &str, note: &str) -> Self {
        Self {
            title: title.to_owned(),
            note: note.to_owned(),
            hue: Hue::Dim,
            command: String::new(),
            finds: String::new(),
        }
    }

    /// A command, with the flag form beside it.
    #[must_use]
    pub fn doing(leaf: Leaf) -> Self {
        Self {
            title: leaf.title.to_owned(),
            note: leaf.blurb.to_owned(),
            hue: Hue::Dim,
            command: leaf.command.to_owned(),
            finds: String::new(),
        }
    }

    /// A door, with what is behind it said in the colour it deserves.
    #[must_use]
    fn door(door: &Door, note: String, hue: Hue) -> Self {
        Self {
            title: door.title.to_owned(),
            note,
            hue,
            command: door.command.to_owned(),
            finds: door.finds.to_owned(),
        }
    }

    /// Everything the filter is allowed to match on.
    #[must_use]
    pub fn matches(&self, looking_for: &str) -> bool {
        looking_for.is_empty()
            || self.title.to_lowercase().contains(looking_for)
            || self.command.to_lowercase().contains(looking_for)
            || self.finds.to_lowercase().contains(looking_for)
    }
}

/// One line to type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The line above the box.
    pub question: String,
    /// What is in the box before anything is typed.
    pub initial: String,
    /// The quiet line under it.
    pub help: String,
}

/// A command, as a menu item: what it is called, what it does, and the flag form of it.
///
/// **The flag form is reference, never an instruction.** The menu is the tool; a flag is
/// how the same job is written into a crontab, and `R20` prints exactly this line after an
/// interactive run so a session someone has just done by hand becomes a line they can
/// schedule. Until `R19` wires the prompt sequence behind each of these, the screen says
/// plainly that it is being built rather than sending anybody to the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leaf {
    /// What the menu calls it.
    pub title: &'static str,
    /// What it does, in one phrase.
    pub blurb: &'static str,
    /// The flag form, ready to paste.
    pub command: &'static str,
    /// What choosing it runs.
    pub job: Job,
    /// Which group it sits under, for the breadcrumb.
    pub under: &'static str,
    /// The one item on the last screen of the flow: the sentence that does it.
    ///
    /// Written per leaf rather than built from the title, because *"Delete one from the
    /// server"* has to become *"Delete it"* and not *"Delete one from the server it"* — and
    /// the last thing somebody reads before a database stops existing is worth writing by
    /// hand.
    pub run_it: &'static str,
}

/// **Every command sloop has, on one screen, under the heading it belongs to.**
///
/// It used to be five doors — `Databases`, `Backups`, `Copy a database`, `Backup key`,
/// `Check my setup` — with the commands a screen behind them. The owner reopened that:
/// a front page of five lines reads as a tool with five features, and neither the eye nor
/// the type-to-filter could find `mirror`, because the word was not on it. So the doors are
/// gone and the groups are headings.
///
/// **It stays a table on purpose.** Restructuring this menu again is editing the rows below,
/// not touching the loop that draws them — which is what makes the next reshuffle cheap. See
/// "The home screen lists every command" in `docs/OWNER-DECISIONS.md`.
///
/// Registering comes first because you cannot back up a database sloop has never heard of;
/// copying comes after both because it needs two.
/// What sits above the Setup screen's one item.
///
/// Its own function because [`Screen::header`] is a table of screens and this is the longest
/// entry in it — and because the failed case is the interesting one: Setup is re-runnable, so
/// what went wrong is shown *with* the reason it is safe to try again.
fn setup_header(trouble: Option<&str>) -> Header {
    let mut lines = vec![
        Line::told("machine", "not set up yet", Hue::Warn),
        Line::Gap,
        Line::Quiet(
            "sloop keeps what it knows in a PostgreSQL 18 of its own — on port 5433, so \
             whatever this machine already runs is left exactly as it is."
                .to_owned(),
        ),
    ];

    if let Some(trouble) = trouble {
        lines.push(Line::Gap);
        lines.push(Line::Wrong(trouble.to_owned()));
        lines.push(Line::Quiet(
            "Setup asks before every step it takes, so running it again carries on from \
             where it stopped."
                .to_owned(),
        ));
    }

    Header {
        banner: Banner::Wordmark,
        crumbs: Vec::new(),
        strap: "register a database once, then back it up and copy it".to_owned(),
        lines,
    }
}

/// The one leaf that is not on the home screen.
///
/// **Setup is not a command somebody browses to.** It is the whole of the first screen of a
/// machine that has not been set up, and it is gone from every screen afterwards — so it has
/// a `Leaf` for what `Flow::Run` needs and no place in [`HOME`]. Putting it on the menu would
/// be offering to set up a machine that is already set up, which is the one thing the owner
/// said must never happen.
const SETUP: Leaf = Leaf {
    title: "Set up this machine",
    blurb: "finds PostgreSQL 18 or installs it, then makes sloop's database and its tables",
    command: "sloop setup",
    job: Job::Setup,
    under: "This machine",
    run_it: "Set it up",
};

/// One door on the home screen, and the commands behind it.
///
/// **Six of them, which is the owner's rule 12** — *"home screen shows a lot of items, which
/// can go inside sub menus"*. Thirty commands under seven headings was a list nobody read to
/// the end of; six doors is a front page somebody can take in at a glance, and the commands
/// are one keystroke behind whichever one they belong to.
///
/// **And the objection that removed the doors last time is answered by [`Door::finds`].** A
/// front page of six lines was tried before and reopened, because the word `mirror` was not
/// on it and neither the eye nor the type-to-filter could find it. The filter now matches
/// every command name behind a door as well as the door's own title, so typing `mirror` on
/// the home screen still lands on the one that does it. See "Headings out, doors in" in
/// `docs/OWNER-DECISIONS.md`.
struct Door {
    /// What the door is called.
    title: &'static str,
    /// The family's flag form, for the right-hand column.
    command: &'static str,
    /// The crumb for the screen behind it.
    crumb: &'static str,
    /// The line above the list behind it.
    question: &'static str,
    /// Every word the filter should match on beyond the title. Never drawn.
    finds: &'static str,
    /// What the middle column says when there is no live fact to put there.
    blurb: &'static str,
    /// The commands.
    leaves: &'static [Leaf],
}

const DATABASES_LEAVES: &[Leaf] = &[
    Leaf {
        title: "Tell sloop about a database",
        blurb: "it already exists on a server; this gives sloop the way in",
        command: "sloop db add <name>",
        job: Job::DbAdd,
        under: "Databases",
        run_it: "Register it",
    },
    Leaf {
        title: "Make a new database",
        blurb: "creates the database and its user on the server, then registers it",
        command: "sloop db create <name>",
        job: Job::DbCreate,
        under: "Databases",
        run_it: "Make it",
    },
    Leaf {
        title: "See the ones sloop knows",
        blurb: "every database in this registry, and where its password comes from",
        command: "sloop db list",
        job: Job::DbList,
        under: "Databases",
        run_it: "Show them",
    },
    Leaf {
        title: "Check one answers",
        blurb: "opens a connection and says what came back",
        command: "sloop db test <name>",
        job: Job::DbTest,
        under: "Databases",
        run_it: "Try it",
    },
    Leaf {
        title: "Change one's details",
        blurb: "host, port, user, database, password",
        command: "sloop db edit <name>",
        job: Job::DbEdit,
        under: "Databases",
        run_it: "Save the change",
    },
    Leaf {
        title: "Rename one",
        blurb: "the name sloop files it under — the server's own name is unchanged",
        command: "sloop db rename <from> <to>",
        job: Job::DbRename,
        under: "Databases",
        run_it: "Rename it",
    },
    Leaf {
        title: "Make sloop forget one",
        blurb: "removes it from this registry. The database itself is untouched",
        command: "sloop db remove <name>",
        job: Job::DbRemove,
        under: "Databases",
        run_it: "Forget it",
    },
    Leaf {
        title: "Delete one from the server",
        blurb: "the database itself, gone for good. `sloop backup` first if you want a copy",
        command: "sloop db drop <name>",
        job: Job::DbDrop,
        under: "Databases",
        run_it: "Delete it from the server",
    },
];

const BACKUPS_LEAVES: &[Leaf] = &[
    Leaf {
        title: "Back one up now",
        blurb: "dumps it, checks every row arrived, and writes a manifest beside it",
        command: "sloop backup <name>",
        job: Job::Backup,
        under: "Backups",
        run_it: "Back it up",
    },
    Leaf {
        title: "Back up every one of them",
        blurb: "carries on past a failure and says at the end which ones failed",
        command: "sloop backup --all",
        job: Job::BackupAll,
        under: "Backups",
        run_it: "Back them all up",
    },
    Leaf {
        title: "See the backups I have",
        blurb: "what was taken, when, how big, and whether it still checks out",
        command: "sloop backups list",
        job: Job::BackupsList,
        under: "Backups",
        run_it: "Show them",
    },
    Leaf {
        title: "Clear out the old ones",
        blurb: "keeps the most recent, deletes the rest",
        command: "sloop backups prune --keep 7",
        job: Job::BackupsPrune,
        under: "Backups",
        run_it: "Clear them out",
    },
    Leaf {
        title: "Put a backup back",
        blurb: "restores a database from one of them",
        command: "sloop restore <name>",
        job: Job::Restore,
        under: "Backups",
        run_it: "Put it back",
    },
];

const COPY_LEAVES: &[Leaf] = &[
    Leaf {
        title: "Mirror, an exact copy",
        blurb: "the destination ends up identical. Anything only it had is gone",
        command: "sloop mirror <source> --to <destination>",
        job: Job::Mirror,
        under: "Copying",
        run_it: "Mirror it",
    },
    Leaf {
        title: "Sync, a merge",
        blurb: "rows are added and replaced. Rows only the destination has are kept",
        command: "sloop sync <source> --to <destination>",
        job: Job::Sync,
        under: "Copying",
        run_it: "Merge it",
    },
];

const READ_LEAVES: &[Leaf] = &[Leaf {
    title: "Look inside a database",
    blurb: "pick a table and tick the columns — no SQL, and nothing it runs can write",
    command: "sloop query <name>",
    job: Job::Query,
    under: "Reading",
    run_it: "Open it",
}];

const SERVICE_LEAVES: &[Leaf] = &[
    Leaf {
        title: "Run sloop in the background",
        blurb: "registers it with this machine so it starts at boot. Needs an \
                        administrator or sudo",
        command: "sloop service install",
        job: Job::ServiceInstall,
        under: "The background service",
        run_it: "Register it",
    },
    Leaf {
        title: "Watch a database",
        blurb: "the service reads it every round. Picked up without a restart",
        command: "sloop service attach <name>",
        job: Job::ServiceAttach,
        under: "The background service",
        run_it: "Watch it",
    },
    Leaf {
        title: "Back one up on a schedule",
        blurb: "sloop runs the backup and the pruning. No cron line, no scheduled task",
        command: "sloop service schedule <name> --every 1d --keep 7",
        job: Job::ServiceSchedule,
        under: "The background service",
        run_it: "Set the schedule",
    },
    Leaf {
        title: "What my databases have been doing",
        blurb: "rows in, rows out and size, by day, week and month",
        command: "sloop service activity",
        job: Job::ServiceActivity,
        under: "The background service",
        run_it: "Show me",
    },
    Leaf {
        title: "Is it running?",
        blurb: "installed, running, when it last ran and when it runs next",
        command: "sloop service status",
        job: Job::ServiceStatus,
        under: "The background service",
        run_it: "Show me",
    },
    Leaf {
        title: "Stop watching one",
        blurb: "everything already recorded about it stays",
        command: "sloop service detach <name>",
        job: Job::ServiceDetach,
        under: "The background service",
        run_it: "Stop watching it",
    },
    Leaf {
        title: "Start it now",
        blurb: "rather than waiting for the next boot",
        command: "sloop service start",
        job: Job::ServiceStart,
        under: "The background service",
        run_it: "Start it",
    },
    Leaf {
        title: "Stop it now",
        blurb: "it still starts again at the next boot",
        command: "sloop service stop",
        job: Job::ServiceStop,
        under: "The background service",
        run_it: "Stop it",
    },
    Leaf {
        title: "Take it off this machine",
        blurb: "stops it and removes it. Nothing it recorded is deleted. Needs an \
                        administrator or sudo",
        command: "sloop service uninstall",
        job: Job::ServiceUninstall,
        under: "The background service",
        run_it: "Take it off",
    },
];

const MACHINE_LEAVES: &[Leaf] = &[
    Leaf {
        title: "Install a database server",
        blurb: "picks an engine and a version, then downloads, installs and starts it",
        command: "sloop server install",
        job: Job::ServerInstall,
        under: "This machine",
        run_it: "Choose one to install",
    },
    Leaf {
        title: "Open sloop's own database",
        blurb: "where sloop keeps its state, for psql, DataGrip or anything else",
        command: "sloop server connection",
        job: Job::ServerConnection,
        under: "This machine",
        run_it: "Show me",
    },
    Leaf {
        title: "Check my setup",
        blurb: "what sloop can find on this machine, and what it cannot",
        command: "sloop doctor",
        job: Job::Doctor,
        under: "This machine",
        run_it: "Check this machine",
    },
    Leaf {
        title: "Export the key",
        blurb: "copy it somewhere safe. Without it, no backup can ever be opened",
        command: "sloop key export",
        job: Job::KeyExport,
        under: "This machine",
        run_it: "Export the key",
    },
    Leaf {
        title: "Import a key",
        blurb: "bring one in from another machine",
        command: "sloop key import",
        job: Job::KeyImport,
        under: "This machine",
        run_it: "Import a key",
    },
];

/// **Every door, and every command behind it.**
///
/// **It stays a table on purpose.** Restructuring this menu again is editing the rows below,
/// not touching the loop that draws them — which is what makes the next reshuffle cheap.
///
/// Registering comes first because you cannot back up a database sloop has never heard of;
/// copying comes after both because it needs two.
const DOORS: &[Door] = &[
    Door {
        title: "Databases",
        blurb: "add one, check one, change one",
        command: "sloop db …",
        crumb: "Databases",
        question: "What about your databases?",
        finds: "add create register list test check edit rename remove forget drop delete",
        leaves: DATABASES_LEAVES,
    },
    Door {
        title: "Backups",
        blurb: "take one, put one back, clear the old ones",
        command: "sloop backup …",
        crumb: "Backups",
        question: "What about your backups?",
        finds: "back up all list restore put back prune clear out old",
        leaves: BACKUPS_LEAVES,
    },
    Door {
        title: "Copy a database",
        blurb: "mirror exactly, or merge with sync",
        command: "sloop mirror …",
        crumb: "Copying",
        question: "Which way of copying?",
        finds: "mirror sync copy clone merge exact duplicate move",
        leaves: COPY_LEAVES,
    },
    Door {
        title: "Look inside one",
        blurb: "tables and rows, with no SQL to type",
        command: "sloop query …",
        crumb: "Reading",
        question: "Which database?",
        finds: "query read look inside tables rows columns select",
        leaves: READ_LEAVES,
    },
    Door {
        title: "Background service",
        blurb: "watch, schedule, and what it has been doing",
        command: "sloop service …",
        crumb: "The background service",
        question: "What about the background service?",
        finds: "service install watch attach detach schedule activity status start stop uninstall daemon",
        leaves: SERVICE_LEAVES,
    },
    Door {
        title: "This machine",
        blurb: "servers, the backup key, and a health check",
        command: "sloop doctor",
        crumb: "This machine",
        question: "What about this machine?",
        finds: "server install postgres mysql mariadb connection doctor health key export import",
        leaves: MACHINE_LEAVES,
    },
];

/// What sits above the box a new project's directory is typed into.
fn new_project_lines(trouble: Option<&str>) -> Vec<Line> {
    let mut lines = vec![Line::Quiet(
        "A .sloop folder is created here, and it ignores itself — git never sees it.".to_owned(),
    )];
    if let Some(trouble) = trouble {
        lines.push(Line::Gap);
        lines.push(Line::Wrong(trouble.to_owned()));
    }
    lines
}

/// What `init` just did.
fn started_lines(report: &Initialised, shell: &Shell) -> Vec<Line> {
    vec![
        Line::Good(if report.existed {
            "There was already a project here.".to_owned()
        } else {
            "Project created.".to_owned()
        }),
        Line::Gap,
        Line::fact("registry", &report.registry.display().to_string()),
        Line::fact("global", &shell.global.display().to_string()),
    ]
}

/// What a job that finished says, above its choices.
///
/// **One screen for two outcomes.** A result with nothing amber in it is a tick and a green
/// rail; one with a single amber row in it is the same screen with that row in amber and the
/// headline changed to say so — which is the owner's Warning C, a result that worked with
/// something to know rather than a different kind of screen.
fn done_lines(leaf: &Leaf, told: &[Told], same: Option<&str>, took: &str) -> Vec<Line> {
    let warned = told.iter().any(|said| said.mark == Mark::Warn);
    let mut lines = vec![
        Line::Verdict {
            mark: if warned { Mark::Warn } else { Mark::Ok },
            what: if warned { "DONE, WITH A NOTE" } else { "DONE" }.to_owned(),
            subject: leaf.title.to_owned(),
            tag: took.to_owned(),
        },
        Line::Gap,
    ];
    lines.extend(rail(told));
    if let Some(same) = same {
        lines.push(Line::Gap);
        lines.push(Line::Quiet("the same thing, from a shell".to_owned()));
        lines.push(Line::Command(same.to_owned()));
    }
    lines
}

/// What a job that did not work says, above the ways out of it.
fn failed_lines(
    leaf: &Leaf,
    told: &[Told],
    said: &str,
    hint: Option<&str>,
    exit: Exit,
) -> Vec<Line> {
    let mut lines = vec![
        Line::Verdict {
            mark: Mark::Bad,
            what: "DID NOT WORK".to_owned(),
            subject: leaf.title.to_owned(),
            tag: format!("exit {}", exit.code()),
        },
        Line::Gap,
    ];
    lines.extend(rail(told));
    lines.push(Line::Gap);
    lines.push(Line::told("it said", said, Hue::Bad));
    if let Some(hint) = hint {
        lines.push(Line::told("try", hint, Hue::Text));
    }
    lines
}

/// The steps a job took, as the rail down the left of an outcome.
///
/// **The job's own words, not a summary of them.** Every marked step goes on the rail in the
/// colour it settled with — which is what makes the warning screen a result that worked with
/// one amber row in it rather than a different screen altogether.
fn rail(told: &[Told]) -> Vec<Line> {
    let steps: Vec<Line> = told
        .iter()
        .filter(|said| said.is_a_step())
        .map(|said| Line::Step {
            mark: said.mark,
            text: said.text.clone(),
            note: said.note.clone(),
        })
        .collect();
    if !steps.is_empty() {
        return steps;
    }

    // A job that settled no steps still said something, and an outcome screen with nothing
    // on it is worse than one carrying the last few lines it printed.
    let said: Vec<&Told> = told
        .iter()
        .filter(|said| !said.text.trim().is_empty())
        .collect();
    said.iter()
        .skip(said.len().saturating_sub(LAST_FEW))
        .map(|said| Line::Said(said.text.clone()))
        .collect()
}

/// How many of a job's own lines an outcome shows when it settled no steps of its own.
const LAST_FEW: usize = 8;

/// The door at `index`.
fn door_at(index: usize) -> Option<&'static Door> {
    DOORS.get(index)
}

/// The command at `index` behind door `door`.
fn leaf_in(door: usize, index: usize) -> Option<Leaf> {
    DOORS.get(door)?.leaves.get(index).copied()
}

/// The six doors, as the rows of the home screen.
fn door_items(standing: &Standing) -> Vec<Item> {
    DOORS
        .iter()
        .enumerate()
        .map(|(at, door)| {
            let (note, mark) = door_note(at, door, standing);
            Item::door(door, note, mark.hue())
        })
        .collect()
}

/// What one door says it is holding, and in which colour.
///
/// **Only what can be known cheaply and truthfully.** A door whose status would need a
/// health check to work out says what it is for instead — a status that is sometimes wrong
/// is worse than a phrase that is always right.
fn door_note(at: usize, door: &Door, standing: &Standing) -> (String, Mark) {
    match at {
        0 => match standing.databases {
            0 => ("nothing registered yet".to_owned(), Mark::Warn),
            1 => ("1 registered".to_owned(), Mark::Ok),
            many => (format!("{many} registered"), Mark::Ok),
        },
        1 => match &standing.backups {
            None => ("none taken yet".to_owned(), Mark::Warn),
            Some((1, when)) => (format!("1 kept · {when}"), Mark::Ok),
            Some((many, when)) => (format!("{many} kept · newest {when}"), Mark::Ok),
        },
        4 => match &standing.service {
            None => ("not registered with this machine".to_owned(), Mark::Todo),
            Some((what, true)) => (what.clone(), Mark::Ok),
            Some((what, false)) => (what.clone(), Mark::Warn),
        },
        _ => (door.blurb.to_owned(), Mark::Todo),
    }
}

/// The chips along the top of the home screen.
fn chips(standing: &Standing) -> Vec<(Mark, String)> {
    let mut chips = vec![match standing.databases {
        0 => (Mark::Warn, "no databases yet".to_owned()),
        1 => (Mark::Ok, "1 database".to_owned()),
        many => (Mark::Ok, format!("{many} databases")),
    }];

    chips.push(match &standing.backups {
        None => (Mark::Warn, "no backups".to_owned()),
        Some((_, when)) => (Mark::Ok, format!("last backup {when}")),
    });

    match &standing.service {
        None => chips.push((Mark::Todo, "no background service".to_owned())),
        Some((what, true)) => chips.push((Mark::Ok, format!("service {what}"))),
        Some((what, false)) => chips.push((Mark::Warn, format!("service {what}"))),
    }
    chips
}

impl Screen {
    /// Where the highlight sits, so that coming back puts it where it was.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        match self {
            Self::Setup { cursor, .. }
            | Self::FirstRun { cursor }
            | Self::Started { cursor, .. }
            | Self::Home { cursor }
            | Self::Door { cursor, .. }
            | Self::Done { cursor, .. }
            | Self::Failed { cursor, .. }
            | Self::Doing { cursor, .. } => *cursor,
            Self::NewProject { .. } => 0,
        }
    }

    /// Remember where the highlight was left.
    pub const fn point_at(&mut self, index: usize) {
        match self {
            Self::Setup { cursor, .. }
            | Self::FirstRun { cursor }
            | Self::Started { cursor, .. }
            | Self::Home { cursor }
            | Self::Door { cursor, .. }
            | Self::Done { cursor, .. }
            | Self::Failed { cursor, .. }
            | Self::Doing { cursor, .. } => *cursor = index,
            Self::NewProject { .. } => {}
        }
    }

    /// Remember what was typed, so that coming back finds it still typed.
    pub fn holding(&mut self, typed: &str) {
        if let Self::NewProject { at, .. } = self {
            typed.clone_into(at);
        }
    }

    /// `← Back` inside a flow, which is one *question* back rather than one screen.
    ///
    /// **The difference matters.** Every question is a screen of its own on the history, so
    /// popping the stack would work — right up until somebody backs out of a flow entirely
    /// and comes into it again, where they would find every answer still given and no
    /// question left to ask. Dropping the last answer is the same move said in the flow's
    /// own terms, and `false` means there was nothing left to drop, so the loop takes over
    /// and leaves.
    pub fn stepped_back(&mut self) -> bool {
        match self {
            Self::Doing {
                answers, trouble, ..
            } => {
                *trouble = None;
                answers.undo()
            }
            _ => false,
        }
    }

    /// The first screen of a flow, with nothing answered yet.
    #[must_use]
    pub fn opening(leaf: Leaf) -> Self {
        Self::Doing {
            leaf,
            answers: Box::default(),
            cursor: 0,
            trouble: None,
        }
    }

    /// Is this the screen a finished job leaves behind?
    ///
    /// **It is the one place the way out is `Home` rather than `← Back`.** The owner's
    /// reason: *"in outcome screen show Home instead of Back. more understandable and user
    /// friendly"*. A result is the end of something, not a step in the middle of it, and
    /// backing out of it one door at a time is walking back up a path nobody is on any more.
    #[must_use]
    pub const fn is_an_outcome(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Failed { .. })
    }

    /// The breadcrumb, deepest part last.
    #[must_use]
    pub fn crumbs(&self) -> Vec<&'static str> {
        match self {
            Self::Setup { .. } | Self::FirstRun { .. } | Self::Home { .. } => Vec::new(),
            Self::NewProject { .. } => vec!["New project"],
            Self::Started { .. } => vec!["New project", "Done"],
            Self::Door { door, .. } => {
                door_at(*door).map_or_else(Vec::new, |door| vec![door.crumb])
            }
            Self::Doing { leaf, .. } | Self::Done { leaf, .. } | Self::Failed { leaf, .. } => {
                vec![leaf.under, leaf.title]
            }
        }
    }

    /// Everything above the list or the box.
    #[must_use]
    pub fn header(&self, shell: &Shell, world: &dyn Doing) -> Header {
        match self {
            Self::Setup { trouble, .. } => setup_header(trouble.as_deref()),

            Self::FirstRun { .. } => Header {
                banner: Banner::Wordmark,
                crumbs: Vec::new(),
                strap: "register a database once, then back it up and copy it".to_owned(),
                lines: vec![Line::told("machine", "nothing registered yet", Hue::Warn)],
            },

            Self::NewProject { at: _, trouble } => Header {
                banner: Banner::Word,
                crumbs: self.crumbs(),
                strap: String::new(),
                lines: new_project_lines(trouble.as_deref()),
            },

            Self::Started { report, .. } => Header {
                banner: Banner::Word,
                crumbs: self.crumbs(),
                strap: String::new(),
                lines: started_lines(report, shell),
            },

            Self::Home { .. } => Header {
                banner: Banner::Wordmark,
                crumbs: Vec::new(),
                strap: "your databases, backed up and moved about".to_owned(),
                lines: vec![
                    Line::Chips(chips(&shell.standing)),
                    Line::Quiet(format!(
                        "{}  \u{00b7}  {}",
                        shell.working_in, shell.found_by
                    )),
                ],
            },

            Self::Door { door, .. } => Header {
                banner: Banner::Word,
                crumbs: self.crumbs(),
                strap: String::new(),
                lines: door_at(*door).map_or_else(Vec::new, |open| {
                    // A chip rather than a fact: the crumb above already names the door, and
                    // a label repeating it would be the word twice on two lines.
                    let (note, mark) = door_note(*door, open, &shell.standing);
                    vec![Line::Chips(vec![(mark, note)])]
                }),
            },

            // **The screen that is read after a job, and the reason there is one.** Nothing
            // drawn inside the alternate buffer survives it, so what the job said on the live
            // screen is gone the moment it returns. This is what is left.
            Self::Done {
                leaf,
                told,
                same,
                took,
                ..
            } => Header {
                banner: Banner::Word,
                crumbs: self.crumbs(),
                strap: String::new(),
                lines: done_lines(leaf, told, same.as_deref(), took),
            },

            Self::Failed {
                leaf,
                told,
                said,
                hint,
                exit,
                ..
            } => Header {
                banner: Banner::Word,
                crumbs: self.crumbs(),
                strap: String::new(),
                lines: failed_lines(leaf, told, said, hint.as_deref(), *exit),
            },

            Self::Doing {
                leaf,
                answers,
                trouble,
                ..
            } => {
                let mut lines = vec![Line::Lead(leaf.blurb.to_owned())];

                if let Next::Blocked(why) = leaf.job.next(answers, world) {
                    lines.push(Line::Gap);
                    lines.push(Line::Quiet(why));
                }

                // **What has been answered, as it is answered.** A flow six questions long
                // is otherwise six screens with no memory of each other, and somebody four
                // questions in has no way to check what they said on the first.
                for (label, value) in Job::so_far(answers) {
                    lines.push(Line::fact(label, &value));
                }

                if let Some(trouble) = trouble {
                    lines.push(Line::Gap);
                    lines.push(Line::Wrong(trouble.clone()));
                }

                Header {
                    banner: Banner::Word,
                    crumbs: self.crumbs(),
                    strap: String::new(),
                    lines,
                }
            }
        }
    }

    /// The list, or the box.
    #[must_use]
    pub fn face(&self, standing: &Standing, world: &dyn Doing) -> Face {
        match self {
            Self::Setup { trouble, .. } => Face::Menu(Menu::of(
                if trouble.is_some() {
                    "Try again — nothing that already worked is done twice."
                } else {
                    "One step, and then everything else works."
                },
                vec![Item::new(
                    if trouble.is_some() {
                        "Carry on setting up this machine"
                    } else {
                        "Set up this machine"
                    },
                    "finds PostgreSQL 18 or installs it, then makes sloop's database, its \
                     role and its tables",
                )],
            )),

            Self::FirstRun { .. } => Face::Menu(Menu::of(
                "Let's start a project.",
                vec![Item::new(
                    "Start a project here",
                    "keeps this folder's databases beside the code that uses them",
                )],
            )),

            Self::NewProject { at, .. } => Face::Ask(Ask {
                question: "Where should the project go?".to_owned(),
                initial: at.clone(),
                help: "Enter accepts it. Esc goes back.".to_owned(),
            }),

            Self::Started { .. } => Face::Menu(Menu::of(
                "That is the hard part done.",
                vec![Item::new(
                    "Go to the menu",
                    "add a database, and sloop can start backing it up",
                )],
            )),

            Self::Home { .. } => {
                Face::Menu(Menu::of("What would you like to do?", door_items(standing)))
            }

            Self::Door { door, .. } => Face::Menu(Menu::of(
                door_at(*door).map_or("What would you like to do?", |door| door.question),
                door_at(*door).map_or_else(Vec::new, |door| {
                    door.leaves.iter().copied().map(Item::doing).collect()
                }),
            )),

            // **Nothing on it but the way out, and that is deliberate.** A result used to
            // carry the thing it had just done, highlighted — so Enter on the screen saying a
            // database had been created created another one. The owner's words: *"the option
            // like create db, or setup sloop still available there, and selected, means
            // pressing enter will reexecute the same thing"*. An empty list leaves the cursor
            // on `← Home`, which is the only thing there is to do here.
            Self::Done { .. } => Face::Menu(Menu::of("That is done.", Vec::new())),

            // **The owner's rule 9**, in three rows: a failure used to leave one sentence and
            // no way forward but running the same answers again.
            Self::Failed { .. } => Face::Menu(Menu::of(
                "What now?",
                vec![
                    Item::new(
                        "Try it again",
                        "with the same answers — nothing was changed",
                    ),
                    Item::new("Change an answer", "back one question, the rest kept"),
                ],
            )),

            Self::Doing { leaf, answers, .. } => match leaf.job.next(answers, world) {
                Next::Ask(step) => {
                    let Step { question, how, .. } = *step;
                    match how {
                        How::Pick { items, .. } => Face::Menu(Menu::of(&question, items)),
                        How::Type { initial, help, .. } => Face::Ask(Ask {
                            question,
                            initial,
                            help,
                        }),
                    }
                }
                // Nothing left to ask: one item, and choosing it runs the job. Never run
                // straight off the last answer — a flow that fires the moment its last
                // question is answered is a flow nobody can read back before it happens.
                // **Its own sentence, not its title.** *"Delete one from the server"* has to
                // become *"Delete it"*, and the last thing somebody reads before a database
                // stops existing is worth writing by hand — see `Leaf::run_it`.
                Next::Ready => Face::Menu(Menu::of(
                    "Everything it needs has been answered.",
                    vec![Item {
                        title: leaf.run_it.to_owned(),
                        ..Item::doing(*leaf)
                    }],
                )),
                Next::Blocked(_) => Face::Menu(Menu::of("Not from here.", Vec::new())),
            },
        }
    }

    /// Somebody picked item `index`.
    #[must_use]
    pub fn chose(&self, shell: &Shell, world: &dyn Doing, index: usize) -> Flow {
        match self {
            // **Out of the alternate screen, and that is the point.** Setup finds or
            // installs a PostgreSQL, makes a database and runs migrations, and says what it
            // is doing as it goes. Inside the alternate screen every one of those lines would
            // vanish on the next redraw; `Flow::Run` hands the terminal back first, so they
            // land in the scrollback the user keeps — and so the one question Setup can ask,
            // for the superuser password of a PostgreSQL it did not install, is asked the
            // same way it is asked from a shell.
            Self::Setup { .. } => Flow::Run(SETUP, Box::default()),

            Self::FirstRun { .. } => Flow::To(Screen::NewProject {
                at: shell.cwd.display().to_string(),
                trouble: None,
            }),

            Self::Home { .. } => match door_at(index) {
                None => Flow::Stay,
                // **A door with one command behind it is that command.** A submenu of one
                // is a keystroke that asks nothing, which is the thing the doors were meant
                // to stop.
                Some(door) if door.leaves.len() == 1 => door
                    .leaves
                    .first()
                    .copied()
                    .map_or(Flow::Stay, |leaf| Flow::To(Self::opening(leaf))),
                Some(_) => Flow::To(Self::Door {
                    door: index,
                    cursor: 0,
                }),
            },

            Self::Door { door, .. } => {
                leaf_in(*door, index).map_or(Flow::Stay, |leaf| Flow::To(Self::opening(leaf)))
            }

            // **Two screens with one answer: the top of the tree.** `Started` has just made a
            // registry and a result has nothing on it that can be chosen at all — the way out
            // the loop adds is the whole of what a result offers.
            Self::Started { .. } | Self::Done { .. } => Flow::Home,

            // **Rule 9, and the whole point of this screen.** Nothing here goes home on its
            // own: one row runs it again as it stands, the other steps back into the flow
            // with the last answer dropped and every other one kept.
            Self::Failed { leaf, answers, .. } => {
                if index == 0 {
                    return Flow::Run(*leaf, answers.clone());
                }
                let mut answers = answers.clone();
                answers.undo();
                Flow::Same(Self::Doing {
                    leaf: *leaf,
                    answers,
                    cursor: 0,
                    trouble: None,
                })
            }

            Self::Doing { leaf, answers, .. } => match leaf.job.next(answers, world) {
                Next::Ask(step) => match step.how {
                    // The answer filed is the value behind the item, never the label: a
                    // menu reads in English and a flag does not, and the two must not end
                    // up being the same string by accident.
                    How::Pick { values, .. } => values.get(index).map_or(Flow::Stay, |value| {
                        let mut answers = answers.clone();
                        answers.put(step.field, value.clone());
                        Flow::Same(Self::Doing {
                            leaf: *leaf,
                            answers,
                            cursor: 0,
                            trouble: None,
                        })
                    }),
                    How::Type { .. } => Flow::Stay,
                },
                Next::Ready => Flow::Run(*leaf, answers.clone()),
                Next::Blocked(_) => Flow::Stay,
            },

            // It has no item of its own; the loop has already handled the way out.
            Self::NewProject { .. } => Flow::Stay,
        }
    }

    /// Somebody typed something and pressed enter.
    ///
    /// **The one place this shell changes anything**, and the reason it is a `Result`: a
    /// directory that is not there is a thing to say on the screen, not a thing to end the
    /// session over. Whatever it makes goes into `kept` to be printed after the terminal
    /// has been given back — a summary lost inside the alternate screen is a summary lost.
    pub fn typed(
        &mut self,
        shell: &mut Shell,
        world: &dyn Doing,
        kept: &mut Vec<Kept>,
        given: &str,
    ) -> Flow {
        if let Self::Doing { leaf, answers, .. } = self {
            let Next::Ask(step) = leaf.job.next(answers, world) else {
                return Flow::Stay;
            };

            // **A blank is an answer for some boxes and not for others.** Left to reach the
            // command, an empty name comes back as a usage error about a flag nobody
            // passed, five screens away from the box it was not typed into.
            if given.trim().is_empty() && matches!(step.how, How::Type { needed: true, .. }) {
                return Flow::Same(Self::Doing {
                    leaf: *leaf,
                    answers: answers.clone(),
                    cursor: 0,
                    trouble: Some(format!("{} It cannot be left blank.", step.question)),
                });
            }

            let mut answers = answers.clone();
            answers.put(step.field, given.trim());
            return Flow::Same(Self::Doing {
                leaf: *leaf,
                answers,
                cursor: 0,
                trouble: None,
            });
        }

        let Self::NewProject { trouble, .. } = self else {
            return Flow::Stay;
        };

        match start_project(shell, given) {
            Ok(report) => {
                shell.moved_into(&report.registry);
                *trouble = None;
                kept.push(Kept::Started(Box::new(report.clone())));
                Flow::To(Screen::Started {
                    report: Box::new(report),
                    cursor: 0,
                })
            }
            Err(failure) => {
                *trouble = Some(failure.message().to_owned());
                Flow::Stay
            }
        }
    }
}

/// Something worth keeping, printed after the alternate screen has been handed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// `init` ran, and this is what it made.
    Started(Box<Initialised>),
}

/// Make the registry, with the same code `sloop init` runs.
fn start_project(shell: &Shell, given: &str) -> Outcome<Initialised> {
    let given = given.trim();
    if given.is_empty() {
        return Err(Failure::usage("a project needs a directory to go in")
            .hint("type a path — `.` is the directory sloop was started in"));
    }

    let target = crate::registry::normalize(&shell.cwd.join(given));
    if !target.is_dir() {
        return Err(Failure::usage(format!(
            "{} is not a directory that exists",
            target.display()
        ))
        .hint("make it first, then type it again — sloop starts a registry, not a directory"));
    }

    crate::commands::init::create(&target, &shell.global, &shell.home)
}
