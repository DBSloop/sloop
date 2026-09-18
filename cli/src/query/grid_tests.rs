//! Moving around a result, with no terminal anywhere near it.
//!
//! **What is worth checking here is arithmetic.** Drawing is `ratatui`'s job and a test that
//! asserted on a buffer of glyphs would be a test that breaks when a border changes. What
//! breaks a grid is a highlight that runs off the end of a short page, a page turn that asks
//! for the wrong page, or a page that fails to load and takes the one on the screen with it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{View, apply, trimmed};
use crate::engine::{Cell, Rows};
use crate::exit::Exit;
use crate::failure::Failure;

/// A page of `how_many` rows, numbered from `first`.
fn page(first: u64, how_many: u64) -> Rows {
    Rows {
        columns: vec!["id".to_owned(), "label".to_owned(), "bucket".to_owned()],
        rows: (first..first + how_many)
            .map(|at| {
                vec![
                    Cell::Text(at.to_string()),
                    Cell::Text(format!("row {at}")),
                    Cell::Null,
                ]
            })
            .collect(),
    }
}

/// A view of a table with more rows than one page holds.
fn opened(how_many: u64) -> View {
    let first = page(1, how_many);
    View {
        more: first.rows.len() as u64 > crate::query::PAGE,
        rows: trimmed(first),
        page: 0,
        at: 0,
        from: 0,
        trouble: None,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A server with a million rows in it, answering one page at a time.
fn a_million() -> impl FnMut(u64) -> crate::failure::Outcome<Rows> {
    move |at: u64| Ok(page(at * crate::query::PAGE + 1, crate::query::PAGE + 1))
}

/// The extra row the statement asks for is an answer, not a row anybody sees.
#[test]
fn the_page_shows_two_hundred_and_knows_there_is_another() {
    let view = opened(crate::query::PAGE + 1);
    assert_eq!(view.rows.rows.len() as u64, crate::query::PAGE);
    assert!(view.more);

    let short = opened(12);
    assert_eq!(short.rows.rows.len(), 12);
    assert!(!short.more, "a short page is the last page");
}

/// The highlight cannot run off either end, however hard anybody leans on a key.
#[test]
fn the_highlight_stays_on_the_page() {
    let mut view = opened(3);
    let mut seen = 3;
    let mut fetch = a_million();

    for _ in 0..20 {
        assert!(apply(&mut view, key(KeyCode::Down), &mut fetch, &mut seen));
    }
    assert_eq!(view.at, 2, "three rows, so the last one is index 2");

    for _ in 0..20 {
        assert!(apply(&mut view, key(KeyCode::Up), &mut fetch, &mut seen));
    }
    assert_eq!(view.at, 0);
}

/// Sideways is the same: a result three columns wide never scrolls past the third.
#[test]
fn the_column_window_stays_on_the_result() {
    let mut view = opened(3);
    let mut seen = 3;
    let mut fetch = a_million();

    for _ in 0..20 {
        apply(&mut view, key(KeyCode::Right), &mut fetch, &mut seen);
    }
    assert_eq!(view.from, 2);

    for _ in 0..20 {
        apply(&mut view, key(KeyCode::Left), &mut fetch, &mut seen);
    }
    assert_eq!(view.from, 0);
}

/// **A million-row table is read a page at a time and counted as it goes.**
#[test]
fn paging_forward_asks_for_the_next_page_and_counts_it() {
    let mut view = opened(crate::query::PAGE + 1);
    let mut seen = crate::query::PAGE;
    let mut fetch = a_million();

    apply(&mut view, key(KeyCode::PageDown), &mut fetch, &mut seen);
    assert_eq!(view.page, 1);
    assert_eq!(view.at, 0, "a new page starts at the top");
    assert_eq!(seen, crate::query::PAGE * 2);
    assert_eq!(view.rows.rows[0][0], Cell::Text("201".to_owned()));

    apply(&mut view, key(KeyCode::PageUp), &mut fetch, &mut seen);
    assert_eq!(view.page, 0);
    assert_eq!(view.rows.rows[0][0], Cell::Text("1".to_owned()));
}

/// There is no page before the first, and no page after the last.
#[test]
fn there_is_nothing_before_the_first_page_or_after_the_last() {
    let mut view = opened(12);
    let mut seen = 12;
    let mut asked = 0;
    let mut fetch = |at: u64| {
        asked += 1;
        Ok(page(at * crate::query::PAGE + 1, 1))
    };

    apply(&mut view, key(KeyCode::PageUp), &mut fetch, &mut seen);
    apply(&mut view, key(KeyCode::PageDown), &mut fetch, &mut seen);
    assert_eq!(view.page, 0);
    assert_eq!(
        asked, 0,
        "neither key should have asked the server anything"
    );
}

/// **A page that will not load is not the end of the session.** The connection may have gone
/// while somebody was reading; the page they can still see stays on the screen.
#[test]
fn a_page_that_will_not_load_leaves_the_one_on_the_screen_alone() {
    let mut view = opened(crate::query::PAGE + 1);
    let mut seen = crate::query::PAGE;
    let mut fetch = |_: u64| Err(Failure::new(Exit::Connect, "the server went away"));

    assert!(
        apply(&mut view, key(KeyCode::PageDown), &mut fetch, &mut seen),
        "a failed page turn does not close the grid"
    );
    assert_eq!(view.page, 0, "still on the page that loaded");
    assert_eq!(view.rows.rows.len() as u64, crate::query::PAGE);
    assert_eq!(seen, crate::query::PAGE, "rows nobody saw are not counted");
    assert_eq!(view.trouble.as_deref(), Some("the server went away"));

    // And the next keystroke clears it, so a stale complaint never sits under a good page.
    apply(&mut view, key(KeyCode::Down), &mut fetch, &mut seen);
    assert!(view.trouble.is_none());
}

/// Three ways out, and every one of them leaves.
#[test]
fn every_way_out_is_a_way_out() {
    let mut seen = 3;
    let mut fetch = a_million();

    for code in [KeyCode::Char('q'), KeyCode::Esc] {
        let mut view = opened(3);
        assert!(!apply(&mut view, key(code), &mut fetch, &mut seen));
    }

    let mut view = opened(3);
    assert!(!apply(
        &mut view,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &mut fetch,
        &mut seen
    ));
}

/// A key nothing is bound to changes nothing and does not leave.
#[test]
fn a_key_nobody_bound_does_nothing() {
    let mut view = opened(3);
    let mut seen = 3;
    let mut fetch = a_million();

    assert!(apply(
        &mut view,
        key(KeyCode::Char('z')),
        &mut fetch,
        &mut seen
    ));
    assert_eq!((view.at, view.from, view.page), (0, 0, 0));
}

/// Vim's keys do what the arrows do, because somebody will try them.
#[test]
fn the_vim_keys_move_the_same_way() {
    let mut view = opened(3);
    let mut seen = 3;
    let mut fetch = a_million();

    apply(&mut view, key(KeyCode::Char('j')), &mut fetch, &mut seen);
    assert_eq!(view.at, 1);
    apply(&mut view, key(KeyCode::Char('k')), &mut fetch, &mut seen);
    assert_eq!(view.at, 0);
    apply(&mut view, key(KeyCode::Char('l')), &mut fetch, &mut seen);
    assert_eq!(view.from, 1);
    apply(&mut view, key(KeyCode::Char('h')), &mut fetch, &mut seen);
    assert_eq!(view.from, 0);
}

/// A control character in a value is shown rather than obeyed: a row is one row.
#[test]
fn a_newline_in_a_value_never_becomes_a_second_row() {
    assert_eq!(super::flattened("line\nbreak"), "line·break");
    assert_eq!(super::flattened("tab\there"), "tab·here");
    assert_eq!(super::flattened("ordinary"), "ordinary");
}

/// `NULL` is drawn as a word, so an empty string beside it is visibly a different thing.
#[test]
fn null_is_a_word_and_an_empty_string_is_not() {
    assert_eq!(Cell::Null.shown(), "NULL");
    assert_eq!(Cell::Text(String::new()).shown(), "");
    assert!(Cell::Null.is_null());
    assert!(!Cell::Text("NULL".to_owned()).is_null());
}

/// The kind of key event matters: a key being *released* is not a keystroke.
#[test]
fn a_key_being_let_go_is_not_a_keystroke() {
    let released = KeyEvent::new_with_kind(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    );
    assert_ne!(released.kind, KeyEventKind::Press);
}

/// **The grid draws what it says it draws**, checked against a buffer rather than a screen.
///
/// `TestBackend` is `ratatui`'s own off-screen terminal, so this is the real widget tree
/// laid out at a real size — the heading row, the values under it, and the footer that says
/// where you are. What it does not check is colour, which is `style`'s answer and is checked
/// where that answer is made.
#[test]
fn a_frame_holds_the_heading_the_rows_and_the_footer() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let view = super::View {
        more: true,
        rows: trimmed(page(1, crate::query::PAGE + 1)),
        page: 0,
        at: 0,
        from: 0,
        trouble: None,
    };

    let mut terminal = Terminal::new(TestBackend::new(60, 8)).expect("an off-screen terminal");
    terminal
        .draw(|frame| super::draw(frame, "public.big", &view))
        .expect("drawing one frame");

    let drawn: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();

    assert!(drawn.contains("sloop public.big"), "no title: {drawn}");
    assert!(drawn.contains("id"), "no heading: {drawn}");
    assert!(drawn.contains("row 1"), "no rows: {drawn}");
    assert!(
        drawn.contains("NULL"),
        "a NULL was not drawn as a word: {drawn}"
    );
    assert!(drawn.contains("rows 1"), "no footer: {drawn}");
    assert!(
        drawn.contains("PgDn next"),
        "no way forward offered: {drawn}"
    );
    assert!(
        !drawn.contains("PgUp back"),
        "offered a page before the first: {drawn}"
    );
}

/// A result with no rows is still a screen, and the footer says so rather than lying.
#[test]
fn a_result_with_no_rows_says_so() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let view = super::View {
        more: false,
        rows: Rows {
            columns: vec!["id".to_owned()],
            rows: Vec::new(),
        },
        page: 0,
        at: 0,
        from: 0,
        trouble: None,
    };

    let mut terminal = Terminal::new(TestBackend::new(40, 5)).expect("an off-screen terminal");
    terminal
        .draw(|frame| super::draw(frame, "public.empty", &view))
        .expect("drawing one frame");

    let drawn: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();

    assert!(drawn.contains("no rows"), "{drawn}");
    assert!(
        drawn.contains("q done"),
        "the way out is always offered: {drawn}"
    );
}
