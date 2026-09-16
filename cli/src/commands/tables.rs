//! Which tables a copy is about, when it is not about all of them — `R15a`.
//!
//! **`--table` is repeatable, schema-qualified and takes a glob. It is not a regex**, and that
//! is a decision rather than an omission: `audit_*` is what people mean, `^audit_.*$` is what
//! they have to look up, and a regex that matches more than it was meant to on a `mirror` is a
//! regex that drops tables nobody named. `*` is any run of characters and `?` is one; nothing
//! else is special.
//!
//! **A pattern with a dot in it names a schema**, split at the first one: `public.audit_*` is
//! the `audit_` tables in `public`, and `audit_*` is the `audit_` tables in any schema. The
//! first dot rather than the last, because a schema name cannot contain one in any engine here
//! and a table's name can.
//!
//! **A pattern that matches nothing is an error.** A typo in `--table` that quietly copies
//! nothing is the worst outcome available: the run succeeds, says it copied zero tables, and
//! nobody reads it until the day they need what was not copied.
//!
//! **Foreign keys are the reason this is not just a filter.** A child table without the
//! parents its keys point at cannot be loaded at all — the rows fail the constraint — so a
//! selection that is missing a parent is refused with the parents named, and
//! `--with-references` pulls them in instead, following the chain the whole way up.

#[cfg(test)]
#[path = "tables_tests.rs"]
mod tests;

use std::collections::BTreeSet;

use crate::engine::{Table, TableShape};
use crate::failure::{Failure, Outcome};

/// What `--table` and `--with-references` were given.
pub struct Selection<'a> {
    /// The patterns, in the order they were typed.
    pub patterns: &'a [String],
    /// `--with-references`: pull in the parents rather than refusing without them.
    pub with_references: bool,
}

impl Selection<'_> {
    /// Was anything named at all?
    #[must_use]
    pub const fn is_everything(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Cut `shapes` down to what was named, and to what those tables need.
    ///
    /// The order of the result is the order of `shapes`, which the caller has already sorted
    /// or is about to — this only decides membership.
    pub fn choose(&self, shapes: &[TableShape]) -> Outcome<Vec<TableShape>> {
        if self.is_everything() {
            return Ok(shapes.to_vec());
        }

        let mut chosen: BTreeSet<Table> = BTreeSet::new();
        for pattern in self.patterns {
            let matched: Vec<&Table> = shapes
                .iter()
                .map(|shape| &shape.table)
                .filter(|table| matches(pattern, table))
                .collect();

            if matched.is_empty() {
                return Err(Failure::usage(format!(
                    "--table {pattern} matches no table in the source"
                ))
                .hint(
                    "a name, or a glob like `audit_*`, optionally with a schema in front. \
                     `sloop db test <name>` says what is there",
                ));
            }
            chosen.extend(matched.into_iter().cloned());
        }

        self.settle_the_references(shapes, &mut chosen)?;

        Ok(shapes
            .iter()
            .filter(|shape| chosen.contains(&shape.table))
            .cloned()
            .collect())
    }

    /// Add the parents, or refuse because they are missing.
    ///
    /// **Followed the whole way up.** Pulling in a table's parents can pull in tables with
    /// parents of their own, so this repeats until a pass adds nothing — otherwise
    /// `--with-references` would fix one level and fail on the next.
    fn settle_the_references(
        &self,
        shapes: &[TableShape],
        chosen: &mut BTreeSet<Table>,
    ) -> Outcome<()> {
        let known: BTreeSet<&Table> = shapes.iter().map(|shape| &shape.table).collect();

        loop {
            let wanted: BTreeSet<Table> = shapes
                .iter()
                .filter(|shape| chosen.contains(&shape.table))
                .flat_map(|shape| shape.references.iter())
                .filter(|parent| {
                    // A parent outside the source entirely is not this command's to find;
                    // one already chosen is not missing.
                    known.contains(parent) && !chosen.contains(parent)
                })
                .cloned()
                .collect();

            if wanted.is_empty() {
                return Ok(());
            }

            if !self.with_references {
                let named: Vec<String> = wanted.iter().map(Table::to_string).collect();
                return Err(Failure::usage(format!(
                    "{} {} named, and {} point{} at {}: {}",
                    plural_tables(chosen.len()),
                    if chosen.len() == 1 { "is" } else { "are" },
                    if chosen.len() == 1 { "it" } else { "they" },
                    if chosen.len() == 1 { "s" } else { "" },
                    plural_tables(named.len()),
                    named.join(", ")
                ))
                .hint(
                    "rows cannot be loaded into a table whose parents are not there. Name \
                     them too, or pass --with-references to pull them in",
                ));
            }

            chosen.extend(wanted);
        }
    }
}

/// "one table" / "three tables", for the sentence above.
fn plural_tables(how_many: usize) -> String {
    if how_many == 1 {
        "1 table".to_owned()
    } else {
        format!("{how_many} tables")
    }
}

/// Does this pattern name this table?
///
/// A dot in the pattern splits it into a schema and a name, at the first dot. Without one the
/// pattern is matched against the table's name alone, in whichever schema it lives.
pub fn matches(pattern: &str, table: &Table) -> bool {
    match pattern.split_once('.') {
        Some((schema, name)) => glob(schema, &table.schema) && glob(name, &table.name),
        None => glob(pattern, &table.name),
    }
}

/// `*` is any run of characters, `?` is exactly one, everything else is itself.
///
/// **Written out rather than taken from a crate.** It is twenty lines, it is the whole of what
/// `R15a` asks for, and a dependency in this graph has to earn itself — the guarantee is that
/// `cargo tree` stays short enough for somebody to read.
///
/// Backtracking on `*` rather than recursion: the pattern and the subject are both short, and
/// a recursive matcher on `a*a*a*a*b` against a long name is how a glob becomes a hang.
fn glob(pattern: &str, subject: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let subject: Vec<char> = subject.chars().collect();

    // Where the last `*` was, and how much of the subject had been eaten when it was met, so
    // a failure later can come back and let that `*` swallow one more character.
    let (mut at, mut here) = (0, 0);
    let (mut star, mut resume) = (None, 0);

    while here < subject.len() {
        let taken = match pattern.get(at) {
            Some('*') => {
                star = Some(at);
                at += 1;
                resume = here;
                continue;
            }
            Some('?') => true,
            Some(letter) => letter.eq_ignore_ascii_case(&subject[here]),
            None => false,
        };

        if taken {
            at += 1;
            here += 1;
        } else if let Some(last) = star {
            at = last + 1;
            resume += 1;
            here = resume;
        } else {
            return false;
        }
    }

    // Whatever is left of the pattern has to be stars, or there is more pattern than subject.
    pattern[at..].iter().all(|letter| *letter == '*')
}
