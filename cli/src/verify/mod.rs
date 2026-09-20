//! Proving a copy landed: count both sides, and say exactly how they differ.
//!
//! **Exact, and not estimated.** Every table on both sides is counted with `count(*)` —
//! for PostgreSQL that is one `query_to_xml` statement against one snapshot, for the MySQL
//! family one `UNION ALL`. The engines' own statistics are available through
//! [`Mode::Fast`], are labelled an estimate wherever they appear, and are never presented
//! as proof. That is not a theoretical caution: an estimate reported double the true count
//! on a table this project was looking at, and a freshly restored database reports zero for
//! every table it has until something collects statistics for it.
//!
//! **Drift and loss are different things, and telling them apart is the point.** A source
//! that is still being written to gains and loses rows while a dump is being taken, so the
//! two sides disagreeing by a few rows is the normal state of a live database rather than a
//! fault. What is not normal is a table arriving with nothing in it, or not arriving at
//! all:
//!
//! ```text
//! both sides have rows, numbers differ    drift     said in those words, not an error
//! the source had rows, the copy has none  nothing arrived    a failure, exit 6
//! the table is not on the destination     missing           a failure, exit 6
//! only the destination has it             reported; mirror and sync judge it
//! ```
//!
//! **An estimate never fails a run**, whichever way it points. `n_live_tup` on a restored
//! database is zero until something analyses it, so a fast run that could exit `6` would
//! report a perfect copy as a catastrophe. Under [`Mode::Fast`] only a table that is
//! genuinely *not there* — a fact from the catalogue rather than from a statistic — is a
//! failure.
//!
//! Nothing calls this yet: `R13`'s `restore`, `R14`'s `mirror` and `R15`'s `sync` are the
//! three commands that will, and all three need the same answer in the same words. Settling
//! it once, here, is what stops them growing three slightly different ideas of what a
//! verified copy is.
#![allow(dead_code)]

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod cluster_tests;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use crate::backup::manifest::Manifest;
use crate::engine::{Adapter, Engine, Table, TableCount, Target};
use crate::exit::Exit;
use crate::failure::Outcome;
use crate::style;

/// The variable that asks for the fast answer instead of the true one.
///
/// **`SLOOP_VERIFY`, and `CLAUDE.md` says `VERIFY`.** The owner was asked and chose the
/// namespaced spelling: everything else sloop reads is `SLOOP_PROJECT` or
/// `SLOOP_PASSPHRASE`, and a bare `VERIFY` already means something in enough CI systems
/// that a backup tool could quietly stop counting because of a variable meant for
/// something else. Recorded in `docs/OWNER-DECISIONS.md`; the brief stays as written.
pub const VARIABLE: &str = "SLOOP_VERIFY";

/// How hard to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// `count(*)` on every table, on both sides. The default, and the only one that proves
    /// anything.
    #[default]
    Exact,
    /// The engine's own statistics: free, stale, and sometimes wildly wrong.
    Fast,
}

impl Mode {
    /// `SLOOP_VERIFY=fast`, or the exact count.
    #[must_use]
    pub fn from_environment() -> Self {
        Self::from_value(std::env::var(VARIABLE).ok().as_deref())
    }

    /// The same decision, without the environment, so it can be tested.
    ///
    /// Anything other than `fast` is the exact count, including `SLOOP_VERIFY=exact`,
    /// `SLOOP_VERIFY=1` and an empty value. The safe way to read a value nobody planned
    /// for is as the answer that is true, so only the one word asks for the other one.
    #[must_use]
    pub fn from_value(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("fast") => Self::Fast,
            _ => Self::Exact,
        }
    }

    /// Whether these numbers are guesses.
    #[must_use]
    pub const fn is_estimate(self) -> bool {
        matches!(self, Self::Fast)
    }

    /// How the numbers were arrived at, for the line above the table.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Exact => "exact count(*) on both sides",
            Self::Fast => "estimates from the engine's own statistics — not proof",
        }
    }

    /// Count a database the way this mode says to.
    pub fn count(self, adapter: &dyn Adapter, target: &Target<'_>) -> Outcome<Vec<TableCount>> {
        match self {
            Self::Exact => adapter.row_counts(target),
            Self::Fast => adapter.estimated_row_counts(target),
        }
    }
}

/// One end of a comparison.
pub struct Side {
    /// Which engine answered.
    pub engine: Engine,
    /// The database's own name on the server, which is also the MySQL family's schema.
    pub database: String,
    /// What it holds, table by table.
    pub counts: Vec<TableCount>,
}

