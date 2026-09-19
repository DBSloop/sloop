//! `sloop query` — read a database without writing SQL.
//!
//! **Pick a table, tick the columns, choose the test. Nothing is typed that is not a value.**
//! That is `R19a`'s rule and the reason this screen is safe by construction rather than by
//! inspection: there is no `DROP` on any of these lists, so there is no `DROP` to catch.
//! What the builder produces is in [`crate::query`]; what is here is the asking.
//!
//! **The flag form is `--sql`, and it is the one place text arrives.** It exists so that a
//! session done by hand becomes a line somebody can schedule — `R20`'s printed equivalent —
//! and so that somebody who *does* know SQL is not made to click through a builder to run
//! one statement. It is not policed by a keyword list. Every statement either form produces
//! runs inside a transaction the engine has been told is read-only, which is what refuses a
//! write that a blacklist would have missed: see [`crate::engine::Adapter::read`].
//!
//! **Two shapes of output, decided by which form was used rather than by what is attached.**
//! The builder ends in the grid, because somebody who built a query interactively is reading
//! it. `--sql` prints, because a flag form that opened a full-screen grid could not be put
//! in a script. Neither one guesses.

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;

use std::io::IsTerminal as _;
use std::path::Path;
use std::time::Duration;

use inquire::{Confirm, MultiSelect, Select, Text};

use crate::engine::{Adapter, ForeignKey, Reading, Rows, Table, TableShape, Target};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::query::{Built, Column, Condition, Join, Joiner, Operator, Wanted, grid};
use crate::registry::file::Database;
use crate::registry::{Registries, Scope};
use crate::ssh::tunnel::Tunnels;
use crate::style;

/// How long the server may spend on one statement.
///
/// **Long enough for a real query on a real table, short enough that a mistake is a pause
/// rather than a session somebody has to kill from another terminal.** A query builder is
/// exactly where an accidental cross join gets written, and a cross join of two large tables
/// does not finish.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What this command needs from the run around it.
pub struct Context<'a> {
    /// Both registries, already open.
    pub registries: Registries,
    /// `--password-command`, which outranks whatever route a record names.
    pub password_command: Option<&'a str>,
    /// The global store, for finding the client tools this machine has.
    pub global: &'a Path,
    /// Every SSH forward this session holds, so a menu session authenticates once.
    pub tunnels: &'a Tunnels,
}

/// What the command line carried.
pub struct Asking<'a> {
    /// Which registered database. `None` asks.
    pub name: Option<&'a str>,
    /// `--sql`: one statement, run as given.
    pub sql: Option<&'a str>,
}

/// Read a database.
pub fn run(context: &Context<'_>, asked: &Asking<'_>) -> Outcome<Exit> {
    let (scope, name, database) = which_database(context, asked)?;
    let adapter = super::adapter_for(database.engine, context.global);

    crate::say!(
        "{} {}  {}",
        style::heading("Reading:"),
        style::paint(&name),
        style::dim(scope.label())
    );

    let (at, secret) = super::open(
        &database,
        scope,
        &context.registries,
        context.tunnels,
        context.password_command,
    )?;
    if let Some(server) = &at.through {
        crate::say!("  {}", style::dim(&format!("through {server}")));
    }

    // **Built once and borrowed, rather than rebuilt per statement.** Every page of every
    // query in this session goes to the same place, including a page fetched twenty minutes
    // in: the forward belongs to the session and the record has not moved.
    let target = database.target_at(&secret, &at.host, at.port);
    let ask = |sql: &str| -> Outcome<Rows> {
        adapter.read(
            &target,
            &Reading {
                sql,
                timeout: TIMEOUT,
            },
        )
    };

    match asked.sql {
        Some(sql) => straight(&ask, sql),
        None => build_one(&*adapter, &target, &ask, &name),
    }
}

/// `--sql`: run it and print what came back.
fn straight(ask: &impl Fn(&str) -> Outcome<Rows>, sql: &str) -> Outcome<Exit> {
    let rows = ask(sql)?;
    crate::report::result(document(&rows));
    print(&rows);
    Ok(Exit::Success)
}

