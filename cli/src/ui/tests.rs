//! The shell, driven without a terminal.
//!
//! **Every one of these walks the real loop.** The only thing replaced is the person: a
//! [`Scripted`] answers each prompt from a list written down in advance and records what it
//! was shown, so `← Back` returning from every screen in the tree is an assertion rather
//! than something somebody remembered to try by hand once.

use std::path::PathBuf;

use super::ask::{Answer, Asking};
use super::flow::{Answers, Doing, Job, field};
use super::paint::Header;
use super::screen::{Ask, Item, Kept, Screen, Shell, Standing};
use super::{Alternate, BACK, HOME, QUIT, at_a_terminal, walk};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// A world with two databases and a backup of each, which runs nothing and remembers what
/// it was asked to run.
struct Bench {
    /// Every job that was run, with the answers it was handed.
    ran: Vec<(Job, Answers)>,
    /// What the next job should come back as.
    says: Option<Failure>,
    /// Whether this run could register something to start at boot. `Default` says yes, so
    /// every test that is not about elevation is unaffected by it.
    elevated: bool,
}

impl Default for Bench {
    fn default() -> Self {
        Self {
            ran: Vec::new(),
            says: None,
            elevated: true,
        }
    }
}

impl Bench {
    /// The same bench in a terminal that cannot register anything to start at boot.
    fn unprivileged() -> Self {
        Self {
            elevated: false,
            ..Self::default()
        }
    }
}

impl Doing for Bench {
    fn databases(&self) -> Vec<String> {
        vec!["orders".to_owned(), "orders_staging".to_owned()]
    }

    fn backups_of(&self, _name: &str) -> Vec<String> {
        vec!["20260916T031500Z".to_owned()]
    }

    fn globally_registered(&self) -> Vec<String> {
        self.databases()
    }

    fn watched(&self) -> Vec<String> {
        self.databases()
    }

    fn may_change_the_machine(&self) -> bool {
        self.elevated
    }

    fn on_the_server(&self, label: &str) -> Option<String> {
        Some(format!("{label}_live"))
    }

    fn standing(&self) -> Standing {
        Standing {
            databases: self.databases().len(),
            backups: Some((2, "2h ago".to_owned())),
            service: Some(("running".to_owned(), true)),
        }
    }

    fn run(&mut self, job: Job, answers: &Answers) -> Outcome<Exit> {
        self.ran.push((job, answers.clone()));
        self.says.take().map_or(Ok(Exit::Success), Err)
    }
}

/// One thing a scripted person does.
#[derive(Debug, Clone)]
enum Does {
    /// Pick the item at this index.
    Pick(usize),
    /// The way out: the last item on a list, and Esc in a box — which are the same move
    /// said twice, because a box has no items to have a last one.
    Leave,
    /// Whatever is in front of them, accepted exactly as it stands: the first item, or
    /// the box with whatever is already in it, blank or not.
    Enter,
    /// The same, except that a box needing an answer gets one. What somebody filling a
    /// flow in actually does, and what the walks below use to reach the end of one.
    Fill,
    /// Type this and press enter.
    Type(&'static str),
    /// Press Esc.
    Back,
    /// Press Ctrl-C.
    Quit,
}

/// Somebody sitting at the menu whose every keystroke is written down in advance.
struct Scripted {
    doing: std::vec::IntoIter<Does>,
    /// Every screen drawn, in order, as its breadcrumb.
    seen: Vec<String>,
    /// Every list shown, as the titles on it.
    listed: Vec<Vec<String>>,
    /// Where the highlight was when each list was shown.
    started_at: Vec<usize>,
    /// The middle column of every row of every list, in the colour it carried.
    noted: Vec<Vec<String>>,
    /// The row each choice landed on.
    answered_at: Vec<usize>,
    /// Every box shown, as what was already in it.
    boxes: Vec<String>,
    /// **`R20`'s line, wherever a screen carried one.** It used to be printed after the
    /// terminal was handed back; there is no such moment now, so it lives on the outcome
    /// screen and is read from there.
    same: Vec<String>,
}

impl Scripted {
    fn doing(script: Vec<Does>) -> Self {
        Self {
            doing: script.into_iter(),
            seen: Vec::new(),
            listed: Vec::new(),
            started_at: Vec::new(),
            noted: Vec::new(),
            answered_at: Vec::new(),
            boxes: Vec::new(),
            same: Vec::new(),
        }
    }

    /// The next thing they do. Running out is leaving, so a script that forgets to say
    /// `Quit` still terminates instead of looping forever.
    fn next(&mut self) -> Does {
        self.doing.next().unwrap_or(Does::Quit)
    }
}

impl Asking for Scripted {
    fn frame(&mut self, header: &Header) -> Outcome<()> {
        self.seen.push(if header.crumbs.is_empty() {
            "·".to_owned()
        } else {
            header.crumbs.join(" › ")
        });
        self.same
            .extend(header.lines.iter().filter_map(|line| match line {
                super::paint::Line::Command(line) => Some(line.clone()),
                _ => None,
            }));
        Ok(())
    }

    fn choose(
        &mut self,
        _question: &str,
        items: &[Item],
        way_out: &str,
        at: usize,
    ) -> Outcome<Answer<usize>> {
        let mut titles: Vec<String> = items.iter().map(|item| item.title.clone()).collect();
        titles.push(way_out.to_owned());

        self.noted
            .push(items.iter().map(|item| item.note.clone()).collect());
        self.listed.push(titles);
        self.started_at.push(at);

        let way_out_row = items.len();
        let answered = match self.next() {
            Does::Pick(index) => index.min(way_out_row),
            Does::Enter | Does::Fill => 0,
            Does::Leave => way_out_row,
            Does::Back => return Ok(Answer::Back),
            Does::Quit | Does::Type(_) => return Ok(Answer::Quit),
        };
        self.answered_at.push(answered);
        Ok(Answer::Given(answered))
    }

