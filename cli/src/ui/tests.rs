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
use super::screen::{Ask, Group, Item, Kept, Screen, Shell};
use super::{Alternate, BACK, QUIT, Stage, at_a_terminal, walk};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// A world with two databases and a backup of each, which runs nothing and remembers what
/// it was asked to run.
#[derive(Default)]
struct Bench {
    /// Every job that was run, with the answers it was handed.
    ran: Vec<(Job, Answers)>,
    /// What the next job should come back as.
    says: Option<Failure>,
}

impl Doing for Bench {
    fn databases(&self) -> Vec<String> {
        vec!["orders".to_owned(), "orders_staging".to_owned()]
    }

    fn backups_of(&self, _name: &str) -> Vec<String> {
        vec!["20260916T031500Z".to_owned()]
    }

    fn run(&mut self, job: Job, answers: &Answers) -> Outcome<Exit> {
        self.ran.push((job, answers.clone()));
        self.says.take().map_or(Ok(Exit::Success), Err)
    }
}

/// The alternate screen, counted rather than drawn.
#[derive(Default)]
struct Curtain {
    /// How many times the menu handed the terminal back.
    out: usize,
    /// How many times it took it again.
    back: usize,
    /// And how many times it waited for somebody to finish reading.
    paused: usize,
}

impl Stage for Curtain {
    fn step_out(&mut self) {
        self.out += 1;
    }

    fn pause(&mut self) {
        self.paused += 1;
    }