/// The builder, from the table to the grid.
fn build_one(
    adapter: &dyn Adapter,
    target: &Target<'_>,
    ask: &impl Fn(&str) -> Outcome<Rows>,
    name: &str,
) -> Outcome<Exit> {
    // **Rule 4, one screen earlier than usual.** The builder is nothing but questions, so a
    // run with nobody to ask is refused at the door with the flag that would have answered
    // it — rather than at the first `Select`, having already connected.
    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            "building a query is a screen of questions, and there is no terminal to ask at",
        )
        .hint("give the statement instead: `sloop query <name> --sql \"select ...\"`"));
    }
    crate::ui::ask::dress();

    let shapes = adapter.shapes(target)?;
    if shapes.is_empty() {
        return Err(Failure::new(
            Exit::Usage,
            "there is not a single table in this database to read",
        )
        .hint(
            "check which database this name points at with `sloop db list` — a role that \
             cannot see a schema sees no tables in it either",
        ));
    }
    let keys = adapter.foreign_keys(target)?;

    let Some(built) = compose(&shapes, &keys)? else {
        crate::say!("{}", style::dim("Nothing was run."));
        return Ok(Exit::Success);
    };

    let sql = |page: u64| built.sql(adapter, page);
    let first = ask(&sql(0))?;
    let seen = grid::show(&built.from.to_string(), first, |page| ask(&sql(page)))?;

    // **After the screen is handed back, so it survives** — and as a line rather than as a
    // statement, which is `R20`. Nothing drawn inside the grid is in the scrollback; what
    // somebody wants afterwards is the thing they can paste, and `--sql` is exactly what
    // takes it. The menu prints no equivalent of its own for this job for that reason: it
    // knows which database was picked and nothing about the statement the builder wrote.
    crate::report::result(serde_json::json!({ "sql": sql(0), "rows_read": seen }));
    crate::say!();
    crate::say!("{} {seen}", style::heading("Rows read:"));
    crate::say!();
    crate::say!("  {}", style::dim("The same thing, from a shell:"));
    crate::say!(
        "  {}",
        style::paint(&format!(
            "sloop query {} --sql {}",
            style::as_argument(name),
            style::as_argument(&sql(0))
        ))
    );
    Ok(Exit::Success)
}

/// Ask every question, in order. `None` when somebody backed out of one of them.
fn compose(shapes: &[TableShape], keys: &[ForeignKey]) -> Outcome<Option<Built>> {
    let tables: Vec<Table> = shapes.iter().map(|shape| shape.table.clone()).collect();
    let Some(from) = pick("Which table?", &tables, Table::to_string)? else {
        return Ok(None);
    };

    let mut built = Built::everything_in(from);
    let Some(joins) = pick_joins(&built.from, keys, &tables, shapes)? else {
        return Ok(None);
    };
    built.joins = joins;

    let Some(columns) = pick_columns(shapes, &built)? else {
        return Ok(None);
    };
    built.columns = columns;

    let Some(conditions) = pick_conditions(shapes, &built)? else {
        return Ok(None);
    };
    built.conditions = conditions;

    if built.conditions.len() > 1 {
        let Some(joiner) = pick(
            "A row has to match…",
            &[Joiner::All, Joiner::Any],
            |joiner| joiner.label().to_owned(),
        )?
        else {
            return Ok(None);
        };
        built.joiner = joiner;
    }

    Ok(Some(built))
}

