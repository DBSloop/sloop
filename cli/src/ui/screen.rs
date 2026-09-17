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
use super::paint::{Banner, Header, Line};
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
    /// How many databases it holds.
    pub holds: usize,
    /// True when there is nothing registered anywhere and no project above the cwd.
    pub fresh: bool,
}

impl Shell {
    /// The screen a session opens on.
    ///
    /// **First run offers `Init` and nothing else.** Not a menu with most of it greyed out:
    /// every other screen in this tree needs a registry to be useful, and a menu whose items
    /// all fail is a worse first impression than a menu with one item that works.
    #[must_use]
    pub const fn opening(&self) -> Screen {
        if self.fresh {
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
    /// The wordmark, and everything sloop can do.
    Home {
        /// Where the highlight is.
        cursor: usize,
    },
    /// One group of commands.
    Group {
        /// Which one.
        group: Group,
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

/// A list of things to pick from, under headings. The way out is added by the loop, never
/// by a screen — which is how *"`← Back` on every menu"* stays true of a menu nobody has
/// looked at yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// The line above the list.
    pub question: String,
    /// The headings, and what is under each. At least one, and often more.
    pub sections: Vec<Section>,
}

/// A heading and the things under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// All caps, in the accent. A row nobody can choose.
    pub heading: String,
    /// What is under it.
    pub items: Vec<Item>,
}

/// One drawn line of a menu: a heading, or something that can be chosen.
///
/// **Flattened here rather than by the renderer**, so there is one answer to "what is on
/// row seven" and the loop, the painter and the tests all read it from the same place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A heading. Choosing it is not a thing that can happen — see [`Menu::rows`].
    Heading(String),
    /// An item, and which item it is counting only items.
    Item(Item, usize),
}

impl Menu {
    /// One section, for the screens that have nothing to group.
    #[must_use]
    pub fn under(heading: &str, question: &str, items: Vec<Item>) -> Self {
        Self {
            question: question.to_owned(),
            sections: vec![Section {
                heading: heading.to_owned(),
                items,
            }],
        }
    }

    /// The rows as they are drawn, headings and all.
    ///
    /// Every item carries its own index among items, so nothing downstream has to count
    /// headings to work out what was chosen — which is the one arithmetic mistake that
    /// would silently run the wrong command.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut chosen = 0;
        for section in &self.sections {
            if section.items.is_empty() {
                continue;
            }
            rows.push(Row::Heading(section.heading.clone()));
            for item in &section.items {
                rows.push(Row::Item(item.clone(), chosen));
                chosen += 1;
            }
        }
        rows
    }

    /// How many things can actually be chosen, headings not counted.
    ///
    /// Only the tests ask: the loop works in rows, because a row is what the cursor sits
    /// on. This is how a test says "this page offers nothing" without counting headings.
    #[cfg(test)]
    #[must_use]
    pub fn choices(&self) -> usize {
        self.sections
            .iter()
            .map(|section| section.items.len())
            .sum()
    }
}

/// One line in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// What it is called.
    pub title: String,
    /// What it does, in one phrase. Shown when there is no command to show instead.
    pub blurb: String,
    /// The flag form that does the same thing, for the items that are commands.
    ///
    /// **The owner asked for it beside every one of them** — *"each task show the command
    /// that how it can be done by command"*. It is reference and never an instruction: the
    /// menu does the whole job, and this is how the same job is written into a crontab.
    pub command: String,
}

impl Item {
    /// Something to pick, with a phrase saying what it means.
    #[must_use]
    pub fn new(title: &str, blurb: &str) -> Self {
        Self {
            title: title.to_owned(),
            blurb: blurb.to_owned(),
            command: String::new(),
        }
    }

    /// A command, with the flag form beside it.
    #[must_use]
    pub fn doing(leaf: Leaf) -> Self {
        Self {
            title: leaf.title.to_owned(),
            blurb: leaf.blurb.to_owned(),
            command: leaf.command.to_owned(),
        }
    }