impl Side {
    /// Build one from a live connection.
    pub fn counted(adapter: &dyn Adapter, target: &Target<'_>, mode: Mode) -> Outcome<Self> {
        Ok(Self {
            engine: target.engine,
            database: target.database.to_owned(),
            counts: mode.count(adapter, target)?,
        })
    }

    /// The same side, with only the tables a scoped run is about.
    ///
    /// **`R15a`.** A `mirror --table audit_*` copies two tables into a destination that has
    /// forty; comparing the source's forty against the destination's forty would be comparing
    /// numbers that were never meant to agree, and comparing forty against two would report a
    /// loss that did not happen. So both sides are narrowed to what the run touched, and the
    /// verification means what it says again.
    #[must_use]
    pub fn narrowed_to(mut self, wanted: impl Fn(&crate::engine::Table) -> bool) -> Self {
        self.counts.retain(|count| wanted(&count.table));
        self
    }

    /// Build one from what a backup's manifest recorded.
    ///
    /// **A restore has no live source to count.** What it has is the exact `count(*)` per
    /// table taken from the source immediately before the dump — which is a better source
    /// than a second query would be, because it is the moment the dump describes rather than
    /// whatever the source holds now. So `restore` verifies against the manifest and gets the
    /// same comparison, in the same words, as `mirror` and `sync` get from two connections.
    #[must_use]
    pub fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            engine: manifest.engine,
            database: manifest.database.clone(),
            counts: manifest
                .tables
                .iter()
                .map(|count| TableCount {
                    table: Table {
                        schema: count.schema.clone(),
                        name: count.name.clone(),
                    },
                    rows: count.rows,
                })
                .collect(),
        }
    }

    /// Does this engine put the database's own name where a schema goes?
    const fn schema_is_the_database(&self) -> bool {
        matches!(self.engine, Engine::Mysql | Engine::Mariadb)
    }
}

/// What happened to one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finding {
    /// The same number on both sides.
    Matched { rows: u64 },
    /// Both have it and the numbers differ. A live source does this by existing.
    Drifted { source: u64, destination: u64 },
    /// The source had rows and the destination's copy is empty: nothing arrived.
    Empty { source: u64 },
    /// The destination has no such table at all.
    Missing { source: u64 },
    /// The destination has a table the source does not.
    ///
    /// Not judged here. `mirror` ends with a destination identical to the source, so one
    /// of these means its drop-and-restore missed something; `sync` keeps whatever the
    /// destination already had, so one of these is the feature working.
    OnlyInDestination { rows: u64 },
}

impl Finding {
    /// Does this stop the run, given how the numbers were arrived at?
    ///
    /// **An estimate never fails anything.** `n_live_tup` is zero on every table of a
    /// database that has just been restored, so a fast run that could fail would report a
    /// perfect copy as a total loss. What survives into fast mode is the one finding that
    /// comes from the catalogue rather than from a statistic: the table is not there.
    #[must_use]
    pub const fn is_failure(self, mode: Mode) -> bool {
        match self {
            Self::Missing { .. } => true,
            Self::Empty { .. } => matches!(mode, Mode::Exact),
            Self::Matched { .. } | Self::Drifted { .. } | Self::OnlyInDestination { .. } => false,
        }
    }

    /// What it means, in the words this project uses for it.
    ///
    /// The mode is part of the sentence rather than a footnote under it. "Nothing arrived"
    /// is a statement of fact when it comes from a count and a suspicion when it comes from
    /// a statistic, and a report that said the same words for both would be claiming
    /// something an estimate cannot support.
    #[must_use]
    pub const fn explain(self, mode: Mode) -> &'static str {
        match (self, mode) {
            (Self::Matched { .. }, _) => "",
            (Self::Drifted { .. }, _) => "drift — the source is live and changed while this ran",
            (Self::Empty { .. }, Mode::Exact) => "nothing arrived",
            (Self::Empty { .. }, Mode::Fast) => {
                "the estimate says nothing arrived — which an estimate cannot be trusted on"
            }
            (Self::Missing { .. }, _) => "the table is not there",
            (Self::OnlyInDestination { .. }, _) => "only in the destination",
        }
    }
}

/// One table, and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// How the table reads in the report.
    pub table: String,
    /// What happened.
    pub finding: Finding,
}

/// Both sides, table by table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// How the numbers were arrived at.
    pub mode: Mode,
    /// Every table either side has, in name order.
    pub rows: Vec<Row>,
    /// Whether the two sides were matched on bare table names because their engines do
    /// not name schemas the same way.
    matched_by_name_only: bool,
}