/// Which tables to bring in beside the first, offered from the keys it actually has.
fn pick_joins(
    from: &Table,
    keys: &[ForeignKey],
    tables: &[Table],
    shapes: &[TableShape],
) -> Outcome<Option<Vec<Join>>> {
    let mut joins: Vec<Join> = Vec::new();

    loop {
        let here: Vec<Table> = std::iter::once(from.clone())
            .chain(joins.iter().map(|join| join.table.clone()))
            .collect();

        let offered = offers(&here, keys);
        let mut rows: Vec<String> = offered.iter().map(Join::describe).collect();
        rows.push("Another table — I will say which columns match".to_owned());
        rows.push(if joins.is_empty() {
            "No, just this table".to_owned()
        } else {
            "That is all of them".to_owned()
        });

        let question = if joins.is_empty() {
            "Bring in another table beside it?"
        } else {
            "And another?"
        };
        let Some(chosen) = choose(question, &rows)? else {
            return Ok(None);
        };

        if chosen == rows.len() - 1 {
            return Ok(Some(joins));
        }
        if chosen == rows.len() - 2 {
            match by_hand(&here, tables, shapes)? {
                None => return Ok(None),
                Some(None) => return Ok(Some(joins)),
                Some(Some(join)) => joins.push(join),
            }
            continue;
        }
        joins.push(offered[chosen].clone());
    }
}

/// A join no key describes: the table, then the two columns that line up.
///
/// The inner `Option` is *"they changed their mind about joining"*, which is not the same as
/// backing out of the whole builder.
#[allow(clippy::option_option)]
fn by_hand(
    here: &[Table],
    tables: &[Table],
    shapes: &[TableShape],
) -> Outcome<Option<Option<Join>>> {
    let choices: Vec<Table> = tables
        .iter()
        .filter(|table| !here.contains(table))
        .cloned()
        .collect();
    if choices.is_empty() {
        crate::say!(
            "  {}",
            style::dim("every table in this database is already in the query")
        );
        return Ok(Some(None));
    }

    let Some(table) = pick("Which table?", &choices, Table::to_string)? else {
        return Ok(None);
    };

    // **Two pickers, never a typed condition.** The column already in the query, then the
    // column in the table being brought in — which is the whole of what a key would have
    // said, asked of somebody who knows their own schema.
    let mine = columns_of(here, shapes);
    let theirs = columns_of(std::slice::from_ref(&table), shapes);
    if mine.is_empty() || theirs.is_empty() {
        crate::say!(
            "  {}",
            style::dim("one of those tables reports no columns, so there is nothing to match")
        );
        return Ok(Some(None));
    }

    let Some(ours) = pick("Which column in the query?", &mine, Column::label)? else {
        return Ok(None);
    };
    let Some(matching) = pick(
        &format!("And which column in {}?", table.name),
        &theirs,
        Column::label,
    )?
    else {
        return Ok(None);
    };

    Ok(Some(Some(Join {
        table,
        on: vec![(ours, matching)],
    })))
}

/// Every join that can be offered, given what is already in the query.
///
/// **Both directions of every key, which is what makes this useful.** A key only points one
/// way: from `order` it answers *"and who placed it"*, and from `customer` the same key
/// answers *"and their orders"*. Offering one of the two would leave half the joins anybody
/// wants unreachable from the side they started on.
///
/// A table already in the query is never offered again — it would join to itself through a
/// key it does not have, and the statement would be wrong rather than merely odd.
///
/// Its own function because it is the one piece of this screen that is easy to get wrong and
/// has nothing to do with a terminal: two keys between the same pair of tables, a key that
/// points at a table already brought in, a self-reference. All three are checked without a
/// prompt anywhere near them.
fn offers(here: &[Table], keys: &[ForeignKey]) -> Vec<Join> {
    let mut offered: Vec<Join> = Vec::new();
    let mut add = |join: Join| {
        if !offered
            .iter()
            .any(|seen| seen.table == join.table && seen.on == join.on)
        {
            offered.push(join);
        }
    };

    for key in keys {
        if here.contains(&key.from) && !here.contains(&key.to) {
            add(Join::from_key(key));
        }
        if here.contains(&key.to) && !here.contains(&key.from) {
            add(Join::from_key_backwards(key));
        }
    }
    offered
}