    /// What is drawn to the right of the title.
    #[must_use]
    pub fn beside(&self) -> &str {
        if self.command.is_empty() {
            &self.blurb
        } else {
            &self.command
        }
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

/// The groups on the home screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Everything about which databases sloop knows.
    Databases,
    /// Taking backups, listing them, putting one back, clearing old ones out.
    Backups,
    /// `mirror` and `sync`.
    Copying,
    /// The keypair a backup is encrypted with.
    Key,
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

/// A door on the home screen: into a group, or straight at a command.
#[derive(Debug, Clone, Copy)]
enum Door {
    /// Opens a group menu.
    Into(Group, &'static str, &'static str),
    /// Runs a command.
    At(Leaf),
}

impl Door {
    fn opens(self) -> Screen {
        match self {
            Self::Into(group, _, _) => Screen::Group { group, cursor: 0 },
            Self::At(leaf) => Screen::opening(leaf),
        }
    }

    /// How it is drawn. A group says what is behind it; a command says the flag form that
    /// does the same thing, which is what the owner asked for beside every one of them.
    fn item(self) -> Item {
        match self {
            Self::Into(_, title, blurb) => Item::new(title, blurb),
            Self::At(leaf) => Item::doing(leaf),
        }
    }
}

/// A heading on the home screen, and the doors under it.
struct Wing(&'static str, &'static [Door]);

/// A heading on a group screen, and the commands under it.
struct Shelf(&'static str, &'static [Leaf]);

/// The home screen, in the order somebody meets the tool.
///
/// Registering comes before backing up because you cannot back up a database sloop has
/// never heard of; copying comes after both because it needs two.
const HOME: &[Wing] = &[
    Wing(
        "EVERY DAY",
        &[
            Door::Into(
                Group::Databases,
                "Databases",
                "tell sloop about one, check it answers, rename it",
            ),
            Door::Into(
                Group::Backups,
                "Backups",
                "take one now, put one back, clear the old ones out",
            ),
        ],
    ),
    Wing(
        "WHEN YOU NEED IT",
        &[
            Door::Into(
                Group::Copying,
                "Copy a database",
                "an exact mirror, or a merge that keeps what is there",
            ),
            Door::Into(
                Group::Key,
                "Backup key",
                "the key your backups are locked with",
            ),
            Door::At(Leaf {
                title: "Check my setup",
                blurb: "what sloop can find on this machine, and what it cannot",
                command: "sloop doctor",
                job: Job::Doctor,
                under: "Check my setup",
                run_it: "Check this machine",
            }),
        ],
    ),
];

const DATABASES: &[Shelf] = &[
    Shelf(
        "ADD ONE",
        &[
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
        ],
    ),
    Shelf(
        "LOOK",
        &[
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
        ],
    ),
    Shelf(
        "CHANGE",
        &[
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
        ],
    ),
    Shelf(
        "REMOVE",
        &[
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
                blurb: "the database itself, gone. A safety copy is taken first",
                command: "sloop db drop <name>",
                job: Job::DbDrop,
                under: "Databases",
                run_it: "Delete it from the server",
            },
        ],
    ),
];

const BACKUPS: &[Shelf] = &[
    Shelf(
        "TAKE ONE",
        &[
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
        ],
    ),
    Shelf(
        "WHAT YOU HAVE",
        &[
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
        ],
    ),
    Shelf(
        "PUT ONE BACK",
        &[Leaf {
            title: "Put a backup back",
            blurb: "restores a database from one of them",
            command: "sloop restore <name>",
            job: Job::Restore,
            under: "Backups",
            run_it: "Put it back",
        }],
    ),
];

const COPYING: &[Shelf] = &[Shelf(
    "COPYING",
    &[
        Leaf {
            title: "Mirror, an exact copy",
            blurb: "the destination ends up identical. Anything only it had is gone",
            command: "sloop mirror <source> --to <destination>",
            job: Job::Mirror,
            under: "Copy a database",
            run_it: "Mirror it",
        },
        Leaf {
            title: "Sync, a merge",
            blurb: "rows are added and replaced. Rows only the destination has are kept",
            command: "sloop sync <source> --to <destination>",
            job: Job::Sync,
            under: "Copy a database",
            run_it: "Merge it",
        },
    ],
)];

const KEY: &[Shelf] = &[Shelf(
    "BACKUP KEY",
    &[
        Leaf {
            title: "Export the key",
            blurb: "copy it somewhere safe. Without it, no backup can ever be opened",
            command: "sloop key export",
            job: Job::KeyExport,
            under: "Backup key",
            run_it: "Export the key",
        },
        Leaf {
            title: "Import a key",
            blurb: "bring one in from another machine",
            command: "sloop key import",
            job: Job::KeyImport,
            under: "Backup key",
            run_it: "Import a key",
        },
    ],
)];

/// The door at `index`, counting doors and not headings.
fn door_at(index: usize) -> Option<Door> {
    HOME.iter()
        .flat_map(|Wing(_, doors)| doors.iter())
        .nth(index)
        .copied()
}

/// The home screen as sections.
fn home_sections() -> Vec<Section> {
    HOME.iter()
        .map(|Wing(heading, doors)| Section {
            heading: (*heading).to_owned(),
            items: doors.iter().map(|door| door.item()).collect(),
        })
        .collect()
}

impl Group {
    /// What the breadcrumb calls it.
    const fn title(self) -> &'static str {
        match self {
            Self::Databases => "Databases",
            Self::Backups => "Backups",
            Self::Copying => "Copy a database",
            Self::Key => "Backup key",
        }
    }