impl Comparison {
    /// Compare what two databases hold.
    #[must_use]
    pub fn of(mode: Mode, source: &Side, destination: &Side) -> Self {
        // A MySQL database *is* its schema, so `shop.orders` copied into `shop_staging`
        // arrives as `shop_staging.orders` and comparing the qualified names would report
        // every table as missing. Between two engines that disagree about that, the bare
        // table name is the only thing both sides mean the same way.
        let bare = source.schema_is_the_database() || destination.schema_is_the_database();
        let key = |count: &TableCount| {
            if bare {
                count.table.name.clone()
            } else {
                count.table.to_string()
            }
        };

        let left: BTreeMap<String, u64> = source
            .counts
            .iter()
            .map(|count| (key(count), count.rows))
            .collect();
        let right: BTreeMap<String, u64> = destination
            .counts
            .iter()
            .map(|count| (key(count), count.rows))
            .collect();

        let mut rows: Vec<Row> = left
            .iter()
            .map(|(table, &source_rows)| Row {
                table: table.clone(),
                finding: match right.get(table) {
                    None => Finding::Missing {
                        source: source_rows,
                    },
                    Some(&0) if source_rows > 0 => Finding::Empty {
                        source: source_rows,
                    },
                    Some(&destination_rows) if destination_rows == source_rows => {
                        Finding::Matched { rows: source_rows }
                    }
                    Some(&destination_rows) => Finding::Drifted {
                        source: source_rows,
                        destination: destination_rows,
                    },
                },
            })
            .collect();

        rows.extend(
            right
                .iter()
                .filter(|(table, _)| !left.contains_key(*table))
                .map(|(table, &rows)| Row {
                    table: table.clone(),
                    finding: Finding::OnlyInDestination { rows },
                }),
        );
        rows.sort_by(|one, two| one.table.cmp(&two.table));

        Self {
            mode,
            rows,
            matched_by_name_only: bare
                && source.schema_is_the_database() != destination.schema_is_the_database(),
        }
    }

    /// Every table that failed.
    pub fn failures(&self) -> impl Iterator<Item = &Row> {
        self.rows
            .iter()
            .filter(|row| row.finding.is_failure(self.mode))
    }

    /// Every table that differs without failing.
    pub fn drifted(&self) -> impl Iterator<Item = &Row> {
        self.rows
            .iter()
            .filter(|row| matches!(row.finding, Finding::Drifted { .. }))
    }

    /// Did the copy land?
    #[must_use]
    pub fn landed(&self) -> bool {
        self.failures().next().is_none()
    }

    /// The same findings, as a `--json` run reports them.
    ///
    /// **One object per table, with the finding as a word.** A script that wants to know
    /// whether a copy landed reads `ok` on the envelope; one that wants to know *which* table
    /// drifted reads this, and a word it can match on is worth more than a sentence it has to
    /// parse.
    #[must_use]
    pub fn as_json(&self) -> Vec<serde_json::Value> {
        self.rows
            .iter()
            .map(|row| {
                let (finding, source, destination) = match row.finding {
                    Finding::Matched { rows } => ("matched", Some(rows), Some(rows)),
                    Finding::Drifted {
                        source,
                        destination,
                    } => ("drifted", Some(source), Some(destination)),
                    Finding::Empty { source } => ("empty", Some(source), Some(0)),
                    Finding::Missing { source } => ("missing", Some(source), None),
                    Finding::OnlyInDestination { rows } => {
                        ("only_in_destination", None, Some(rows))
                    }
                };
                serde_json::json!({
                    "table": row.table,
                    "finding": finding,
                    "source": source,
                    "destination": destination,
                    "counted": self.mode == Mode::Exact,
                })
            })
            .collect()
    }

    /// What the process exits with.
    ///
    /// `6` and not `1`: it finished, and then the counts disagreed. A scheduler reading
    /// `6` knows the work ran and the result is suspect, which is a different thing to
    /// retry than a server that was down.
    #[must_use]
    pub fn exit(&self) -> Exit {
        if self.landed() {
            Exit::Success
        } else {
            Exit::Mismatch
        }
    }

    /// The report, line by line.
    ///
    /// Built rather than printed, so that what somebody reads is something a test can
    /// assert on — including the words "drift" and "nothing arrived", which are the whole
    /// distinction this module exists to draw.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        let mut lines = vec![format!("verifying — {}", self.mode.describe())];

        if self.matched_by_name_only {
            lines.push(
                "the two engines do not name schemas the same way, so tables are matched \
                 by name"
                    .to_owned(),
            );
        }

