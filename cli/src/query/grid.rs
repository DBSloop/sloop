//! The result grid — the one screen in this program `ratatui` draws.
//!
//! **`CLAUDE.md` allows `ratatui` "only when panes or in-place progress arrive", and this is
//! that condition arriving.** A scrollable, column-aligned table over a paged result set is
//! not an `inquire` prompt: it has a viewport that moves in two directions over data that is
//! wider and longer than the terminal, and a footer that changes as it moves. Every other
//! screen in sloop stays `inquire`, and nothing here is reused by one.
//!
//! **Its own alternate screen, entered and left by this module.** The menu's screen has
//! already been handed back by the time a command runs — see `ui`'s header — so the grid
//! takes the terminal for itself and gives it back the same way, with the guard running on a
//! panic as well as on the way out. Whatever is worth keeping is printed afterwards.
//!
//! **A page is fetched, never sliced.** The statement carries its own `LIMIT`, so moving to
//! the next page is another round trip for two hundred rows rather than a window over a
//! million already in memory — which is the whole of `R19a`'s *"a million-row table pages
//! without the terminal stalling"*.

use std::io::Write as _;

use ratatui::Terminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::{execute, terminal};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell as Box_, Paragraph, Row as Line_, Table};

use crate::engine::Rows;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::style::{Hue, Ink};

/// The terminal the grid draws on: standard error, so a `--sql` run being piped somewhere
/// is not competing with the screen for standard output.
type Screen = Terminal<ratatui::backend::CrosstermBackend<std::io::Stderr>>;

/// The widest a column is allowed to get before it is cut.
///
/// A `text` column holding an essay would otherwise push every column after it off the
/// screen, and the column that matters is rarely the long one.
const WIDEST: u16 = 40;

/// What the grid is showing, and where it is looking.
struct View {
    /// The page on screen.
    rows: Rows,
    /// Which page that is, counting from zero.
    page: u64,
    /// Whether the server had more rows than this page holds.
    more: bool,
    /// The highlighted row within the page.
    at: usize,
    /// The first column drawn, for a result wider than the terminal.
    from: usize,
    /// Something that went wrong, shown in the footer until the next keystroke.
    trouble: Option<String>,
}

/// Show a result, and let somebody move around it.
///
/// `fetch` is handed a page number and returns that page, one row longer than [`super::PAGE`]
/// where there is a next one. It is called for every page, so a query nobody scrolls costs
/// one round trip.
///
/// **Returns the number of rows the reader actually saw**, which is what the line printed
/// after the screen is handed back reports. There is no total: counting a million rows to
/// print a number nobody asked for is the query this screen exists to avoid.
pub fn show(title: &str, first: Rows, mut fetch: impl FnMut(u64) -> Outcome<Rows>) -> Outcome<u64> {
    let mut view = View {
        more: first.rows.len() as u64 > super::PAGE,
        rows: trimmed(first),
        page: 0,
        at: 0,
        from: 0,
        trouble: None,
    };

    let mut screen = Held::take()?;
    let outcome = walk(title, &mut view, &mut fetch, &mut screen.terminal);
    screen.give_back();
    outcome
}

/// The key loop.
fn walk(
    title: &str,
    view: &mut View,
    fetch: &mut impl FnMut(u64) -> Outcome<Rows>,
    terminal: &mut Screen,
) -> Outcome<u64> {
    let mut seen = view.rows.rows.len() as u64;

    loop {
        terminal
            .draw(|frame| draw(frame, title, view))
            .map_err(|error| drawing(&error))?;

        let Some(key) = next_key()? else {
            continue;
        };
        if !apply(view, key, fetch, &mut seen) {
            break;
        }
    }

    Ok(seen)
}

