//! The shell, driven without a terminal.
//!
//! **Every one of these walks the real loop.** The only thing replaced is the person: a
//! [`Scripted`] answers each prompt from a list written down in advance and records what it
//! was shown, so `← Back` returning from every screen in the tree is an assertion rather
//! than something somebody remembered to try by hand once.

use std::path::PathBuf;

use super::ask::{Answer, Asking};
use super::paint::Header;
use super::screen::{Ask, Group, Item, Kept, Screen, Shell};
use super::{Alternate, BACK, QUIT, at_a_terminal, walk};
use crate::failure::Outcome;

/// One thing a scripted person does.
#[derive(Debug, Clone)]
enum Does {
    /// Pick the item at this index.
    Pick(usize),
    /// Pick the last item, whatever it is — the way out.
    Leave,
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
            Does::Leave => Answer::Given(last),
            Does::Back => Answer::Back,
            Does::Quit | Does::Type(_) => Answer::Quit,
        })
    }

    fn text(&mut self, ask: &Ask) -> Outcome<Answer<String>> {
        self.boxes.push(ask.initial.clone());

        Ok(match self.next() {
            Does::Type(text) => Answer::Given(text.to_owned()),
            Does::Back => Answer::Back,
            _ => Answer::Quit,
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
    let mut shell = shell(false);
    let mut scripted = Scripted::doing(script);
    walk(&mut shell, &mut scripted).expect("the menu should not fail on a scripted session");
    scripted
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
    walk(&mut shell, &mut scripted).expect("a first run should draw");

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
    let doors = match home.face() {
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
    match home.chose(&shell(false), index) {
        super::Flow::To(Screen::Group { group, .. }) => {
            match (Screen::Group { group, cursor: 0 }).face() {
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
        let count = match screen.face() {
            super::Face::Menu(menu) => menu.items.len(),
            super::Face::Ask(_) => 0,
        };

        for index in 0..count {
            let super::Flow::To(Screen::Soon { leaf, .. }) = screen.chose(&shell(false), index)
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
    let kept = walk(&mut shell, &mut scripted).expect("init should run from the menu");

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
    walk(&mut shell, &mut scripted).expect("a bad directory is not a failed session");

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
    walk(&mut shell, &mut scripted).expect("a bad directory is not a failed session");

    let screen = Screen::NewProject {
        at: "nowhere-at-all".to_owned(),
        trouble: Some("nowhere-at-all is not a directory that exists".to_owned()),
    };
    let header = screen.header(&shell);
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