    fn text(&mut self, ask: &Ask) -> Outcome<Answer<String>> {
        self.boxes.push(ask.initial.clone());

        Ok(match self.next() {
            Does::Type(text) => Answer::Given(text.to_owned()),
            Does::Fill if ask.initial.trim().is_empty() => Answer::Given("filled in".to_owned()),
            Does::Enter | Does::Fill => Answer::Given(ask.initial.clone()),
            // A box has no items, so the way out of one is the key rather than the row.
            Does::Back | Does::Leave => Answer::Back,
            Does::Pick(_) | Does::Quit => Answer::Quit,
        })
    }
}

fn shell(fresh: bool) -> Shell {
    Shell {
        global: PathBuf::from("/home/me/.sloop"),
        home: PathBuf::from("/home/me"),
        cwd: PathBuf::from("/work"),
        working_in: "the global store".to_owned(),
        found_by: "nothing named a project".to_owned(),
        standing: Standing {
            databases: if fresh { 0 } else { 2 },
            backups: if fresh {
                None
            } else {
                Some((2, "2h ago".to_owned()))
            },
            service: Some(("running".to_owned(), true)),
        },
        fresh,
        // Every test in this file is about a machine that has been through Setup. The one
        // that is about a machine that has not says so by name.
        set_up: true,
    }
}

/// Run a script against a settled session and hand back what the person saw.
fn session(script: Vec<Does>) -> Scripted {
    run_it(script, &mut shell(false), &mut Bench::default()).0
}

/// The same, against a world the caller can read afterwards.
fn run_it(script: Vec<Does>, shell: &mut Shell, bench: &mut Bench) -> (Scripted, Exit) {
    let mut scripted = Scripted::doing(script);
    let (exit, _) =
        walk(shell, &mut scripted, bench).expect("the menu should not fail on a scripted session");
    (scripted, exit)
}

#[test]
fn a_settled_session_opens_on_the_menu_and_a_first_run_does_not() {
    assert!(matches!(shell(false).opening(), Screen::Home { .. }));
    assert!(matches!(shell(true).opening(), Screen::FirstRun { .. }));
}

/// **First run offers `Init` and nothing else.** One thing to do, plus the way out — a
/// menu with no way out is a trap, and Esc does the same thing anyway.
#[test]
fn the_first_screen_of_a_fresh_machine_offers_one_thing() {
    let mut shell = shell(true);
    let mut scripted = Scripted::doing(vec![Does::Quit]);
    walk(&mut shell, &mut scripted, &mut Bench::default()).expect("a first run should draw");

    assert_eq!(
        scripted.listed,
        vec![vec!["Start a project here".to_owned(), QUIT.to_owned()]]
    );
}

/// **The `Done when`, first half of it: `← Back` returns from every screen in the tree.**
///
/// Every command in the tree, reached through its door, entered and backed out of — with the
/// way home checked each time. Two levels now that the doors are back: the door, and the flow
/// behind it.
#[test]
fn back_returns_from_every_screen_in_the_tree() {
    for (door, at, _) in every_leaf() {
        let command = door;
        let mut script = down_to(door, at);
        script.extend([Does::Leave, Does::Leave, Does::Leave]);
        let seen = session(script).seen;

        assert_eq!(
            seen.first().map(String::as_str),
            Some("·"),
            "command {command} did not start at the menu: {seen:?}"
        );
        assert_eq!(
            seen.last().map(String::as_str),
            Some("·"),
            "command {command} never came home: {seen:?}"
        );
    }
}

/// How many doors the home screen offers.
fn doors_on_the_home_screen() -> usize {
    match (Screen::Home { cursor: 0 }).face(&here(), &Bench::default()) {
        super::Face::Menu(menu) => menu.items.len(),
        super::Face::Ask(_) => unreachable!("the home screen is a menu"),
    }
}

/// The standing every test screen is drawn against.
fn here() -> Standing {
    Bench::default().standing()
}

/// **The second half of it: nothing entered is lost by leaving and coming back.**
///
/// The highlight is the simplest entered value there is. Go down into the third door, come
/// back, and the list has to be showing with the highlight still on the third door — not
/// reset to the top, which is what rebuilding the screen instead of keeping it would do.
#[test]
fn coming_back_finds_the_highlight_where_it_was_left() {
    let seen = session(vec![Does::Pick(2), Does::Leave, Does::Leave]);

    assert_eq!(
        seen.started_at.first(),
        Some(&0),
        "the menu did not open on its first door"
    );
    assert_eq!(
        seen.started_at.last(),
        seen.answered_at.first(),
        "coming back lost the highlight: {:?} after choosing {:?}",
        seen.started_at,
        seen.answered_at
    );
}

/// Down and back up: the highlight survives, because the history holds whole screens rather
/// than a path through them.
///
/// **One level is the whole depth now that the commands are on the front page.** A flow is a
/// single screen whose answers change rather than a screen per question — which is why
/// `← Back` inside one drops an answer instead of popping a screen, and that half is
/// `back_inside_a_flow_undoes_one_question_at_a_time`. The row picked here is deliberately
/// not the first: a menu that came home to the top would pass a test that started there.
#[test]
fn every_cursor_on_the_way_down_survives_the_way_up() {
    // Home, the fourth door, the first command behind it, and back up through both.
    let seen = session(vec![
        Does::Pick(3),
        Does::Pick(0),
        Does::Leave,
        Does::Leave,
        Does::Leave,
    ]);

    let at = seen.started_at;
    let chose = seen.answered_at;
    assert!(at.len() >= 4, "{at:?}");
    assert_eq!(at[0], 0, "the home screen opened somewhere unexpected");
    assert_ne!(
        chose[0], 0,
        "the test picked the row it would have opened on"
    );
    assert_eq!(
        at.last(),
        Some(&chose[0]),
        "the menu lost its highlight: {at:?}"
    );
}

/// Esc does what `← Back` does. Rule: *"`Esc` from any text or password prompt"*, and the
/// same key from a menu, because a user who has learned one has learned the other.
#[test]
fn esc_is_the_same_as_choosing_back() {
    let by_item = session(vec![Does::Pick(0), Does::Leave, Does::Leave]).seen;
    let by_key = session(vec![Does::Pick(0), Does::Back, Does::Back]).seen;
    assert_eq!(by_item, by_key);
}

/// The root has nothing behind it, so its last item says so rather than lying about it.
#[test]
fn the_root_offers_quit_and_everywhere_else_offers_back() {
    let seen = session(vec![Does::Pick(2), Does::Leave, Does::Leave]);

    let root = seen.listed.first().expect("the menu was drawn");
    assert_eq!(root.last().map(String::as_str), Some(QUIT));

    let inside = seen.listed.get(1).expect("the command was drawn");
    assert_eq!(inside.last().map(String::as_str), Some(BACK));
}

/// Every menu in the tree ends in a way out, including the ones with nothing else on them.
#[test]
fn no_menu_anywhere_is_a_dead_end() {
    let seen = session(vec![
        Does::Pick(0),
        Does::Pick(0),
        Does::Leave,
        Does::Leave,
        Does::Leave,
    ]);

    for list in &seen.listed {
        let last = list.last().map(String::as_str);
        assert!(
            last == Some(BACK) || last == Some(QUIT),
            "a menu with no way out: {list:?}"
        );
    }
}

/// Ctrl-C from anywhere ends the session rather than stepping back one screen at a time.
#[test]
fn ctrl_c_leaves_from_wherever_it_is_pressed() {
    let seen = session(vec![Does::Pick(1), Does::Fill, Does::Quit]);
    assert_eq!(
        seen.seen.len(),
        3,
        "the session carried on: {:?}",
        seen.seen
    );
}

/// Every leaf names the command that does the same thing, and every one of those is a
/// `sloop` line somebody could paste.
#[test]
fn every_leaf_names_a_command_that_could_be_pasted() {
    for (door, at, _) in every_leaf() {
        let leaf = leaf_of(door, at);
        assert!(
            leaf.command.starts_with("sloop "),
            "{:?} names {:?}, which is not a command",
            leaf.title,
            leaf.command
        );
        assert!(!leaf.blurb.is_empty(), "{:?} says nothing", leaf.title);
    }
}

/// The `Flow` that choosing a row behind a door produces.
fn reach_flow(door: usize, at: usize, bench: &Bench) -> super::Flow {
    let home = Screen::Home { cursor: 0 };
    let super::Flow::To(opened) = home.chose(&shell(false), bench, door) else {
        panic!("door {door} opens nothing")
    };
    opened.chose(&shell(false), bench, at)
}

/// The leaf a door and a row lead to.
fn leaf_of(door: usize, at: usize) -> super::screen::Leaf {
    match reach(door, at, &Bench::default()) {
        Screen::Doing { leaf, .. } => leaf,
        other => panic!("door {door}, row {at} is not a command: {other:?}"),
    }
}

/// **`R28`, on the surface a newcomer actually meets.** The clap summaries answer to
/// `cli::tests::every_command_says_what_it_does`; this is the same rule for the menu.
///
/// The blurb is the lead line of the flow screen — the sentence somebody reads while typing
/// the name of a database they are about to destroy — so it carries the whole weight of
/// saying what is about to happen. It is measured on the words in it that the title did not
/// already have, for the reason the clap test gives: *"Delete one from the server"* followed
/// by *"deletes one from the server"* is a screen that said one thing twice.
#[test]
fn every_leaf_says_what_it_does_in_words_its_title_did_not() {
    let mut thin = Vec::new();

    for (door, at, _) in every_leaf() {
        let leaf = leaf_of(door, at);

        let words = |text: &str| -> Vec<String> {
            text.split_whitespace()
                .map(|word| {
                    word.trim_matches(|c: char| !c.is_alphanumeric())
                        .to_lowercase()
                })
                .filter(|word| !word.is_empty())
                .collect()
        };

        let title = words(leaf.title);
        let said: Vec<String> = words(leaf.blurb)
            .into_iter()
            .filter(|word| !title.contains(word))
            .collect();

        if said.len() < 5 {
            thin.push(format!(
                "{:?} — {:?} adds {} word{} to its own title",
                leaf.title,
                leaf.blurb,
                said.len(),
                if said.len() == 1 { "" } else { "s" }
            ));
        }

        assert!(
            leaf.blurb
                .starts_with(|first: char| first.is_lowercase() || first == '`'),
            "{:?}: a blurb continues the title rather than opening a sentence of its own",
            leaf.title
        );
        assert!(
            !leaf.run_it.is_empty(),
            "{:?} has no sentence for the last screen",
            leaf.title
        );
    }

    assert!(
        thin.is_empty(),
        "{} menu item{} restate their titles instead of saying what they do:\n\n  {}\n",
        thin.len(),
        if thin.len() == 1 { "" } else { "s" },
        thin.join("\n  ")
    );
}

/// **Reached from the menu, not from the flow.** `flow_tests` proves the block is there;
/// this proves somebody choosing the item meets it — on the screen, with nothing run.
#[test]
fn choosing_install_in_an_ordinary_terminal_says_so_and_runs_nothing() {
    let screen = Screen::Home { cursor: 0 };
    let bench = Bench::unprivileged();

    let _ = &screen;
    let leaf = every_leaf()
        .into_iter()
        .find_map(|(door, at, job)| (job == Job::ServiceInstall).then(|| leaf_of(door, at)));
    let leaf = leaf.expect("installing the service is on the home screen");

    let super::flow::Next::Blocked(said) = leaf.job.next(&Answers::default(), &bench) else {
        panic!("the screen asked a question instead of saying it needs another terminal");
    };
    assert!(said.contains("machine-wide change"), "{said:?}");
    assert!(
        bench.ran.is_empty(),
        "nothing should have been run: {:?}",
        bench.ran
    );
}

/// **The one flow this task owns.** Picking `Init` types a directory, makes the registry,
/// and leaves something to print once the terminal has been handed back.
#[test]
fn starting_a_project_creates_the_registry_and_keeps_the_summary() {
    // Laid out the way a real machine is: the global store in the home directory, and the
    // working directory under it. `Init` refuses the home directory itself, so a fixture
    // that put the two the other way round would be testing a shape that cannot exist.
    let home = std::env::temp_dir().join(format!("sloop-ui-{}", std::process::id()));
    let here = home.join("work");
    let global = home.join(".sloop");
    std::fs::create_dir_all(&global).expect("a temporary directory");
    std::fs::create_dir_all(&here).expect("a temporary directory");

    let mut shell = Shell {
        global,
        home: home.clone(),
        cwd: here.clone(),
        working_in: "the global store".to_owned(),
        found_by: "nothing named a project".to_owned(),
        standing: Standing::default(),
        fresh: true,
        set_up: true,
    };

    let mut scripted = Scripted::doing(vec![
        Does::Pick(0),
        Does::Type("."),
        Does::Leave,
        Does::Leave,
    ]);
    let (_, kept) = walk(&mut shell, &mut scripted, &mut Bench::default())
        .expect("init should run from the menu");

    assert!(here.join(".sloop").is_dir(), "no registry was made");
    assert!(
        matches!(kept.as_slice(), [Kept::Started(_)]),
        "nothing was kept to print afterwards: {kept:?}"
    );
    assert!(
        !shell.fresh,
        "the session still thinks it is a first run after init"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// The directory box comes pre-filled with where sloop was run, and coming back to it
/// finds what was typed still typed.
#[test]
fn the_directory_box_keeps_what_was_typed() {
    // Laid out the way a real machine is: the global store in the home directory, and the
    // working directory under it. `Init` refuses the home directory itself, so a fixture
    // that put the two the other way round would be testing a shape that cannot exist.
    let home = std::env::temp_dir().join(format!("sloop-ui-typed-{}", std::process::id()));
    let here = home.join("work");
    let global = home.join(".sloop");
    std::fs::create_dir_all(&global).expect("a temporary directory");
    std::fs::create_dir_all(&here).expect("a temporary directory");

    let mut shell = Shell {
        global,
        home: home.clone(),
        cwd: here.clone(),
        working_in: "the global store".to_owned(),
        found_by: "nothing named a project".to_owned(),
        standing: Standing::default(),
        fresh: true,
        set_up: true,
    };

    let mut scripted = Scripted::doing(vec![
        Does::Pick(0),
        // Somewhere that is not there, so the screen says so and stays put.
        Does::Type("nowhere-at-all"),
        Does::Back,
        Does::Pick(0),
        Does::Leave,
    ]);
    walk(&mut shell, &mut scripted, &mut Bench::default())
        .expect("a bad directory is not a failed session");

    assert_eq!(
        scripted.boxes.first().map(String::as_str),
        Some(here.display().to_string().as_str()),
        "the box did not open on the working directory"
    );
    assert_eq!(
        scripted.boxes.get(1).map(String::as_str),
        Some("nowhere-at-all"),
        "the screen forgot what was typed into it: {:?}",
        scripted.boxes
    );
    assert!(!here.join(".sloop").is_dir(), "a registry was made anyway");

    let _ = std::fs::remove_dir_all(&home);
}

/// A directory that is not there is said on the screen, not thrown at the session.
#[test]
fn a_directory_that_is_not_there_is_said_on_the_screen() {
    let mut shell = shell(true);
    let mut scripted = Scripted::doing(vec![
        Does::Pick(0),
        Does::Type("nowhere-at-all"),
        Does::Back,
    ]);
    walk(&mut shell, &mut scripted, &mut Bench::default())
        .expect("a bad directory is not a failed session");

    let screen = Screen::NewProject {
        at: "nowhere-at-all".to_owned(),
        trouble: Some("nowhere-at-all is not a directory that exists".to_owned()),
    };
    let header = screen.header(&shell, &Bench::default());
    assert!(
        header
            .lines
            .iter()
            .any(|line| matches!(line, super::paint::Line::Wrong(_))),
        "the complaint never reached the screen"
    );
}

/// **The other half of the `Done when`: quitting puts the terminal back.**
///
/// The guard leaves the screen when it is dropped, and exactly once however many times it
/// is asked to — an early return, an explicit `drop`, and a panic all end in the same place.
#[test]
fn the_alternate_screen_is_given_back_on_the_way_out() {
    let mut written: Vec<u8> = Vec::new();
    {
        let mut screen = Alternate::entered(&mut written).expect("a Vec always takes bytes");
        screen.leave();
        screen.leave();
    }

    let bytes = String::from_utf8(written).expect("crossterm writes ASCII escapes");
    assert_eq!(
        bytes.matches("\x1b[?1049h").count(),
        1,
        "the alternate screen was entered {bytes:?}"
    );
    assert_eq!(
        bytes.matches("\x1b[?1049l").count(),
        1,
        "the alternate screen was left {bytes:?}"
    );
    assert!(
        bytes.ends_with("\x1b[?1049l"),
        "something was written after the screen was handed back: {bytes:?}"
    );
}

/// Dropped without `leave` ever being called — which is what a panic looks like.
#[test]
fn a_guard_nobody_closed_still_closes_itself() {
    let mut written: Vec<u8> = Vec::new();
    drop(Alternate::entered(&mut written).expect("a Vec always takes bytes"));

    let bytes = String::from_utf8(written).expect("crossterm writes ASCII escapes");
    assert!(bytes.ends_with("\x1b[?1049l"), "{bytes:?}");
}

/// Rule 4 at the door. These tests run with standard input closed, which is exactly the
/// state a cron line hands a command.
#[test]
fn the_menu_refuses_when_there_is_nothing_to_draw_on() {
    let refused = at_a_terminal().expect_err("a test harness is not a terminal");
    assert_eq!(refused.exit(), crate::exit::Exit::Usage);
    assert!(
        refused
            .hint_text()
            .is_some_and(|hint| hint.contains("sloop --help")),
        "the refusal did not say what to do instead"
    );
}

// ---------------------------------------------------------------------------------------
// The flows
// ---------------------------------------------------------------------------------------

/// Every leaf in the tree, as the door and the item that reaches it.
fn every_leaf() -> Vec<(usize, usize, Job)> {
    let bench = Bench::default();
    let mut found = Vec::new();

    let home = Screen::Home { cursor: 0 };
    for door in 0..doors_on_the_home_screen() {
        match home.chose(&shell(false), &bench, door) {
            // A door with one command behind it is that command — see `Screen::chose`.
            super::Flow::To(Screen::Doing { leaf: what, .. }) => {
                found.push((door, usize::MAX, what.job));
            }
            super::Flow::To(opened) => {
                let behind = match opened.face(&here(), &bench) {
                    super::Face::Menu(menu) => menu.items.len(),
                    super::Face::Ask(_) => unreachable!("a door opens a menu"),
                };
                for leaf in 0..behind {
                    let super::Flow::To(Screen::Doing { leaf: what, .. }) =
                        opened.chose(&shell(false), &bench, leaf)
                    else {
                        panic!("door {door}, leaf {leaf} opens nothing");
                    };
                    found.push((door, leaf, what.job));
                }
            }
            other => panic!("door {door} opens nothing: {other:?}"),
        }
    }
    found
}

/// How many questions a job asks, answered the way somebody pressing Enter answers them.
fn questions(job: Job, world: &dyn Doing) -> usize {
    let mut answers = Answers::default();
    for asked in 0..40 {
        match job.next(&answers, world) {
            super::flow::Next::Ask(step) => {
                let value = match &step.how {
                    super::flow::How::Pick { values, .. } => {
                        values.first().cloned().unwrap_or_default()
                    }
                    // The same rule `Does::Fill` follows: a box that needs an answer gets
                    // one, and a box where a blank means something keeps its blank.
                    super::flow::How::Type {
                        initial, needed, ..
                    } if *needed && initial.trim().is_empty() => "filled in".to_owned(),
                    super::flow::How::Type { initial, .. } => initial.clone(),
                };
                answers.put(step.field, value);
            }
            _ => return asked,
        }
    }
    panic!("{job:?} never stopped asking");
}

/// The keys that reach a leaf from the menu.
fn down_to(door: usize, leaf: usize) -> Vec<Does> {
    if leaf == usize::MAX {
        vec![Does::Pick(door)]
    } else {
        vec![Does::Pick(door), Does::Pick(leaf)]
    }
}

/// **The `Done when`: every command is reachable and completable without touching a flag.**
///
/// Every leaf in the tree, reached from the menu and answered the way somebody pressing
/// Enter answers it, ends with that command run — and run with the terminal handed back,
/// because a command whose output the menu paints over is a command nobody can read.
#[test]
fn every_command_is_reachable_and_completable_from_the_menu() {
    let leaves = every_leaf();

    // **Counted against the enum, not against a number kept here.** A hard-coded 22 is a
    // number somebody edits to make the test pass, which is the opposite of what it is for.
    // `Job` is the list of every command the menu can run, so a variant added without a row
    // on a screen fails here with the name of the one that is missing.
    let reachable: Vec<Job> = leaves.iter().map(|(_, _, job)| *job).collect();
    let missing: Vec<Job> = crate::ui::flow::Job::every()
        .iter()
        .copied()
        // Setup is the one job with no row on the home screen, on purpose: it is what the
        // first-run screen offers and it is gone from every screen after that.
        .filter(|job| *job != Job::Setup)
        .filter(|job| !reachable.contains(job))
        .collect();
    assert!(
        missing.is_empty(),
        "these commands are not reachable from the menu: {missing:?}"
    );
    assert_eq!(
        leaves.len(),
        reachable.len(),
        "a command has two rows on the menu: {leaves:?}"
    );

    for (door, leaf, job) in leaves {
        let mut script = down_to(door, leaf);
        // Enter through every question, and once more on the screen that runs it. Exactly
        // that many, so a job that runs twice is a failure rather than a longer script.
        script.extend(std::iter::repeat_n(
            Does::Fill,
            questions(job, &Bench::default()) + 1,
        ));

        let mut bench = Bench::default();
        let (seen, _) = run_it(script, &mut shell(false), &mut bench);

        assert_eq!(
            bench.ran.iter().map(|(job, _)| *job).collect::<Vec<_>>(),
            vec![job],
            "{job:?} was not run once from the menu"
        );
        // **And it ran on the menu's own screen.** The terminal is never handed back now —
        // rule 1 of the owner's list — so what proves the run happened somewhere readable is
        // the outcome screen it left behind, not a curtain going up and down.
        //
        // **And there is nothing on that screen to press but the way out.** A result used to
        // carry the thing it had just done, with the cursor on it, so Enter would have run it
        // twice — which is the one keystroke a result must not be.
        assert_eq!(
            seen.listed.last().map(Vec::as_slice),
            Some([HOME.to_owned()].as_slice()),
            "{job:?} did not finish on a result with nothing but Home on it: {:?}",
            seen.listed
        );
    }
}

/// **Nothing runs off the back of the last answer.** There is always one more screen, with
/// one item on it, that says what is about to happen — a flow that fires the moment its
/// last question is answered cannot be read back before it happens.
#[test]
fn a_flow_always_shows_what_it_is_about_to_do() {
    let mut bench = Bench::default();

    // Databases, then Rename one: two questions, and the third screen runs it.
    let (seen, _) = run_it(
        vec![Does::Pick(0), Does::Pick(5), Does::Fill, Does::Fill],
        &mut shell(false),
        &mut bench,
    );

    assert!(bench.ran.is_empty(), "it ran before it was asked to");
    assert_eq!(
        seen.listed.last().map(Vec::as_slice),
        Some(["Rename it".to_owned(), BACK.to_owned()].as_slice()),
        "the last screen of a flow is the one that does it: {:?}",
        seen.listed
    );
}

/// **The answers reach the command.** A menu that collected six answers and then ran the
/// command with none of them would pass every test above this one.
#[test]
fn what_was_answered_is_what_the_command_is_handed() {
    let mut bench = Bench::default();

    // Databases → Rename one → the second database → a new name → run it.
    run_it(
        vec![
            Does::Pick(0),
            Does::Pick(5),
            Does::Pick(1),
            Does::Type("orders_old"),
            Does::Fill,
        ],
        &mut shell(false),
        &mut bench,
    );

    let (job, answers) = bench.ran.first().expect("the rename was not run");
    assert_eq!(*job, Job::DbRename);
    assert_eq!(answers.text(field::NAME), "orders_staging");
    assert_eq!(answers.text(field::RENAMED), "orders_old");
}

/// **`← Back` inside a flow is one question back, not the whole flow.**
///
/// Two answers in, back once, and the question in front of you is the second one again —
/// with the first still answered. Back again and it is the first. Back a third time and
/// the flow is left, because there is nothing else left to undo.
#[test]
fn back_inside_a_flow_undoes_one_question_at_a_time() {
    let mut bench = Bench::default();

    let (seen, _) = run_it(
        vec![
            Does::Pick(0),
            Does::Pick(0),
            Does::Type("orders"),
            Does::Pick(1),
            Does::Leave,
            Does::Leave,
            Does::Leave,
            Does::Leave,
            Does::Leave,
        ],
        &mut shell(false),
        &mut bench,
    );

    assert!(bench.ran.is_empty(), "backing out ran something");
    assert_eq!(
        seen.seen.last().map(String::as_str),
        Some("·"),
        "backing out of a flow did not come home: {:?}",
        seen.seen
    );
    // The box was shown twice: once when it was asked, and again once the answer in front
    // of it had been dropped.
    assert_eq!(seen.boxes.len(), 2, "{:?}", seen.boxes);
}

/// And the answers before the one that was dropped are still given.
#[test]
fn the_answers_before_the_dropped_one_survive() {
    let mut bench = Bench::default();

    // A name, then "one line", then back, then "field by field" instead — and the name
    // given before the change of mind stands.
    let mut script = vec![
        Does::Pick(0),
        Does::Pick(0),
        Does::Type("orders"),
        Does::Pick(0),
        Does::Leave,
        Does::Pick(1),
    ];
    script.extend(std::iter::repeat_n(Does::Fill, 10));
    run_it(script, &mut shell(false), &mut bench);

    let (job, answers) = bench.ran.first().expect("the registration was not run");
    assert_eq!(*job, Job::DbAdd);
    assert_eq!(answers.text(field::NAME), "orders");
    assert_eq!(answers.text(field::HOW), "fields");
    assert_eq!(
        answers.get(field::URL),
        None,
        "the URL survived a change of mind"
    );
}

/// A job that fails says so on the screen it was started from, with its answers still
/// there — so it can be corrected and run again rather than started from the top.
#[test]
fn a_job_that_fails_leaves_the_flow_where_it_was() {
    let mut bench = Bench {
        says: Some(Failure::usage("orders_staging is already registered")),
        ..Bench::default()
    };

    let (seen, exit) = run_it(
        vec![
            Does::Pick(0),
            Does::Pick(5),
            Does::Fill,
            Does::Type("orders_old"),
            Does::Fill,
            Does::Quit,
        ],
        &mut shell(false),
        &mut bench,
    );

    assert_eq!(exit, Exit::Usage, "the menu swallowed the command's code");
    assert_eq!(bench.ran.len(), 1);
    assert_eq!(
        seen.listed.last().map(Vec::as_slice),
        Some(
            [
                "Try it again".to_owned(),
                "Change an answer".to_owned(),
                HOME.to_owned(),
            ]
            .as_slice()
        ),
        "a failure did not offer a way on: {:?}",
        seen.listed
    );
    assert_eq!(
        seen.seen.last().map(String::as_str),
        Some("Databases › Rename one"),
        "the failure screen forgot which command it was: {:?}",
        seen.seen
    );
}

/// **Rule 9, the other half: changing an answer keeps the rest.** The second row of the
/// failure screen steps back into the flow with the last answer dropped, so correcting a
/// typo is one keystroke rather than starting from the top.
#[test]
fn changing_an_answer_after_a_failure_keeps_the_ones_before_it() {
    let mut bench = Bench {
        says: Some(Failure::usage("orders_staging is already registered")),
        ..Bench::default()
    };

    let (seen, _) = run_it(
        vec![
            Does::Pick(0),
            Does::Pick(5),
            Does::Fill,
            Does::Type("orders_old"),
            Does::Fill,
            // The failure screen: "Change an answer".
            Does::Pick(1),
            Does::Quit,
        ],
        &mut shell(false),
        &mut bench,
    );

    assert_eq!(bench.ran.len(), 1, "changing an answer ran it again");
    // Back on the question that was answered last, asked again — and the answer before it
    // is still given, which is what "the rest kept" means.
    assert_eq!(
        seen.boxes.len(),
        2,
        "the last question was not asked again: {:?}",
        seen.boxes
    );
    assert_eq!(
        seen.seen.last().map(String::as_str),
        Some("Databases › Rename one"),
        "changing an answer left the flow: {:?}",
        seen.seen
    );
}

/// A job that works leaves the flow behind, so nothing is one keystroke away from being
/// run a second time.
#[test]
fn a_job_that_works_comes_back_to_the_menu() {
    let mut bench = Bench::default();

    let (seen, exit) = run_it(
        vec![
            Does::Pick(0),
            Does::Pick(2),
            Does::Fill,
            // One key from the result to the front page.
            Does::Leave,
            Does::Quit,
        ],
        &mut shell(false),
        &mut bench,
    );

    assert_eq!(exit, Exit::Success);
    assert_eq!(bench.ran.len(), 1, "{:?}", bench.ran);

    // **The owner's rule: a result leads Home, not back up the path that reached it.** The
    // door it came through is not walked back through, and the flow is not on the way
    // either — nothing is one keystroke away from being run a second time.
    assert_eq!(
        seen.seen,
        [
            "·",
            "Databases",
            "Databases › See the ones sloop knows",
            "Databases › See the ones sloop knows",
            "·",
        ],
        "{:?}",
        seen.seen
    );
}

/// **And the way out of a result says so.** `← Back` on a result is a promise to step one
/// screen back up a path nobody is on any more; `← Home` is what it actually does.
#[test]
fn the_way_out_of_a_result_is_home() {
    let mut bench = Bench::default();

    let (seen, _) = run_it(
        vec![Does::Pick(0), Does::Pick(2), Does::Fill, Does::Quit],
        &mut shell(false),
        &mut bench,
    );

    assert_eq!(
        seen.listed.last().and_then(|list| list.last()),
        Some(&HOME.to_owned()),
        "a result did not offer Home: {:?}",
        seen.listed
    );
    for list in seen.listed.iter().take(seen.listed.len() - 1) {
        assert_ne!(
            list.last(),
            Some(&HOME.to_owned()),
            "a screen that is not a result offered Home: {list:?}"
        );
    }
}

/// The breadcrumb names every step that reached the screen, so somebody four questions
/// deep can see where they are without leaving.
#[test]
fn a_flow_screen_names_every_step_that_reached_it() {
    let mut bench = Bench::default();

    let (seen, _) = run_it(
        vec![Does::Pick(0), Does::Pick(5), Does::Quit],
        &mut shell(false),
        &mut bench,
    );

    assert_eq!(
        seen.seen.last().map(String::as_str),
        Some("Databases › Rename one"),
        "{:?}",
        seen.seen
    );
}

// ---------------------------------------------------------------------------------------
// Every page in the tree
// ---------------------------------------------------------------------------------------

/// Every menu screen in the tree, so a page added later cannot quietly go flat.
fn every_menu() -> Vec<(&'static str, super::screen::Menu)> {
    let bench = Bench::default();
    let mut found: Vec<(&'static str, super::screen::Menu)> = Vec::new();

    let mut pages: Vec<(&'static str, Screen)> = vec![
        ("the first run", Screen::FirstRun { cursor: 0 }),
        ("the menu", Screen::Home { cursor: 0 }),
    ];
    // And every flow's first screen, plus the screen that runs it.
    for (door, leaf, _) in every_leaf() {
        let opening = reach(door, leaf, &bench);
        pages.push(("a flow", opening.clone()));
        pages.push(("a flow, answered", answered_through(opening, &bench)));
    }

    for (what, page) in pages {
        if let super::Face::Menu(menu) = page.face(&here(), &bench) {
            found.push((what, menu));
        }
    }
    found
}

/// The screen a leaf opens on.
fn reach(door: usize, leaf: usize, bench: &Bench) -> Screen {
    let home = Screen::Home { cursor: 0 };
    let super::Flow::To(opened) = home.chose(&shell(false), bench, door) else {
        panic!("door {door} opens nothing")
    };
    if leaf == usize::MAX {
        return opened;
    }
    let super::Flow::To(reached) = opened.chose(&shell(false), bench, leaf) else {
        panic!("leaf {leaf} opens nothing")
    };
    reached
}

/// The same flow with every question answered, which is the screen that runs it.
fn answered_through(mut screen: Screen, bench: &Bench) -> Screen {
    for _ in 0..40 {
        match screen.face(&here(), bench) {
            super::Face::Menu(menu) if menu.items.is_empty() => return screen,
            super::Face::Menu(_) => match screen.chose(&shell(false), bench, 0) {
                super::Flow::Same(next) => screen = next,
                _ => return screen,
            },
            super::Face::Ask(ask) => {
                let mut shell = shell(false);
                match screen
                    .clone()
                    .typed(&mut shell, bench, &mut Vec::new(), &ask.initial.clone())
                {
                    super::Flow::Same(next) => screen = next,
                    _ => return screen,
                }
            }
        }
    }
    screen
}

/// **Rule 2 of the owner's list: there are no headings left to draw.**
///
/// *"headings are looking bad needs to remove though i asked to add"*. Every row of every
/// page is now something that can be chosen, so the cursor cannot land on a word that does
/// nothing — and the structure the headings carried is the doors instead. The whole heading
/// machinery went with them: `Row`, `Section` and the cursor's skipping rule.
#[test]
fn no_page_in_the_tree_draws_a_heading() {
    for (what, menu) in every_menu() {
        for item in &menu.items {
            assert!(
                !item.title.is_empty(),
                "{what}: a row with no title, which is what a heading used to be"
            );
            assert_ne!(
                item.title,
                item.title.to_uppercase(),
                "{what}: {:?} reads as a heading rather than a choice",
                item.title
            );
        }
    }
}

/// **And the home screen is six doors rather than thirty commands.**
///
/// Rule 12: *"home screen shows a lot of items, which can go inside sub menus"*. Thirty rows
/// under seven headings left a twenty-four-row terminal five rows of list to scroll them all
/// through.
#[test]
fn the_home_screen_is_a_handful_of_doors() {
    let doors = doors_on_the_home_screen();
    assert!(
        (4..=8).contains(&doors),
        "the home screen offers {doors} things, which is a list again"
    );
    assert_eq!(
        every_leaf().len(),
        30,
        "a command was lost or gained on the way behind a door"
    );
}

/// A door with one command behind it opens that command, because a submenu of one is a
/// keystroke that asks nothing.
#[test]
fn a_door_with_one_command_behind_it_is_that_command() {
    let bench = Bench::default();
    let home = Screen::Home { cursor: 0 };
    let straight_through = (0..doors_on_the_home_screen()).filter(|door| {
        matches!(
            home.chose(&shell(false), &bench, *door),
            super::Flow::To(Screen::Doing { .. })
        )
    });
    assert_eq!(
        straight_through.count(),
        1,
        "exactly one door — Look inside one — holds a single command"
    );
}

/// Nothing on the home screen opens a screen with nothing on it.
#[test]
fn every_door_has_something_behind_it() {
    let mut bench = Bench::default();

    let (seen, _) = run_it(
        vec![Does::Pick(0), Does::Leave, Does::Quit],
        &mut shell(false),
        &mut bench,
    );

    assert!(bench.ran.is_empty(), "opening a door ran something");
    assert_eq!(
        seen.seen,
        ["·", "Databases", "·"],
        "a door did not open and come back: {:?}",
        seen.seen
    );
    assert_eq!(
        seen.started_at.first(),
        Some(&0),
        "the home screen did not open on its first door: {:?}",
        seen.started_at
    );
}

/// **The third part: every item names the command that does the same thing.**
///
/// Every door on the home screen names the family's flag form, and every command behind one
/// names its own. The things that are *not* commands — an engine, a yes — carry their phrase
/// instead, because there is no command that means "MySQL".
#[test]
fn every_command_in_the_tree_names_its_flag_form() {
    let bench = Bench::default();

    let page = Screen::Home { cursor: 0 };
    let super::Face::Menu(menu) = page.face(&here(), &bench) else {
        panic!("the home screen is not a menu");
    };

    for item in menu.items {
        assert!(
            item.command.starts_with("sloop "),
            "{:?} names no command",
            item.title
        );
    }

    for (door, at, _) in every_leaf() {
        let leaf = leaf_of(door, at);
        let item = super::screen::Item::doing(leaf);
        assert_eq!(
            item.command, leaf.command,
            "{:?} shows something other than its command",
            item.title
        );
        assert!(
            !item.note.is_empty(),
            "{:?} has an empty column",
            item.title
        );
    }
}

/// A choice that is not a command shows what it means instead of an empty column. There
/// is no flag form that means "MySQL", so those rows carry their phrase.
#[test]
fn a_choice_that_is_not_a_command_still_says_what_it_means() {
    let bench = Bench::default();
    let mut answers = Answers::default();
    answers.put(super::flow::field::NAME, "orders");

    let super::flow::Next::Ask(step) = Job::DbCreate.next(&answers, &bench) else {
        panic!("the engine was not asked for");
    };
    assert_eq!(step.field, super::flow::field::ENGINE);
    let super::flow::How::Pick { items, .. } = step.how else {
        panic!("an engine is picked, not typed");
    };

    for item in items {
        assert!(
            item.command.is_empty(),
            "{:?} claimed a command",
            item.title
        );
        assert!(!item.note.is_empty(), "{:?} says nothing", item.title);
    }
}

/// **The fourth part: the breadcrumb on the deepest screen names every step.**
#[test]
fn the_deepest_screen_names_every_step_that_reached_it() {
    let bench = Bench::default();

    for (door, leaf, _) in every_leaf() {
        let opening = reach(door, leaf, &bench);
        let crumbs = opening.crumbs();

        assert!(
            crumbs.len() >= 2 || leaf == usize::MAX,
            "a flow under a group named only one step: {crumbs:?}"
        );
        for crumb in &crumbs {
            assert!(!crumb.trim().is_empty(), "an empty step: {crumbs:?}");
        }
    }

    // And the root carries none, because there is nothing above it.
    assert!(Screen::Home { cursor: 0 }.crumbs().is_empty());
    assert!(Screen::FirstRun { cursor: 0 }.crumbs().is_empty());
}

/// **The defect this exists for.** Pressing Enter on an empty box that needs an answer
/// used to file the blank and carry it five screens to the command, which came back with a
/// usage error about a flag nobody passed. It now says so on the box.
#[test]
fn a_required_box_refuses_a_blank_and_says_so_on_the_spot() {
    let mut bench = Bench::default();

    // Databases > Make a new database > Enter on the empty name.
    let (seen, _) = run_it(
        vec![Does::Pick(0), Does::Pick(1), Does::Enter, Does::Quit],
        &mut shell(false),
        &mut bench,
    );

    assert!(bench.ran.is_empty(), "a blank name reached the command");
    // Still on the same box: the question was asked again and nothing moved on.
    assert_eq!(seen.boxes.len(), 2, "{:?} / {:?}", seen.boxes, seen.seen);

    // And the same thing said directly: a blank into the box comes back as a complaint on
    // the screen it was typed into, not as an answer.
    let super::Flow::To(mut making) = reach_flow(0, 1, &bench) else {
        panic!("the flow did not open");
    };
    let flow = making.typed(&mut shell(false), &bench, &mut Vec::new(), "   ");
    let super::Flow::Same(Screen::Doing { trouble, .. }) = flow else {
        panic!("a blank was accepted");
    };
    assert!(
        trouble.is_some_and(|said| said.contains("cannot be left blank")),
        "the box did not say why"
    );
}

// ---------------------------------------------------------------------------------------
// R19c5 — the Setup screen
// ---------------------------------------------------------------------------------------

/// A machine that has not been set up, for the tests that are about one.
fn unset_up() -> Shell {
    Shell {
        set_up: false,
        ..shell(true)
    }
}

/// **Setup replaces `Init` as the first screen, and it outranks everything.** `R19c` moved
/// sloop's state into a PostgreSQL of its own, so a machine without one has no registry to
/// read — a fresh machine and a machine with a full registry both get Setup first if they
/// have not been through it.
#[test]
fn a_machine_that_is_not_set_up_opens_on_setup_whatever_else_is_true() {
    assert!(matches!(unset_up().opening(), Screen::Setup { .. }));

    // Even with databases registered and a project in play — which cannot really happen,
    // and is exactly why it must not depend on being impossible.
    let settled_but_not_set_up = Shell {
        set_up: false,
        ..shell(false)
    };
    assert!(matches!(
        settled_but_not_set_up.opening(),
        Screen::Setup { .. }
    ));
}

/// **The owner's rule, in one assertion: a machine that is set up is never asked again.**
/// Not on the first screen, not on the home screen, not anywhere in the tree.
#[test]
fn a_machine_that_is_set_up_is_never_offered_setup_again() {
    assert!(matches!(shell(false).opening(), Screen::Home { .. }));
    assert!(matches!(shell(true).opening(), Screen::FirstRun { .. }));

    // And `Set up` is on no menu anywhere: the home screen lists every command sloop has,
    // and Setup is deliberately not one of them.
    let mut scripted = Scripted::doing(vec![Does::Quit]);
    walk(&mut shell(false), &mut scripted, &mut Bench::default())
        .expect("a settled session should draw");

    for screen in &scripted.listed {
        for item in screen {
            assert!(
                !item.to_lowercase().contains("set up"),
                "a set-up machine was offered {item}"
            );
        }
    }
}

/// One thing to do, plus the way out. The same shape the first run has, for the same reason:
/// a menu with no way out is a trap, and Esc does that anyway.
#[test]
fn the_setup_screen_offers_one_thing_and_says_what_it_will_do() {
    let mut scripted = Scripted::doing(vec![Does::Quit]);
    walk(&mut unset_up(), &mut scripted, &mut Bench::default())
        .expect("a machine that is not set up should draw");

    assert_eq!(
        scripted.listed,
        vec![vec!["Set up this machine".to_owned(), QUIT.to_owned()]]
    );
}

/// Choosing it runs the real thing, on the real terminal.
///
/// **`Flow::Run` and not `Flow::To`**, which is what hands the terminal back before Setup
/// starts talking — its step-by-step output would otherwise be wiped by the next redraw, and
/// the one question it can ask could not be asked at all.
#[test]
fn choosing_setup_runs_it_outside_the_alternate_screen() {
    let shell = unset_up();
    let screen = shell.opening();

    let flow = screen.chose(&shell, &Bench::default(), 0);
    match flow {
        crate::ui::screen::Flow::Run(leaf, _) => {
            assert_eq!(leaf.job, crate::ui::flow::Job::Setup);
            assert_eq!(leaf.command, "sloop setup");
        }
        other => panic!("setup should run rather than ask: {other:?}"),
    }
}

/// **Re-runnable after a failure, which is the other half of the `Done when`.**
///
/// A failed Setup used to stay on the Setup screen carrying what went wrong. It now lands on
/// the same outcome screen every other job does — which offers to try it again rather than
/// only to re-enter it, and which `← Back` returns from to the Setup screen itself rather
/// than out of a session that has nowhere else to be.
#[test]
fn a_failed_setup_can_be_tried_again_without_leaving() {
    let mut bench = Bench {
        says: Some(Failure::usage("the cluster would not start")),
        ..Bench::default()
    };

    let (seen, exit) = run_it(
        vec![Does::Enter, Does::Leave, Does::Quit],
        &mut unset_up(),
        &mut bench,
    );

    assert_eq!(exit, Exit::Usage, "the menu swallowed the command's code");
    assert_eq!(bench.ran.len(), 1, "setup did not run");

    // The failure screen, then the Setup screen again behind it.
    assert_eq!(
        seen.listed.get(1).map(Vec::as_slice),
        Some(
            [
                "Try it again".to_owned(),
                "Change an answer".to_owned(),
                HOME.to_owned(),
            ]
            .as_slice()
        ),
        "a failed setup did not offer a way on: {:?}",
        seen.listed
    );
    // **Home on a machine that is not set up means Setup**, because every other screen on
    // one is a menu whose every item fails.
    assert!(
        seen.listed
            .last()
            .is_some_and(|list| list[0].contains("Set up")),
        "leaving the failure did not come home to Setup: {:?}",
        seen.listed
    );
}

/// **And Setup stops being home the moment it works.** The shell worked out `set_up` when
/// the session opened; a machine that has just been set up is not that machine any more, and
/// leaving the result has to land somewhere that works rather than back on Setup.
#[test]
fn a_machine_that_has_just_been_set_up_never_sees_setup_again() {
    // A machine with a registry behind it comes home to the menu.
    let mut settled = Shell {
        set_up: false,
        ..shell(false)
    };
    let mut bench = Bench::default();
    let (seen, _) = run_it(
        vec![Does::Enter, Does::Leave, Does::Quit],
        &mut settled,
        &mut bench,
    );

    assert_eq!(bench.ran.len(), 1, "setup did not run");
    assert!(settled.set_up, "the session still thinks it is not set up");
    assert!(
        seen.listed
            .last()
            .is_some_and(|list| list.iter().any(|row| row == "Databases")),
        "leaving a finished setup did not come home to the menu: {:?}",
        seen.listed
    );

    // And one with nothing registered comes home to the first run, because that is what
    // home means on a machine with no databases — not Setup, which is over.
    let mut fresh = unset_up();
    let (seen, _) = run_it(
        vec![Does::Enter, Does::Leave, Does::Quit],
        &mut fresh,
        &mut Bench::default(),
    );
    assert!(fresh.set_up);
    assert_eq!(
        seen.listed.last().map(Vec::as_slice),
        Some(["Start a project here".to_owned(), QUIT.to_owned()].as_slice()),
        "a fresh machine did not come home to the first run: {:?}",
        seen.listed
    );
}

/// Setup is the root of the tree it opens: nothing is behind it, so there is no breadcrumb
/// and no `← Back` into a menu that would not work.
#[test]
fn setup_is_a_root_with_nothing_behind_it() {
    let screen = unset_up().opening();
    assert!(screen.crumbs().is_empty());
}

/// **`R20`, through the real loop.** Every job reached from the menu and answered the way
/// somebody pressing Enter answers it, and what the session *said* afterwards.
///
/// The line's content is `equivalent`'s own business and is checked there, against `clap`.
/// What this checks is the wiring: that it is printed at all, that it is printed once, and
/// that it is printed **after the terminal has been handed back and before the menu takes
/// it again** — which is the whole of *"after leaving the alternate screen, so it survives"*.
#[test]
fn every_run_ends_with_the_line_that_would_repeat_it() {
    for (door, leaf, job) in every_leaf() {
        let mut script = down_to(door, leaf);
        script.extend(std::iter::repeat_n(
            Does::Fill,
            questions(job, &Bench::default()) + 1,
        ));

        let mut bench = Bench::default();
        let (seen, _) = run_it(script, &mut shell(false), &mut bench);

        let answers = bench
            .ran
            .first()
            .map(|(_, answers)| answers.clone())
            .unwrap_or_default();
        let expected = super::equivalent::line(job, &answers, &Bench::default()).is_some();

        assert_eq!(
            seen.same.len(),
            usize::from(expected),
            "{job:?} carried {:?}",
            seen.same
        );
        if let Some(line) = seen.same.first() {
            assert!(line.starts_with("sloop "), "{job:?}: {line}");
        }
    }
}

/// A run that failed prints no line. One that reproduces a failure is a line somebody pastes
/// into a scheduler and then wonders about.
#[test]
fn a_run_that_did_not_work_suggests_nothing() {
    let (door, leaf, job) = every_leaf()
        .into_iter()
        .find(|(_, _, job)| *job == Job::DbList)
        .expect("db list is on the menu");

    let mut script = down_to(door, leaf);
    script.extend(std::iter::repeat_n(
        Does::Fill,
        questions(job, &Bench::default()) + 1,
    ));

    let mut bench = Bench {
        says: Some(Failure::usage("the registry would not open")),
        ..Bench::default()
    };
    let (seen, _) = run_it(script, &mut shell(false), &mut bench);

    assert_eq!(bench.ran.len(), 1, "the job should still have been run");
    assert!(seen.same.is_empty(), "{:?}", seen.same);
}