/// What one keystroke does. `false` when it is time to leave.
///
/// **Split from the loop so the whole of the moving-around can be driven by a written-down
/// list of keys in a test**, with no terminal anywhere near it — which is the rule `ui::walk`
/// already follows for the menu. What is worth checking here is arithmetic: that the
/// highlight cannot run off either end, that a page turn asks for the right page, and that
/// a page that will not load leaves the one on the screen alone.
fn apply(
    view: &mut View,
    key: KeyEvent,
    fetch: &mut impl FnMut(u64) -> Outcome<Rows>,
    seen: &mut u64,
) -> bool {
    view.trouble = None;
    let width = view.rows.columns.len();

    match (key.code, key.modifiers) {
        // Three ways out, because somebody who opened this by accident should not have to
        // guess which one this program chose.
        (KeyCode::Char('q') | KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            return false;
        }

        (KeyCode::Down | KeyCode::Char('j'), _) => {
            view.at = (view.at + 1).min(view.rows.rows.len().saturating_sub(1));
        }
        (KeyCode::Up | KeyCode::Char('k'), _) => view.at = view.at.saturating_sub(1),
        (KeyCode::Home, _) => view.at = 0,
        (KeyCode::End, _) => view.at = view.rows.rows.len().saturating_sub(1),

        (KeyCode::Right | KeyCode::Char('l'), _) => {
            view.from = (view.from + 1).min(width.saturating_sub(1));
        }
        (KeyCode::Left | KeyCode::Char('h'), _) => view.from = view.from.saturating_sub(1),

        (KeyCode::PageDown | KeyCode::Char('n'), _) if view.more => {
            if turn(view, fetch, view.page + 1) {
                *seen += view.rows.rows.len() as u64;
            }
        }
        (KeyCode::PageUp | KeyCode::Char('p'), _) if view.page > 0 => {
            turn(view, fetch, view.page - 1);
        }
        _ => {}
    }

    true
}

/// Fetch a page and move to it, or say why not and stay where it was.
///
/// **A page that will not load is not the end of the session.** The connection may have gone
/// while somebody was reading; saying so in the footer and leaving the page they can still
/// see on the screen is better than tearing the screen down under them.
fn turn(view: &mut View, fetch: &mut impl FnMut(u64) -> Outcome<Rows>, to: u64) -> bool {
    match fetch(to) {
        Ok(page) => {
            view.more = page.rows.len() as u64 > super::PAGE;
            view.rows = trimmed(page);
            view.page = to;
            view.at = 0;
            true
        }
        Err(failure) => {
            view.trouble = Some(failure.message().to_owned());
            false
        }
    }
}

/// Drop the extra row the statement asked for so it is never drawn.
///
/// One row past the page is how *"is there a next page"* is answered without counting the
/// whole result — see [`super::Built::sql`]. It is an answer, not a row somebody asked for.
fn trimmed(mut rows: Rows) -> Rows {
    rows.rows
        .truncate(usize::try_from(super::PAGE).unwrap_or(usize::MAX));
    rows
}

/// One frame.
fn draw(frame: &mut ratatui::Frame<'_>, title: &str, view: &View) {
    let [top, middle, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" sloop ", accent().add_modifier(Modifier::BOLD)),
            Span::styled(title.to_owned(), plain()),
        ])),
        top,
    );

    frame.render_widget(grid(view, middle), middle);
    frame.render_widget(Paragraph::new(footer(view)), bottom);
}

/// The table itself, from the first drawn column onwards.
fn grid(view: &View, area: Rect) -> Table<'_> {
    let columns: Vec<&String> = view.rows.columns.iter().skip(view.from).collect();
    if columns.is_empty() {
        return Table::default().block(Block::default());
    }

    let widths: Vec<u16> = columns
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let widest = view
                .rows
                .rows
                .iter()
                .filter_map(|row| row.get(view.from + index))
                .map(|cell| flattened(cell.shown()).chars().count())
                .max()
                .unwrap_or(0);
            u16::try_from(widest.max(name.chars().count()))
                .unwrap_or(WIDEST)
                .clamp(1, WIDEST)
        })
        .collect();

    // Everything below the header and above the footer, minus the header row itself.
    let showing = usize::from(area.height).saturating_sub(1);
    let first = view.at.saturating_sub(showing.saturating_sub(1));

    let body = view
        .rows
        .rows
        .iter()
        .enumerate()
        .skip(first)
        .map(|(at, row)| {
            let cells = row.iter().skip(view.from).map(|cell| {
                Box_::from(flattened(cell.shown())).style(if cell.is_null() {
                    dim().add_modifier(Modifier::ITALIC)
                } else {
                    plain()
                })
            });
            Line_::new(cells).style(if at == view.at {
                plain().add_modifier(Modifier::REVERSED)
            } else {
                plain()
            })
        });

    Table::new(body, widths.into_iter().map(Constraint::Length)).header(
        Line_::new(
            columns
                .into_iter()
                .map(|name| Box_::from(name.as_str()).style(accent().add_modifier(Modifier::BOLD))),
        )
        .bottom_margin(0),
    )
}