        // Three columns measured against every row before any of them is written, so the
        // arrows line up and a long list can be read down rather than across. A column of
        // numbers that does not align is a column nobody scans.
        let counted: Vec<(&Row, String, String)> = self
            .rows
            .iter()
            .map(|row| {
                let (source, destination) = match row.finding {
                    Finding::Matched { rows } => (rows.to_string(), rows.to_string()),
                    Finding::Drifted {
                        source,
                        destination,
                    } => (source.to_string(), destination.to_string()),
                    Finding::Empty { source } => (source.to_string(), "0".to_owned()),
                    Finding::Missing { source } => (source.to_string(), "—".to_owned()),
                    Finding::OnlyInDestination { rows } => ("—".to_owned(), rows.to_string()),
                };
                (row, source, destination)
            })
            .collect();

        let widest =
            |of: fn(&(&Row, String, String)) -> usize| counted.iter().map(of).max().unwrap_or(0);
        let table_column = widest(|(row, _, _)| row.table.chars().count());
        let source_column = widest(|(_, source, _)| source.chars().count());
        let destination_column = widest(|(_, _, destination)| destination.chars().count());

        let pad = |text: &str, to: usize| " ".repeat(to.saturating_sub(text.chars().count()));

        for (row, source, destination) in &counted {
            let explanation = row.finding.explain(self.mode);
            let mut line = format!(
                "{}{}  {}{source} → {destination}{}",
                row.table,
                pad(&row.table, table_column),
                pad(source, source_column),
                pad(destination, destination_column),
            );
            if !explanation.is_empty() {
                line.push_str("   ");
                line.push_str(explanation);
            }
            // Nothing else is padded to the right of the last column.
            lines.push(line.trim_end().to_owned());
        }

        lines.push(self.summary());
        lines
    }

    /// The one line somebody reads when they do not read the rest.
    fn summary(&self) -> String {
        use std::fmt::Write as _;

        let matched = self
            .rows
            .iter()
            .filter(|row| matches!(row.finding, Finding::Matched { .. }))
            .count();
        let drifted = self.drifted().count();
        let failed = self.failures().count();

        let mut summary = format!("{matched} of {} tables matched", self.rows.len());
        // Writing to a String cannot fail, and the alternative is a `format!` per clause.
        if drifted > 0 {
            let _ = write!(summary, ", {drifted} drifted");
        }
        if failed > 0 {
            let _ = write!(summary, ", {failed} did not arrive");
        }
        if self.mode.is_estimate() {
            summary.push_str(" — from estimates, so this is a glance and not a proof");
        }
        summary
    }
}

/// Count two databases and compare them, in whichever mode the environment asked for.
///
/// **The one function a command calls**, and the only place `SLOOP_VERIFY` is read. A command
/// that built its own [`Comparison`] could quietly ignore the variable and nothing would
/// notice until somebody set it and got the slow answer anyway; going through here means
/// honouring it is the default and bypassing it is the thing that takes effort.
pub fn against(
    adapter: &dyn Adapter,
    source: &Target<'_>,
    destination: &Target<'_>,
) -> Outcome<Comparison> {
    let mode = Mode::from_environment();
    // **The slowest honest thing this tool does.** An exact `count(*)` over every table on
    // both sides of a copy is minutes on a large database, and until now it happened in
    // silence. Rule 7 of the owner's list.
    let counting = crate::console::step("Counting rows on both sides", "Verified");
    let counted = Side::counted(adapter, source, mode)?;
    let landed = Side::counted(adapter, destination, mode)?;
    let comparison = Comparison::of(mode, &counted, &landed);
    settled(counting, &comparison);
    Ok(comparison)
}

/// Settle the counting step with what the comparison found.
///
/// **Three outcomes, three marks, and the middle one is the interesting one.** A copy whose
/// counts disagree is a failure; one where they agree exactly is a tick; one where the source
/// moved underneath the copy worked *and* there is something to know about it, which is the
/// amber the owner asked for — see "A result that worked, with something to know" in
/// `docs/OWNER-DECISIONS.md`.
pub fn settled(counting: crate::console::Step, comparison: &Comparison) {
    let drifted = comparison.drifted().count();
    if !comparison.landed() {
        counting.bad("the counts do not agree");
    } else if drifted > 0 {
        counting.warn(&format!(
            "{drifted} table{} moved — the source was live",
            if drifted == 1 { "" } else { "s" }
        ));
    } else {
        counting.ok(comparison.mode.describe());
    }
}

/// Print a comparison, accent on the headline and the detail dimmed under it.
pub fn print(comparison: &Comparison) {
    let mut lines = comparison.describe().into_iter();
    if let Some(heading) = lines.next() {
        crate::say!("  {}", style::dim(&heading));
    }
    for line in lines {
        crate::say!("    {}", style::dim(&line));
    }
}