    /// The line above the list.
    const fn question(self) -> &'static str {
        match self {
            Self::Databases => "What would you like to do with a database?",
            Self::Backups => "What would you like to do about backups?",
            Self::Copying => "Which kind of copy?",
            Self::Key => "What would you like to do with the key?",
        }
    }

    /// What is in it, under its headings.
    const fn shelves(self) -> &'static [Shelf] {
        match self {
            Self::Databases => DATABASES,
            Self::Backups => BACKUPS,
            Self::Copying => COPYING,
            Self::Key => KEY,
        }
    }

    /// The command at `index`, counting commands and not headings.
    fn leaf(self, index: usize) -> Option<Leaf> {
        self.shelves()
            .iter()
            .flat_map(|Shelf(_, leaves)| leaves.iter())
            .nth(index)
            .copied()
    }

    /// The page, as the headings it is drawn under.
    fn sections(self) -> Vec<Section> {
        self.shelves()
            .iter()
            .map(|Shelf(heading, leaves)| Section {
                heading: (*heading).to_owned(),
                items: leaves.iter().copied().map(Item::doing).collect(),
            })
            .collect()
    }

    /// Every group, for the walk that proves `← Back` comes home from all of them.
    #[cfg(test)]
    pub const ALL: &'static [Self] = &[Self::Databases, Self::Backups, Self::Copying, Self::Key];
}

