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
use super::screen::{Ask, Kept, Row, Screen, Shell};
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
    /// Choose the heading on this row, which is a thing the cursor can land on and Enter
    /// can be pressed over, however little it should do.
    Heading(usize),
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
    /// The headings each list was drawn under.
    headed: Vec<Vec<String>>,
    /// The row each choice actually landed on. A menu with headings in it draws more rows
    /// than it has items, so "the third item" and "row three" are different numbers, and
    /// the highlight is kept by row.
    answered_at: Vec<usize>,
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
            headed: Vec::new(),
            answered_at: Vec::new(),
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
        rows: &[Row],
        way_out: &str,
        at: usize,
    ) -> Outcome<Answer<usize>> {
        // What can be chosen, and where each one sits among the drawn rows. A test says
        // "the second item" and means the second item, not the fourth row.
        let mut titles = Vec::new();
        let mut at_row = Vec::new();
        for (row, drawn) in rows.iter().enumerate() {
            if let Row::Item(item, _) = drawn {
                titles.push(item.title.clone());
                at_row.push(row);
            }
        }
        titles.push(way_out.to_owned());

        self.headed.push(
            rows.iter()
                .filter_map(|row| match row {
                    Row::Heading(heading) => Some(heading.clone()),
                    Row::Item(..) => None,
                })
                .collect(),
        );
        self.listed.push(titles);
        self.started_at.push(at);

        let way_out_row = rows.len();
        let answered = match self.next() {
            Does::Heading(row) => row,
            Does::Pick(index) => at_row.get(index).copied().unwrap_or(way_out_row),
            Does::Enter | Does::Fill => at_row.first().copied().unwrap_or(way_out_row),
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
            Does::Pick(_) | Does::Heading(_) | Does::Quit => Answer::Quit,
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
        holds: if fresh { 0 } else { 2 },
        fresh,
        // Every test in this file is about a machine that has been through Setup. The one
        // that is about a machine that has not says so by name.
        set_up: true,
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
/// Every command on the home screen, entered and backed out of, with the highlight checked
/// on the way home each time. There is one level to come back from since the doors went: a
/// command is on the front page, and choosing it opens its flow.
#[test]
fn back_returns_from_every_screen_in_the_tree() {
    for command in 0..commands_on_the_home_screen() {
        let seen = session(vec![Does::Pick(command), Does::Leave, Does::Leave]).seen;

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

/// How many commands the home screen offers, headings not counted.
fn commands_on_the_home_screen() -> usize {
    match (Screen::Home { cursor: 0 }).face(&Bench::default()) {
        super::Face::Menu(menu) => menu.choices(),
        super::Face::Ask(_) => unreachable!("the home screen is a menu"),
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
        Some(&1),
        "the menu did not open on the first thing under its first heading"
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
    let seen = session(vec![Does::Pick(8), Does::Leave, Does::Leave]);

    // Three screens: the menu, the command, and the menu again on the way back up.
    let at = seen.started_at;
    let chose = seen.answered_at;
    assert_eq!(at.len(), 3, "{at:?}");
    // Row 1, not row 0: every page opens on the first thing under its first heading.
    assert_eq!(at[..2], [1, 1], "a screen opened somewhere unexpected");
    assert_ne!(
        chose[0], 1,
        "the test picked the row it would have opened on"
    );
    assert_eq!(at[2], chose[0], "the menu lost its highlight: {at:?}");
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
    let screen = Screen::Home { cursor: 0 };

    for index in 0..commands_on_the_home_screen() {
        let super::Flow::To(Screen::Doing { leaf, .. }) =
            screen.chose(&shell(false), &Bench::default(), index)
        else {
            panic!("home item {index} does not open a command");
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
        holds: 0,
        fresh: true,
        set_up: true,
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
        holds: 0,
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
        super::Face::Menu(menu) => menu.choices(),
        super::Face::Ask(_) => unreachable!("the home screen is a menu"),
    };

    for door in 0..doors {
        if let super::Flow::To(Screen::Doing { leaf: what, .. }) =
            home.chose(&shell(false), &bench, door)
        {
            found.push((door, usize::MAX, what.job));
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
    assert_eq!(leaves.len(), 21, "the tree lost a command: {leaves:?}");

    for (door, leaf, job) in leaves {
        let mut script = down_to(door, leaf);
        // Enter through every question, and once more on the screen that runs it. Exactly
        // that many, so a job that runs twice is a failure rather than a longer script.
        script.extend(std::iter::repeat_n(
            Does::Fill,
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
        vec![Does::Pick(5), Does::Fill, Does::Fill],
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
            Does::Pick(5),
            Does::Pick(1),
            Does::Type("orders_old"),
            Does::Fill,
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
        Does::Type("orders"),
        Does::Pick(0),
        Does::Leave,
        Does::Pick(1),
    ];
    script.extend(std::iter::repeat_n(Does::Fill, 10));
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
            Does::Pick(5),
            Does::Fill,
            Does::Type("orders_old"),
            Does::Fill,
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
fn a_job_that_works_comes_back_to_the_menu() {
    let mut bench = Bench::default();
    let mut curtain = Curtain::default();

    let (seen, exit) = run_it(
        vec![Does::Pick(2), Does::Fill, Does::Quit],
        &mut shell(false),
        &mut bench,
        &mut curtain,
    );

    assert_eq!(exit, Exit::Success);
    assert_eq!(bench.ran.len(), 1, "{:?}", bench.ran);
    assert_eq!(
        seen.seen.last().map(String::as_str),
        Some("·"),
        "a finished job did not come back to the menu: {:?}",
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
        vec![Does::Pick(5), Does::Quit],
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

// ---------------------------------------------------------------------------------------
// The headings
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
        if let super::Face::Menu(menu) = page.face(&bench) {
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
        match screen.face(bench) {
            super::Face::Menu(menu) if menu.choices() == 0 => return screen,
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

/// **The `Done when`, first part: every page in the tree draws under headings.**
#[test]
fn every_page_with_something_on_it_draws_under_a_heading() {
    for (what, menu) in every_menu() {
        if menu.choices() == 0 {
            // A page with nothing to choose has nothing to head. The only row is the way
            // out the loop adds.
            assert!(menu.rows().is_empty(), "{what}: {:?}", menu.rows());
            continue;
        }

        let rows = menu.rows();
        let headings: Vec<&String> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Heading(heading) => Some(heading),
                Row::Item(..) => None,
            })
            .collect();

        assert!(
            !headings.is_empty(),
            "{what} ({:?}) is a flat list",
            menu.question
        );
        for heading in headings {
            assert_eq!(
                heading.as_str(),
                heading.to_uppercase(),
                "{what}: {heading:?} is not in caps"
            );
            assert!(!heading.trim().is_empty(), "{what}: an empty heading");
        }
    }
}

/// And the first row of every page is a heading, so nothing floats above the structure.
#[test]
fn nothing_sits_above_the_first_heading() {
    for (what, menu) in every_menu() {
        if let Some(Row::Item(item, _)) = menu.rows().first() {
            panic!("{what}: {:?} sits above every heading", item.title);
        }
    }
}

/// **The second part: no heading can be chosen as though it were an item.**
///
/// `inquire` owns the key loop and has no notion of a row the cursor skips, so the proof is
/// what happens when one is chosen: the highlight moves to the first thing under it and the
/// screen draws again. Nothing runs, and nothing opens.
#[test]
fn choosing_a_heading_moves_to_what_is_under_it_and_does_nothing_else() {
    let mut bench = Bench::default();
    let mut curtain = Curtain::default();

    // Row 0 of the menu is a heading. `Does::Heading(0)` chooses it rather than an item.
    let (seen, _) = run_it(
        vec![Does::Heading(0), Does::Quit],
        &mut shell(false),
        &mut bench,
        &mut curtain,
    );

    assert!(bench.ran.is_empty(), "a heading ran something");
    assert_eq!(
        seen.seen,
        ["·", "·"],
        "a heading opened a screen: {:?}",
        seen.seen
    );
    assert_eq!(
        seen.started_at.last(),
        Some(&1),
        "the highlight did not move under the heading: {:?}",
        seen.started_at
    );
}

/// **The third part: every item names the command that does the same thing.**
///
/// Every command in the tree, and the flag form beside it. The things that are *not*
/// commands — a group door, an engine, a yes — carry their phrase instead, because there
/// is no command that means "MySQL".
#[test]
fn every_command_in_the_tree_names_its_flag_form() {
    let bench = Bench::default();

    let page = Screen::Home { cursor: 0 };
    let super::Face::Menu(menu) = page.face(&bench) else {
        panic!("the home screen is not a menu");
    };

    for row in menu.rows() {
        let Row::Item(item, _) = row else { continue };
        assert!(
            item.command.starts_with("sloop "),
            "{:?} names no command",
            item.title
        );
        assert_eq!(
            item.beside(),
            item.command,
            "{:?} shows something other than its command",
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
    assert_eq!(step.heading(), "ENGINE");
    let super::flow::How::Pick { items, .. } = step.how else {
        panic!("an engine is picked, not typed");
    };

    for item in items {
        assert!(
            item.command.is_empty(),
            "{:?} claimed a command",
            item.title
        );
        assert_eq!(
            item.beside(),
            item.blurb,
            "{:?} showed something other than its phrase",
            item.title
        );
        assert!(!item.beside().is_empty(), "{:?} says nothing", item.title);
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

/// The highlight never opens on a heading, and never lands on one when it is restored from
/// a screen whose sections have since changed shape.
#[test]
fn the_highlight_starts_on_something_that_can_be_chosen() {
    let rows = vec![
        Row::Heading("EVERY DAY".to_owned()),
        Row::Item(super::screen::Item::new("Databases", ""), 0),
        Row::Item(super::screen::Item::new("Backups", ""), 1),
        Row::Heading("WHEN YOU NEED IT".to_owned()),
        Row::Item(super::screen::Item::new("Backup key", ""), 2),
    ];

    assert_eq!(super::settled(&rows, 0), 1, "it opened on a heading");
    assert_eq!(super::settled(&rows, 1), 1, "it moved off an item");
    assert_eq!(super::settled(&rows, 3), 4, "it stayed on a heading");
    assert_eq!(
        super::settled(&rows, 9),
        1,
        "a cursor past the end went nowhere"
    );
    assert_eq!(super::settled(&[], 0), 0, "an empty list has nowhere to be");
}

/// **The defect this exists for.** Pressing Enter on an empty box that needs an answer
/// used to file the blank and carry it five screens to the command, which came back with a
/// usage error about a flag nobody passed. It now says so on the box.
#[test]
fn a_required_box_refuses_a_blank_and_says_so_on_the_spot() {
    let mut bench = Bench::default();
    let mut curtain = Curtain::default();

    // Databases > Make a new database > Enter on the empty name.
    let (seen, _) = run_it(
        vec![Does::Pick(1), Does::Enter, Does::Quit],
        &mut shell(false),
        &mut bench,
        &mut curtain,
    );

    assert!(bench.ran.is_empty(), "a blank name reached the command");
    // Still on the same box: the question was asked again and nothing moved on.
    assert_eq!(seen.boxes.len(), 2, "{:?} / {:?}", seen.boxes, seen.seen);

    // And the same thing said directly: a blank into the box comes back as a complaint on
    // the screen it was typed into, not as an answer.
    let page = Screen::Home { cursor: 0 };
    let super::Flow::To(mut making) = page.chose(&shell(false), &bench, 1) else {
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
    walk(
        &mut shell(false),
        &mut scripted,
        &mut Bench::default(),
        &mut Curtain::default(),
    )
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
    walk(
        &mut unset_up(),
        &mut scripted,
        &mut Bench::default(),
        &mut Curtain::default(),
    )
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

/// **Re-runnable after a failure, which is the other half of the `Done when`.** A failed
/// Setup stays on the Setup screen with what went wrong, rather than dropping into a menu
/// that cannot work or out of the session altogether.
#[test]
fn a_failed_setup_stays_on_the_setup_screen_and_says_why() {
    let screen = unset_up().opening();
    let troubled = screen.troubled("the cluster would not start".to_owned());

    let Screen::Setup { trouble, .. } = &troubled else {
        panic!("a failed setup left the setup screen: {troubled:?}");
    };
    assert_eq!(trouble.as_deref(), Some("the cluster would not start"));

    // And it offers to carry on rather than to start again, because every step it takes
    // asks before it acts.
    let crate::ui::screen::Face::Menu(menu) = troubled.face(&Bench::default()) else {
        panic!("the setup screen should be a menu");
    };
    assert!(
        menu.sections[0].items[0].title.contains("Carry on"),
        "{}",
        menu.sections[0].items[0].title
    );
}

/// Setup is the root of the tree it opens: nothing is behind it, so there is no breadcrumb
/// and no `← Back` into a menu that would not work.
#[test]
fn setup_is_a_root_with_nothing_behind_it() {
    let screen = unset_up().opening();
    assert!(screen.crumbs().is_empty());
}
