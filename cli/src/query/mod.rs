//! A query somebody built by choosing, and the SQL it turns into.
//!
//! **Nothing is ever typed that is not a value.** The table, the columns, the operator, the
//! joiner and every join come off a list; the only characters a person types are the thing
//! they are looking for. That is `R19a`'s rule and it is a stronger property than validation
//! — a destructive statement is not *expressible* here rather than being rejected once
//! written, so there is nothing for a check to miss.
//!
//! **The `WHERE` is a flat list, with no nested parentheses.** Nesting needs a tree editor,
//! and a tree editor is where a screen a beginner can operate stops being one. The owner's
//! *"must not be complex so that even a \[beginner\] can use it"* and an arbitrary boolean
//! expression pull against each other in exactly one place, and it was resolved toward the
//! beginner. See "Where `ratatui` starts" in `docs/OWNER-DECISIONS.md`.
//!
//! **Read-only is still the server's job, not this module's.** Everything here builds a
//! `SELECT`, so nothing it can produce writes — but that is a property of the builder, and
//! the flag form's `--sql` does not go through the builder at all. What guarantees the rule
//! for both is [`crate::engine::Adapter::read`], which runs every statement inside a
//! transaction the engine has been told is read-only.

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub mod grid;

use std::fmt::Write as _;

use crate::engine::{Adapter, ForeignKey, Table};

/// How many rows one page of results holds.
///
/// **This is what makes a million-row table openable.** The page is a `LIMIT` in the
/// statement rather than a window over everything the server sent, so a table nobody could
/// print is a table the server returns two hundred rows of.
pub const PAGE: u64 = 200;

/// A column, and which table in the query it came from.
///
/// **The table travels with the name because a join makes names ambiguous.** Two tables with
/// an `id` each are ordinary, and a condition on "id" that does not say whose is a condition
/// the server refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Which table.
    pub table: Table,
    /// The column's own name.
    pub name: String,
}

impl Column {
    /// How it reads on a screen: `orders.total`, never quoted.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}.{}", self.table.name, self.name)
    }
}

/// Which columns come back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// `All`, ticked at the top of the checklist — every column of every table in the query.
    Everything,
    /// The ones that were ticked, in the order they were offered.
    These(Vec<Column>),
}

/// What two things are compared with.
///
/// **Named the way somebody would say it**, not the way SQL spells it. The owner's rule for
/// the whole menu — *"totally very very much user friendly"* — and this is the list where it
/// bites hardest: `<>` and `NOT LIKE '%x%'` are the two operators people get wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// Exactly this.
    Is,
    /// Anything but this.
    IsNot,
    /// This appears somewhere in it.
    Contains,
    /// It begins with this.
    StartsWith,
    /// It ends with this.
    EndsWith,
    /// Greater than.
    Above,
    /// Less than.
    Below,
    /// Greater than or equal to.
    AtLeast,
    /// Less than or equal to.
    AtMost,
    /// There is no value at all.
    IsEmpty,
    /// There is some value.
    IsNotEmpty,
}

impl Operator {
    /// Every one, in the order the list offers them.
    pub const ALL: [Self; 11] = [
        Self::Is,
        Self::IsNot,
        Self::Contains,
        Self::StartsWith,
        Self::EndsWith,
        Self::Above,
        Self::Below,
        Self::AtLeast,
        Self::AtMost,
        Self::IsEmpty,
        Self::IsNotEmpty,
    ];

    /// How it reads on the list.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Is => "is",
            Self::IsNot => "is not",
            Self::Contains => "contains",
            Self::StartsWith => "starts with",
            Self::EndsWith => "ends with",
            Self::Above => "is more than",
            Self::Below => "is less than",
            Self::AtLeast => "is at least",
            Self::AtMost => "is at most",
            Self::IsEmpty => "is empty",
            Self::IsNotEmpty => "is not empty",
        }
    }

    /// Does this one need something typed after it?
    ///
    /// The two that do not are the two that ask about the absence of a value, and a box
    /// waiting for a value that has no part in the question is a box somebody has to guess at.
    #[must_use]
    pub const fn takes_a_value(self) -> bool {
        !matches!(self, Self::IsEmpty | Self::IsNotEmpty)
    }
}

/// How the conditions are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Joiner {
    /// Every condition has to hold.
    All,
    /// Any one of them is enough.
    Any,
}