impl Screen {
    /// Where the highlight sits, so that coming back puts it where it was.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        match self {
            Self::FirstRun { cursor }
            | Self::Started { cursor, .. }
            | Self::Home { cursor }
            | Self::Group { cursor, .. }
            | Self::Doing { cursor, .. } => *cursor,
            Self::NewProject { .. } => 0,
        }
    }

    /// Remember where the highlight was left.
    pub const fn point_at(&mut self, index: usize) {
        match self {
            Self::FirstRun { cursor }
            | Self::Started { cursor, .. }
            | Self::Home { cursor }
            | Self::Group { cursor, .. }
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

    /// The same flow with its last run's complaint on it, so a failure is read on the
    /// screen that caused it rather than scrolling past in the terminal underneath.
    #[must_use]
    pub fn troubled(self, said: String) -> Self {
        match self {
            Self::Doing {
                leaf,
                answers,
                cursor,
                ..
            } => Self::Doing {
                leaf,
                answers,
                cursor,
                trouble: Some(said),
            },
            other => other,
        }
    }

    /// The breadcrumb, deepest part last.
    #[must_use]
    pub fn crumbs(&self) -> Vec<&'static str> {
        match self {
            Self::FirstRun { .. } | Self::Home { .. } => Vec::new(),
            Self::NewProject { .. } => vec!["New project"],
            Self::Started { .. } => vec!["New project", "Done"],
            Self::Group { group, .. } => vec![group.title()],
            Self::Doing { leaf, .. } => vec![leaf.under, leaf.title],
        }
    }

    /// Everything above the list or the box.
    #[must_use]
    pub fn header(&self, shell: &Shell, world: &dyn Doing) -> Header {
        match self {
            Self::FirstRun { .. } => Header {
                banner: Banner::Wordmark,
                crumbs: Vec::new(),
                strap: "register a database once, then back it up and copy it".to_owned(),
                lines: vec![Line::told("machine", "nothing registered yet", Hue::Warn)],
            },

            Self::NewProject { at: _, trouble } => {
                let mut lines = vec![Line::Quiet(
                    "A .sloop folder is created here, and it ignores itself — git never sees it."
                        .to_owned(),
                )];
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

            Self::Started { report, .. } => Header {
                banner: Banner::Word,
                crumbs: self.crumbs(),
                strap: String::new(),
                lines: vec![
                    Line::Good(if report.existed {
                        "There was already a project here.".to_owned()
                    } else {
                        "Project created.".to_owned()
                    }),
                    Line::Gap,
                    Line::fact("registry", &report.registry.display().to_string()),
                    Line::fact("global", &shell.global.display().to_string()),
                ],
            },

            Self::Home { .. } => Header {
                banner: Banner::Wordmark,
                crumbs: Vec::new(),
                strap: "your databases, backed up and moved about".to_owned(),
                lines: vec![
                    Line::fact("working in", &shell.working_in),
                    Line::under(&shell.found_by),
                    match shell.holds {
                        0 => Line::told("registered", "nothing yet", Hue::Warn),
                        1 => Line::told("registered", "1 database", Hue::Ok),
                        many => Line::told("registered", &format!("{many} databases"), Hue::Ok),
                    },
                ],
            },

            Self::Group { group, .. } => Header {
                banner: Banner::Word,
                crumbs: self.crumbs(),
                strap: String::new(),
                lines: vec![Line::Quiet(group.blurb().to_owned())],
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
    pub fn face(&self, world: &dyn Doing) -> Face {
        match self {
            Self::FirstRun { .. } => Face::Menu(Menu::under(
                "GET STARTED",
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

            Self::Started { .. } => Face::Menu(Menu::under(
                "WHAT NEXT",
                "That is the hard part done.",
                vec![Item::new(
                    "Go to the menu",
                    "add a database, and sloop can start backing it up",
                )],
            )),

            Self::Home { .. } => Face::Menu(Menu {
                question: "What would you like to do?".to_owned(),
                sections: home_sections(),
            }),

            Self::Group { group, .. } => Face::Menu(Menu {
                question: group.question().to_owned(),
                sections: group.sections(),
            }),

            Self::Doing { leaf, answers, .. } => match leaf.job.next(answers, world) {
                Next::Ask(step) => {
                    let heading = step.heading();
                    let Step { question, how, .. } = *step;
                    match how {
                        How::Pick { items, .. } => {
                            Face::Menu(Menu::under(heading, &question, items))
                        }
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
                Next::Ready => Face::Menu(Menu::under(
                    "READY",
                    "Everything it needs has been answered.",
                    vec![Item {
                        title: leaf.run_it.to_owned(),
                        blurb: leaf.blurb.to_owned(),
                        command: leaf.command.to_owned(),
                    }],
                )),
                Next::Blocked(_) => Face::Menu(Menu {
                    question: "Not from here.".to_owned(),
                    sections: Vec::new(),
                }),
            },
        }
    }

    /// Somebody picked item `index`.
    #[must_use]
    pub fn chose(&self, shell: &Shell, world: &dyn Doing, index: usize) -> Flow {
        match self {
            Self::FirstRun { .. } => Flow::To(Screen::NewProject {
                at: shell.cwd.display().to_string(),
                trouble: None,
            }),
            Self::Started { .. } => Flow::Home,
            Self::Home { .. } => door_at(index).map_or(Flow::Stay, |door| Flow::To(door.opens())),
            Self::Group { group, .. } => group
                .leaf(index)
                .map_or(Flow::Stay, |leaf| Flow::To(Self::opening(leaf))),

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

impl Group {
    /// The line under the breadcrumb.
    const fn blurb(self) -> &'static str {
        match self {
            Self::Databases => {
                "A database sloop knows about can be backed up, copied and checked. \
                 Everything here is about that list."
            }
            Self::Backups => {
                "A backup is a dump, a manifest and an exact row count, in a folder named \
                 for the moment it was taken."
            }
            Self::Copying => {
                "Both read the source and only the source. The difference is what happens \
                 to rows the destination already has."
            }
            Self::Key => {
                "Backups are encrypted with an age keypair. The public half lives in the \
                 config so a scheduled run needs no secret; the private half is in this \
                 machine's keyring, and only restoring needs it."
            }
        }
    }
}

/// Make the registry, with the same code `sloop init` runs.
fn start_project(shell: &Shell, given: &str) -> Outcome<Initialised> {
    let given = given.trim();
    if given.is_empty() {
        return Err(Failure::usage("a project needs a directory to go in"));
    }

    let target = crate::registry::normalize(&shell.cwd.join(given));
    if !target.is_dir() {
        return Err(Failure::usage(format!(
            "{} is not a directory that exists",
            target.display()
        )));
    }

    crate::commands::init::create(&target, &shell.global, &shell.home)
}