/// What a set of ticks means.
///
/// **`All` at the top, and an empty tick, are the same answer.** A query of no columns is
/// not a query, and a screen that punished pressing Enter would be a screen somebody has to
/// be told about. Index `0` is the `All` row, so every other index is one past its column.
fn wanted_from(ticked: &[usize], columns: &[Column]) -> Wanted {
    if ticked.is_empty() || ticked.contains(&0) {
        return Wanted::Everything;
    }

    Wanted::These(
        ticked
            .iter()
            .filter_map(|at| at.checked_sub(1).and_then(|at| columns.get(at)).cloned())
            .collect(),
    )
}

/// Every column of these tables, in the order the engine reports them.
fn columns_of(tables: &[Table], shapes: &[TableShape]) -> Vec<Column> {
    let mut columns = Vec::new();
    for table in tables {
        let Some(shape) = shapes.iter().find(|shape| &shape.table == table) else {
            continue;
        };
        columns.extend(shape.columns.iter().map(|name| Column {
            table: table.clone(),
            name: name.clone(),
        }));
    }
    columns
}

/// The column checklist, with `All` at the top.
///
/// **`All` is a row on the list rather than a second question.** Ticking it is `select *`,
/// ticking three is those three — which is exactly how the owner described it, and one
/// keystroke either way.
fn pick_columns(shapes: &[TableShape], built: &Built) -> Outcome<Option<Wanted>> {
    let columns = columns_of(&built.tables(), shapes);

    let mut rows = vec!["All of them".to_owned()];
    rows.extend(columns.iter().map(Column::label));

    let Some(ticked) = tick("Which columns?", &rows)? else {
        return Ok(None);
    };

    Ok(Some(wanted_from(&ticked, &columns)))
}

/// The `WHERE`, one flat line at a time.
fn pick_conditions(shapes: &[TableShape], built: &Built) -> Outcome<Option<Vec<Condition>>> {
    let columns = columns_of(&built.tables(), shapes);
    if columns.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let mut conditions: Vec<Condition> = Vec::new();
    loop {
        let question = if conditions.is_empty() {
            "Narrow it down?"
        } else {
            "Another condition?"
        };
        match Confirm::new(question)
            .with_default(!conditions.is_empty())
            .with_help_message("every row comes back if you say no")
            .prompt()
        {
            Ok(false) => return Ok(Some(conditions)),
            Ok(true) => {}
            Err(error) => return backed_out(&error).map(|()| None),
        }

        let Some(column) = pick("Which column?", &columns, Column::label)? else {
            return Ok(None);
        };
        let Some(operator) = pick("And it…", &Operator::ALL, |operator| {
            operator.label().to_owned()
        })?
        else {
            return Ok(None);
        };

        let value = if operator.takes_a_value() {
            match Text::new("What are you looking for?").prompt() {
                Ok(typed) => typed,
                Err(error) => return backed_out(&error).map(|()| None),
            }
        } else {
            String::new()
        };

        conditions.push(Condition {
            column,
            operator,
            value,
        });
    }
}

/// Which database this is about.
fn which_database(context: &Context<'_>, asked: &Asking<'_>) -> Outcome<(Scope, String, Database)> {
    if let Some(name) = asked.name {
        let (scope, database) = context.registries.find(name)?;
        return Ok((scope, name.to_owned(), database.clone()));
    }

    let known: Vec<(Scope, String, Database)> = context
        .registries
        .all()
        .map(|(scope, name, database)| (scope, name.to_owned(), database.clone()))
        .collect();

    if known.is_empty() {
        return Err(
            Failure::new(Exit::Usage, "there is nothing registered here to read")
                .hint("`sloop db add <name>` tells sloop about a database that already exists"),
        );
    }
    if known.len() == 1 {
        return Ok(known.into_iter().next().expect("one of them"));
    }
    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            "which database should be read? There is no terminal to ask at",
        )
        .hint("name it: `sloop query <name>`"));
    }

    crate::ui::ask::dress();
    let rows: Vec<String> = known
        .iter()
        .map(|(scope, name, database)| {
            format!("{name}  {}  {}", database.credential_key(), scope.label())
        })
        .collect();
    let Some(chosen) = choose("Which database?", &rows)? else {
        return Err(
            Failure::new(Exit::Usage, "nothing was chosen, so nothing was read")
                .hint("name it instead of picking it: `sloop query <name>`"),
        );
    };
    Ok(known.into_iter().nth(chosen).expect("it was on the list"))
}