/// The line along the bottom: where you are, and what the keys do.
fn footer(view: &View) -> Line<'static> {
    if let Some(trouble) = &view.trouble {
        return Line::from(vec![
            Span::styled(" ! ", hue(Hue::Bad).add_modifier(Modifier::BOLD)),
            Span::styled(trouble.clone(), hue(Hue::Bad)),
        ]);
    }

    let rows = view.rows.rows.len();
    let first = view.page * super::PAGE;
    let here = if rows == 0 {
        " no rows".to_owned()
    } else {
        format!(
            " rows {}–{}{}",
            first + 1,
            first + rows as u64,
            if view.more { " of more" } else { "" }
        )
    };

    let mut keys = vec!["↑↓ row", "←→ column"];
    if view.more {
        keys.push("PgDn next");
    }
    if view.page > 0 {
        keys.push("PgUp back");
    }
    keys.push("q done");

    Line::from(vec![
        Span::styled(here, accent()),
        Span::styled(format!("   {}", keys.join("   ")), dim()),
    ])
}

/// One line of a cell, whatever the value did.
///
/// **A newline inside a value is shown, not obeyed.** A row is one row; a `text` column
/// holding an address would otherwise redraw the grid three rows tall and put every column
/// after it out of line.
fn flattened(value: &str) -> String {
    value
        .chars()
        .map(|letter| if letter.is_control() { '·' } else { letter })
        .collect()
}

/// sloop's palette, in `ratatui`'s terms.
///
/// The `COLORTERM` question is answered once, in `style`, and every renderer asks that one
/// answer — see [`crate::style::ink`]. A second check here would be a second answer.
fn hue(which: Hue) -> Style {
    Style::default().fg(match crate::style::ink(which) {
        Ink::True(r, g, b) => Color::Rgb(r, g, b),
        Ink::Cube(index) => Color::Indexed(index),
    })
}

fn accent() -> Style {
    hue(Hue::Brand)
}

fn plain() -> Style {
    hue(Hue::Text)
}

fn dim() -> Style {
    hue(Hue::Dim)
}

/// One keystroke, ignoring everything that is not one.
///
/// A resize or a key being *released* both arrive as events and neither is an answer;
/// returning `None` redraws, which is exactly what a resize wants anyway.
fn next_key() -> Outcome<Option<KeyEvent>> {
    match event::read().map_err(|error| drawing(&error))? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(Some(key)),
        _ => Ok(None),
    }
}

/// The terminal, taken for the grid and given back whatever happens.
///
/// **Given back on drop as well as explicitly**, which is the same rule `ui` follows and for
/// the same reason: a panic inside the loop must not leave somebody with a terminal in raw
/// mode and no echo. `panic = "abort"` is deliberately not set in `Cargo.toml` so that the
/// unwind runs this.
struct Held {
    terminal: Screen,
    handed_back: bool,
    /// Whether this grid is the one that took the alternate buffer.
    ///
    /// **False under the menu, and that is the whole of it.** The shell is already in the
    /// alternate screen and hands it back itself when the session ends; a grid that entered
    /// it again and then left would put the *menu* back on the terminal underneath, which is
    /// the one thing the owner's first rule forbids.
    took_the_screen: bool,
}

impl Held {
    fn take() -> Outcome<Self> {
        terminal::enable_raw_mode().map_err(|error| drawing(&error))?;
        let took_the_screen = !crate::console::shell_is_driving();
        let mut out = std::io::stderr();
        if took_the_screen {
            execute!(out, terminal::EnterAlternateScreen).map_err(|error| drawing(&error))?;
        }
        let terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(out))
            .map_err(|error| drawing(&error))?;
        Ok(Self {
            terminal,
            handed_back: false,
            took_the_screen,
        })
    }

    fn give_back(&mut self) {
        if self.handed_back {
            return;
        }
        self.handed_back = true;
        let _ = terminal::disable_raw_mode();
        if self.took_the_screen {
            let _ = execute!(std::io::stderr(), terminal::LeaveAlternateScreen);
        }
        let _ = std::io::stderr().flush();
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.give_back();
    }
}

/// The terminal would not do as it was told, which is not a database problem.
fn drawing(error: &std::io::Error) -> Failure {
    Failure::new(
        Exit::Usage,
        format!("the terminal would not answer: {error}"),
    )
}

#[cfg(test)]
#[path = "grid_tests.rs"]
mod tests;