impl Joiner {
    /// How it reads on the list.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "all of them",
            Self::Any => "any of them",
        }
    }

    /// The word SQL uses.
    const fn keyword(self) -> &'static str {
        match self {
            Self::All => "AND",
            Self::Any => "OR",
        }
    }
}

/// One line of the `WHERE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    /// Which column.
    pub column: Column,
    /// What is being asked about it.
    pub operator: Operator,
    /// The one thing anybody typed. Empty for the two operators that take nothing.
    pub value: String,
}

/// One table brought in beside another, and the columns that line them up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    /// The table being brought in.
    pub table: Table,
    /// Column pairs: one already in the query, one in [`Join::table`].
    pub on: Vec<(Column, Column)>,
}

impl Join {
    /// The join a foreign key describes, in the direction it points.
    #[must_use]
    pub fn from_key(key: &ForeignKey) -> Self {
        Self {
            table: key.to.clone(),
            on: key
                .pairs()
                .into_iter()
                .map(|(here, there)| {
                    (
                        Column {
                            table: key.from.clone(),
                            name: here,
                        },
                        Column {
                            table: key.to.clone(),
                            name: there,
                        },
                    )
                })
                .collect(),
        }
    }

    /// The same key read backwards: the table that *points at* this one, brought in beside it.
    ///
    /// **Both directions are offered, because a key only points one way and a person wants
    /// either.** From `customer`, the key on `order` is the one that answers *"and their
    /// orders"*; from `order`, the same key answers *"and who placed it"*.
    #[must_use]
    pub fn from_key_backwards(key: &ForeignKey) -> Self {
        Self {
            table: key.from.clone(),
            on: key
                .pairs()
                .into_iter()
                .map(|(there, here)| {
                    (
                        Column {
                            table: key.to.clone(),
                            name: here,
                        },
                        Column {
                            table: key.from.clone(),
                            name: there,
                        },
                    )
                })
                .collect(),
        }
    }

    /// How it reads on a screen.
    #[must_use]
    pub fn describe(&self) -> String {
        let pairs: Vec<String> = self
            .on
            .iter()
            .map(|(here, there)| format!("{} = {}", here.label(), there.label()))
            .collect();
        format!("{} on {}", self.table.name, pairs.join(" and "))
    }
}

/// A query, as a set of answers rather than as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    /// The table it starts from.
    pub from: Table,
    /// Which columns come back.
    pub columns: Wanted,
    /// Tables brought in beside it.
    pub joins: Vec<Join>,
    /// The `WHERE`, flat.
    pub conditions: Vec<Condition>,
    /// How those conditions are joined.
    pub joiner: Joiner,
}

impl Built {
    /// Everything in one table, which is what picking a table and ticking `All` means.
    #[must_use]
    pub fn everything_in(table: Table) -> Self {
        Self {
            from: table,
            columns: Wanted::Everything,
            joins: Vec::new(),
            conditions: Vec::new(),
            joiner: Joiner::All,
        }
    }

    /// Every table this query touches, in the order it touches them.
    #[must_use]
    pub fn tables(&self) -> Vec<Table> {
        let mut tables = vec![self.from.clone()];
        tables.extend(self.joins.iter().map(|join| join.table.clone()));
        tables
    }

    /// The statement, for one page of results.
    ///
    /// **`LIMIT` and `OFFSET` are in the statement rather than applied to what came back.**
    /// That is the whole of *"a million-row table pages without the terminal stalling"*: the
    /// server is asked for two hundred rows and sends two hundred rows, so nothing between
    /// here and the screen ever holds a million of anything.
    ///
    /// One row past the page is asked for, and it is the cheapest honest way to know whether
    /// there is a next page. Counting the whole result to find out would cost more than the
    /// page itself on the table where it matters.
    #[must_use]
    pub fn sql(&self, adapter: &dyn Adapter, page: u64) -> String {
        let mut sql = format!(
            "SELECT {} FROM {}",
            self.select_list(adapter),
            Self::named(adapter, &self.from)
        );

        for join in &self.joins {
            let _ = write!(
                sql,
                " LEFT JOIN {} ON {}",
                Self::named(adapter, &join.table),
                join.on
                    .iter()
                    .map(|(here, there)| format!(
                        "{} = {}",
                        Self::column(adapter, here),
                        Self::column(adapter, there)
                    ))
                    .collect::<Vec<_>>()
                    .join(" AND ")
            );
        }

        let tests: Vec<String> = self
            .conditions
            .iter()
            .map(|condition| Self::test(adapter, condition))
            .collect();
        if !tests.is_empty() {
            let _ = write!(
                sql,
                " WHERE {}",
                tests.join(&format!(" {} ", self.joiner.keyword()))
            );
        }

        let _ = write!(sql, " LIMIT {} OFFSET {}", PAGE + 1, page * PAGE);
        sql
    }