/// The rows as the one document a `--json` run prints. `NULL` is `null`, not `"NULL"`.
fn document(rows: &Rows) -> serde_json::Value {
    serde_json::json!({
        "columns": rows.columns,
        "rows": rows
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| match cell {
                        crate::engine::Cell::Null => serde_json::Value::Null,
                        crate::engine::Cell::Text(text) => {
                            serde_json::Value::String(text.clone())
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>(),
    })
}

/// What `--sql` prints: the columns, then the rows, in aligned plain text.
fn print(rows: &Rows) {
    if rows.columns.is_empty() {
        crate::say!("{}", style::dim("That statement returned nothing."));
        return;
    }

    let widths: Vec<usize> = rows
        .columns
        .iter()
        .enumerate()
        .map(|(at, name)| {
            rows.rows
                .iter()
                .filter_map(|row| row.get(at))
                .map(|cell| one_line(cell.shown()).chars().count())
                .chain(std::iter::once(name.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    crate::say!();
    crate::say!(
        "{}",
        style::heading(&padded(&rows.columns, &widths, Clone::clone))
    );
    for row in &rows.rows {
        crate::say!("{}", padded(row, &widths, |cell| one_line(cell.shown())));
    }
    crate::say!();
    crate::say!("{} {}", style::heading("Rows:"), rows.rows.len());
}

/// One row of the printed table, padded to the column widths.
///
/// Padded before anything is styled, so an escape sequence never counts towards a column —
/// the same rule `init`'s rows follow.
fn padded<T>(row: &[T], widths: &[usize], text: impl Fn(&T) -> String) -> String {
    row.iter()
        .enumerate()
        .map(|(at, value)| {
            let width = widths.get(at).copied().unwrap_or(0);
            format!("{:<width$}", text(value))
        })
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_owned()
}

/// A value on one line, whatever it did.
fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|letter| if letter.is_control() { '·' } else { letter })
        .collect()
}

/// Choose one of a list of values, by a label. `None` when somebody backed out.
fn pick<T: Clone>(
    question: &str,
    values: &[T],
    label: impl Fn(&T) -> String,
) -> Outcome<Option<T>> {
    let rows: Vec<String> = values.iter().map(label).collect();
    Ok(choose(question, &rows)?.and_then(|at| values.get(at).cloned()))
}

/// Choose one row, and say which. `None` when somebody backed out.
fn choose(question: &str, rows: &[String]) -> Outcome<Option<usize>> {
    match Select::new(question, rows.to_vec())
        .with_help_message("↑↓ to move, Enter to choose, Esc to go back")
        .raw_prompt()
    {
        Ok(chosen) => Ok(Some(chosen.index)),
        Err(error) => backed_out(&error).map(|()| None),
    }
}

/// Tick as many as apply, and say which. `None` when somebody backed out.
fn tick(question: &str, rows: &[String]) -> Outcome<Option<Vec<usize>>> {
    match MultiSelect::new(question, rows.to_vec())
        .with_help_message("Space to tick, ↑↓ to move, Enter when done, Esc to go back")
        .raw_prompt()
    {
        Ok(ticked) => Ok(Some(ticked.into_iter().map(|one| one.index).collect())),
        Err(error) => backed_out(&error).map(|()| None),
    }
}

/// Esc and Ctrl-C are answers; anything else is a terminal that stopped working.
fn backed_out(error: &inquire::InquireError) -> Outcome<()> {
    match error {
        inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted => {
            Ok(())
        }
        other => Err(Failure::new(
            Exit::Usage,
            format!("the question could not be asked: {other}"),
        )
        .hint("give the statement instead: `sloop query <name> --sql \"select ...\"`")),
    }
}