    fn step_in(&mut self) {
        self.back += 1;
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
    /// Whatever is in front of them, accepted: the first item, or the box as it stands.
    /// What somebody pressing Enter through a flow does.
    Enter,
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
    /// Every box shown, as what was already in it.
    boxes: Vec<String>,
}

impl Scripted {
    fn doing(script: Vec<Does>) -> Self {
        Self {
            doing: script.into_iter(),
            seen: Vec::new(),
            listed: Vec::new(),
            started_at: Vec::new(),
            boxes: Vec::new(),
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
        let last = titles.len() - 1;
        self.listed.push(titles);
        self.started_at.push(at);

        Ok(match self.next() {
            Does::Pick(index) => Answer::Given(index),
            Does::Enter => Answer::Given(0),
            Does::Leave => Answer::Given(last),
            Does::Back => Answer::Back,
            Does::Quit | Does::Type(_) => Answer::Quit,
        })
    }

    fn text(&mut self, ask: &Ask) -> Outcome<Answer<String>> {
        self.boxes.push(ask.initial.clone());

        Ok(match self.next() {
            Does::Type(text) => Answer::Given(text.to_owned()),
            Does::Enter => Answer::Given(ask.initial.clone()),
            // A box has no items, so the way out of one is the key rather than the row.
            Does::Back | Does::Leave => Answer::Back,
            Does::Pick(_) | Does::Quit => Answer::Quit,
        })
    }
}

fn shell(fresh: bool) -> Shell {
    Shell {
        global: PathBuf::from("/global"),
        cwd: PathBuf::from("/work"),
        working_in: "the global store".to_owned(),
        found_by: "nothing named a project".to_owned(),
        holds: if fresh { 0 } else { 2 },
        fresh,
    }
}

/// Run a script against a settled session and hand back what the person saw.
fn session(script: Vec<Does>) -> Scripted {
    run_it(
        script,
        &mut shell(false),
        &mut Bench::default(),
        &mut Curtain::default(),
    )
    .0
}

/// The same, against a world and a stage the caller can read afterwards.
fn run_it(
    script: Vec<Does>,
    shell: &mut Shell,
    bench: &mut Bench,
    curtain: &mut Curtain,
) -> (Scripted, Exit) {
    let mut scripted = Scripted::doing(script);
    let (exit, _) = walk(shell, &mut scripted, bench, curtain)
        .expect("the menu should not fail on a scripted session");
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
    walk(
        &mut shell,
        &mut scripted,
        &mut Bench::default(),
        &mut Curtain::default(),
    )
    .expect("a first run should draw");

    assert_eq!(
        scripted.listed,
        vec![vec!["Start a project here".to_owned(), QUIT.to_owned()]]
    );
}

/// **The `Done when`, first half of it: `← Back` returns from every screen in the tree.**
///
/// Every door on the home screen, and every leaf behind every door, entered and backed out
/// of — with the highlight checked on the way home each time.
#[test]
fn back_returns_from_every_screen_in_the_tree() {
    let home = Screen::Home { cursor: 0 };
    let doors = match home.face(&Bench::default()) {
        super::Face::Menu(menu) => menu.items.len(),
        super::Face::Ask(_) => unreachable!("the home screen is a menu"),
    };

    for door in 0..doors {
        let mut script = vec![Does::Pick(door)];

        // A group has leaves of its own; a leaf on the home screen has none.
        let leaves = leaves_behind(door);
        for leaf in 0..leaves {
            script.push(Does::Pick(leaf));
            script.push(Does::Leave);
        }

        script.push(Does::Leave);
        script.push(Does::Leave);

        let seen = session(script).seen;
        assert_eq!(
            seen.first().map(String::as_str),
            Some("·"),
            "door {door} did not start at the menu: {seen:?}"
        );
        assert_eq!(
            seen.last().map(String::as_str),
            Some("·"),
            "door {door} never came home: {seen:?}"
        );
    }
}

/// How many leaves sit behind door `index` on the home screen.
fn leaves_behind(index: usize) -> usize {
    let home = Screen::Home { cursor: 0 };
    match home.chose(&shell(false), &Bench::default(), index) {
        super::Flow::To(Screen::Group { group, .. }) => {
            match (Screen::Group { group, cursor: 0 }).face(&Bench::default()) {
                super::Face::Menu(menu) => menu.items.len(),
                super::Face::Ask(_) => 0,
            }
        }
        _ => 0,
    }
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
        "the menu did not open at the top"
    );
    assert_eq!(
        seen.started_at.last(),
        Some(&2),
        "coming back lost the highlight: {:?}",
        seen.started_at
    );
}

/// Two levels down and back up: both cursors survive, because the history holds whole
/// screens rather than a path through them.
#[test]
fn every_cursor_on_the_way_down_survives_the_way_up() {
    let seen = session(vec![
        Does::Pick(1),
        Does::Pick(3),
        Does::Leave,
        Does::Leave,
        Does::Leave,
    ]);

    // Five screens: the menu, the group, the leaf, and then both of them again on the way
    // back up. The last two are the ones that matter.
    let at = seen.started_at;
    assert_eq!(at.len(), 5, "{at:?}");
    assert_eq!(at[..3], [0, 0, 0], "a screen opened somewhere but the top");
    assert_eq!(at[3], 3, "the group lost its highlight: {at:?}");
    assert_eq!(at[4], 1, "the menu lost its highlight: {at:?}");
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
    let seen = session(vec![Does::Pick(0), Does::Leave, Does::Leave]);

    let root = seen.listed.first().expect("the menu was drawn");
    assert_eq!(root.last().map(String::as_str), Some(QUIT));

    let inside = seen.listed.get(1).expect("the group was drawn");
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
    let seen = session(vec![Does::Pick(0), Does::Pick(1), Does::Quit]);
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
    for group in Group::ALL {
        let screen = Screen::Group {
            group: *group,
            cursor: 0,
        };
        let count = match screen.face(&Bench::default()) {
            super::Face::Menu(menu) => menu.items.len(),
            super::Face::Ask(_) => 0,
        };

        for index in 0..count {
            let super::Flow::To(Screen::Doing { leaf, .. }) =
                screen.chose(&shell(false), &Bench::default(), index)
            else {
                panic!("{group:?} item {index} does not open a command");
            };
            assert!(
                leaf.command.starts_with("sloop "),
                "{:?} names {:?}, which is not a command",
                leaf.title,
                leaf.command
            );
            assert!(!leaf.blurb.is_empty(), "{:?} says nothing", leaf.title);
        }
    }
}

/// **The one flow this task owns.** Picking `Init` types a directory, makes the registry,
/// and leaves something to print once the terminal has been handed back.
#[test]
fn starting_a_project_creates_the_registry_and_keeps_the_summary() {
    let here = std::env::temp_dir().join(format!("sloop-ui-{}", std::process::id()));
    let global = here.join("global");
    std::fs::create_dir_all(&global).expect("a temporary directory");

    let mut shell = Shell {
        global,
        cwd: here.clone(),
        working_in: "the global store".to_owned(),
        found_by: "nothing named a project".to_owned(),
        holds: 0,
        fresh: true,
    };

    let mut scripted = Scripted::doing(vec![
        Does::Pick(0),
        Does::Type("."),
        Does::Leave,
        Does::Leave,
    ]);
    let (_, kept) = walk(
        &mut shell,
        &mut scripted,
        &mut Bench::default(),
        &mut Curtain::default(),
    )
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

    let _ = std::fs::remove_dir_all(&here);
}

/// The directory box comes pre-filled with where sloop was run, and coming back to it
/// finds what was typed still typed.
#[test]
fn the_directory_box_keeps_what_was_typed() {
    let here = std::env::temp_dir().join(format!("sloop-ui-typed-{}", std::process::id()));
    let global = here.join("global");
    std::fs::create_dir_all(&global).expect("a temporary directory");

    let mut shell = Shell {
        global,
        cwd: here.clone(),
        working_in: "the global store".to_owned(),
        found_by: "nothing named a project".to_owned(),
        holds: 0,
        fresh: true,
    };

    let mut scripted = Scripted::doing(vec![
        Does::Pick(0),
        // Somewhere that is not there, so the screen says so and stays put.
        Does::Type("nowhere-at-all"),
        Does::Back,
        Does::Pick(0),
        Does::Leave,
    ]);
    walk(
        &mut shell,
        &mut scripted,
        &mut Bench::default(),
        &mut Curtain::default(),
    )
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

    let _ = std::fs::remove_dir_all(&here);
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
    walk(
        &mut shell,
        &mut scripted,
        &mut Bench::default(),
        &mut Curtain::default(),
    )
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
    let doors = match home.face(&bench) {
        super::Face::Menu(menu) => menu.items.len(),
        super::Face::Ask(_) => unreachable!("the home screen is a menu"),
    };

    for door in 0..doors {
        match home.chose(&shell(false), &bench, door) {
            super::Flow::To(Screen::Group { group, .. }) => {
                let inside = Screen::Group { group, cursor: 0 };
                let leaves = match inside.face(&bench) {
                    super::Face::Menu(menu) => menu.items.len(),
                    super::Face::Ask(_) => 0,
                };
                for leaf in 0..leaves {
                    if let super::Flow::To(Screen::Doing { leaf: what, .. }) =
                        inside.chose(&shell(false), &bench, leaf)
                    {
                        found.push((door, leaf, what.job));
                    }
                }
            }
            super::Flow::To(Screen::Doing { leaf: what, .. }) => {
                found.push((door, usize::MAX, what.job));
            }
            _ => {}
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
    assert_eq!(leaves.len(), 18, "the tree lost a command: {leaves:?}");

    for (door, leaf, job) in leaves {
        let mut script = down_to(door, leaf);
        // Enter through every question, and once more on the screen that runs it. Exactly
        // that many, so a job that runs twice is a failure rather than a longer script.
        script.extend(std::iter::repeat_n(
            Does::Enter,
            questions(job, &Bench::default()) + 1,
        ));

        let mut bench = Bench::default();
        let mut curtain = Curtain::default();
        run_it(script, &mut shell(false), &mut bench, &mut curtain);

        assert_eq!(
            bench.ran.iter().map(|(job, _)| *job).collect::<Vec<_>>(),
            vec![job],
            "{job:?} was not run once from the menu"
        );
        assert_eq!(
            curtain.out, 1,
            "{job:?} ran without the terminal handed back"
        );
        assert_eq!(curtain.back, 1, "{job:?} never took the screen again");
        assert_eq!(curtain.paused, 1, "{job:?} painted over its own output");
    }
}

/// **Nothing runs off the back of the last answer.** There is always one more screen, with
/// one item on it, that says what is about to happen — a flow that fires the moment its
/// last question is answered cannot be read back before it happens.
#[test]
fn a_flow_always_shows_what_it_is_about_to_do() {
    let mut bench = Bench::default();
    let mut curtain = Curtain::default();

    // Databases, then Rename one: two questions, and the third screen runs it.
    let (seen, _) = run_it(
        vec![Does::Pick(0), Does::Pick(5), Does::Enter, Does::Enter],
        &mut shell(false),
        &mut bench,
        &mut curtain,
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
    let mut curtain = Curtain::default();

    // Databases → Rename one → the second database → a new name → run it.
    run_it(
        vec![
            Does::Pick(0),
            Does::Pick(5),
            Does::Pick(1),
            Does::Type("orders_old"),
            Does::Enter,
        ],
        &mut shell(false),
        &mut bench,
        &mut curtain,
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
    let mut curtain = Curtain::default();

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
        ],
        &mut shell(false),
        &mut bench,
        &mut curtain,
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
    let mut curtain = Curtain::default();

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
    script.extend(std::iter::repeat_n(Does::Enter, 10));
    run_it(script, &mut shell(false), &mut bench, &mut curtain);

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
    let mut curtain = Curtain::default();

    let (seen, exit) = run_it(
        vec![
            Does::Pick(0),
            Does::Pick(5),
            Does::Enter,
            Does::Type("orders_old"),
            Does::Enter,
            Does::Quit,
        ],
        &mut shell(false),
        &mut bench,
        &mut curtain,
    );

    assert_eq!(exit, Exit::Usage, "the menu swallowed the command's code");
    assert_eq!(bench.ran.len(), 1);
    assert_eq!(
        seen.listed.last().map(Vec::as_slice),
        Some(["Rename it".to_owned(), BACK.to_owned()].as_slice()),
        "a failure threw the flow away: {:?}",
        seen.listed
    );
}

/// A job that works leaves the flow behind, so nothing is one keystroke away from being
/// run a second time.
#[test]
fn a_job_that_works_goes_back_to_the_group_it_came_from() {
    let mut bench = Bench::default();
    let mut curtain = Curtain::default();

    let (seen, exit) = run_it(
        vec![Does::Pick(0), Does::Pick(2), Does::Enter, Does::Quit],
        &mut shell(false),
        &mut bench,
        &mut curtain,
    );

    assert_eq!(exit, Exit::Success);
    assert_eq!(bench.ran.len(), 1, "{:?}", bench.ran);
    assert_eq!(
        seen.seen.last().map(String::as_str),
        Some("Databases"),
        "a finished job did not come back to its group: {:?}",
        seen.seen
    );
}

/// The breadcrumb names every step that reached the screen, so somebody four questions
/// deep can see where they are without leaving.
#[test]
fn a_flow_screen_names_every_step_that_reached_it() {
    let mut bench = Bench::default();
    let mut curtain = Curtain::default();

    let (seen, _) = run_it(
        vec![Does::Pick(0), Does::Pick(5), Does::Quit],
        &mut shell(false),
        &mut bench,
        &mut curtain,
    );

    assert_eq!(
        seen.seen.last().map(String::as_str),
        Some("Databases › Rename one"),
        "{:?}",
        seen.seen
    );
}