    /// What goes between `SELECT` and `FROM`.
    ///
    /// **`All` is `*` only while there is one table.** Once something is joined in, two
    /// tables with an `id` each would come back as two columns called `id`, and a grid with
    /// two identically-named columns is a grid nobody can read — so every column is named,
    /// and aliased to the label the screen already uses.
    fn select_list(&self, adapter: &dyn Adapter) -> String {
        match &self.columns {
            Wanted::Everything if self.joins.is_empty() => "*".to_owned(),
            Wanted::Everything => self
                .tables()
                .iter()
                .map(|table| format!("{}.*", Self::named(adapter, table)))
                .collect::<Vec<_>>()
                .join(", "),
            Wanted::These(columns) if columns.is_empty() => "*".to_owned(),
            Wanted::These(columns) => columns
                .iter()
                .map(|column| {
                    if self.joins.is_empty() {
                        Self::column(adapter, column)
                    } else {
                        format!(
                            "{} AS {}",
                            Self::column(adapter, column),
                            adapter.quoted_name(&column.label())
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// One line of the `WHERE`, in this engine's spelling.
    fn test(adapter: &dyn Adapter, condition: &Condition) -> String {
        let column = Self::column(adapter, &condition.column);
        let value = |text: &str| adapter.quoted_value(text);
        // **Every wildcard sloop adds, never one the user typed.** `%` and `_` are
        // `LIKE`'s, so a search for a literal `100%` has to arrive as `100\%` or it matches
        // everything beginning with `100`. The escape is named in the statement rather than
        // left to the engine's default, because MySQL's and PostgreSQL's differ.
        let like = |pattern: String| format!("{column} LIKE {} ESCAPE '\\'", value(&pattern));
        let escaped = escape_wildcards(&condition.value);

        match condition.operator {
            Operator::Is => format!("{column} = {}", value(&condition.value)),
            Operator::IsNot => format!("{column} <> {}", value(&condition.value)),
            Operator::Contains => like(format!("%{escaped}%")),
            Operator::StartsWith => like(format!("{escaped}%")),
            Operator::EndsWith => like(format!("%{escaped}")),
            Operator::Above => format!("{column} > {}", value(&condition.value)),
            Operator::Below => format!("{column} < {}", value(&condition.value)),
            Operator::AtLeast => format!("{column} >= {}", value(&condition.value)),
            Operator::AtMost => format!("{column} <= {}", value(&condition.value)),
            Operator::IsEmpty => format!("{column} IS NULL"),
            Operator::IsNotEmpty => format!("{column} IS NOT NULL"),
        }
    }

    /// A table, quoted for this engine.
    ///
    /// **Schema-qualified only where a schema means something.** MySQL puts the database
    /// name in that position — see [`Table`] — and naming it would send every query through
    /// a cross-database reference that the connected user may not be granted.
    fn named(adapter: &dyn Adapter, table: &Table) -> String {
        if adapter.engine() == crate::engine::Engine::Postgres {
            format!(
                "{}.{}",
                adapter.quoted_name(&table.schema),
                adapter.quoted_name(&table.name)
            )
        } else {
            adapter.quoted_name(&table.name)
        }
    }

    /// A column, quoted, and qualified by its table.
    fn column(adapter: &dyn Adapter, column: &Column) -> String {
        format!(
            "{}.{}",
            Self::named(adapter, &column.table),
            adapter.quoted_name(&column.name)
        )
    }
}

/// Take the meaning out of `%` and `_` so a search for them finds them.
///
/// The backslash is doubled first, or escaping the wildcards would be undone by the engine
/// reading the backslash that escapes them as an escaped backslash.
fn escape_wildcards(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
