//! `sloop sync` — merge one database into another, keeping what only the destination has.
//!
//! **This is not `mirror`, and the difference is the whole command.** A mirror makes the
//! destination identical: anything that existed only there is gone. A sync *merges* — rows
//! the source has are added, rows both have are replaced with the source's, and rows only
//! the destination has are **kept**. That is what makes it the command for a staging
//! database somebody is working in, and `mirror` the command for one nobody is.
//!
//! **Order is not optional.** A row cannot point at a parent that is not there yet, so the
//! tables are sorted by their foreign keys before anything moves — parents first, in one
//! topological pass over the whole set. Two tables that point at each other cannot be sorted
//! at all, and that is **refused**, naming the tables in the cycle: half the rows would load
//! and half would not, which is worse than not starting.
//!
//! **A table with no primary key is skipped, and said out loud.** Without one there is no
//! definition of "the same row", so there is nothing a merge could replace — inserting
//! everything again would double the table on the second run. Skipping it is the only honest
//! answer, and doing it silently would be the dishonest one.
//!
//! **Rows the source no longer has are notified, not deleted.** Keeping them is the point of
//! the command, so the number is reported per table rather than acted on: a row that was
//! deleted in the source and a row that only ever existed in the destination look exactly
//! the same from here, and deleting both because they might be the first is not a trade this
//! command gets to make on somebody's behalf.
//!
//! **Sequences are reset at the end.** A merge inserts explicit keys and a sequence does not
//! notice — copy a table whose `id` runs to 500 into a destination whose sequence sits at 7
//! and the next insert *there* collides with row 7. That failure surfaces later, in somebody
//! else's application, with nothing pointing back here.
//!
//! **Nothing is left behind**, on the same terms as `R14`: the rows go from one client to the
//! other through a staging table that belongs to the session, and `--safe` is the opt-in net
//! — a dump taken first, deleted the moment the counts agree, kept with its path printed when
//! anything fails.

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::consent::{Consent, Destroying};
use crate::engine::{Adapter, Merged, Table, TableShape, Target};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::Database;
use crate::registry::{Registries, Scope};
use crate::style;

use super::backup::{describe_bytes, plural};
use super::mirror::{self, Destination, New};

/// Everything `sync` needs from the outside.
pub struct Context<'a> {
    /// Both registries, already open.
    pub registries: Registries,
    /// The global store, which is where sloop's own copy of the client tools lives.
    pub global: &'a Path,
    /// `--password-command`, which outranks whatever route a record names.
    pub password_command: Option<&'a str>,
    /// What this run was given permission to do — see [`crate::consent`].
    pub consent: Consent<'a>,
}

/// Everything `sync` was asked for, named rather than positional.
pub struct Syncing<'a> {
    /// The registered database to merge from. Only ever read.
    pub source: &'a str,
    /// `--to`: a destination that is already registered.
    pub to: Option<&'a str>,
    /// `--create`: a destination that is not there yet, and what to call it here.
    pub create: Option<&'a str>,
    /// `--safe`: dump the source first, so a merge that fails can be replayed.
    pub safe: bool,
    /// The flags that only mean something when one is being made.
    pub new: New<'a>,
}

/// Merge `source` into a destination — making the destination first, if asked to.
pub fn run(context: &mut Context<'_>, asked: &Syncing<'_>) -> Outcome<Exit> {
    let (from_scope, from) = context.registries.find(asked.source)?;
    let (from_scope, from) = (from_scope, from.clone());

    let Some(destination) = mirror::decide(
        &context.registries,
        context.consent,
        asked.source,
        asked.to,
        asked.create,
    )?
    else {
        anstream::println!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    };

    match destination {
        Destination::Registered(name) => {
            into_a_registered_database(context, asked, from_scope, &from, &name)
        }
        Destination::Made(name) => into_a_new_database(context, asked, from_scope, &from, &name),
    }
}

/// `--to`: the destination is already there, and its rows are merged with the source's.
fn into_a_registered_database(
    context: &Context<'_>,
    asked: &Syncing<'_>,
    from_scope: Scope,
    from: &Database,
    destination: &str,
) -> Outcome<Exit> {
    let (into_scope, into) = context.registries.find(destination)?;
    let into = into.clone();

    mirror::refuse_the_same_connection(
        asked.source,
        from,
        destination,
        &into.host,
        into.port,
        &into.database,
        SYNCING_ITSELF,
    )?;
    refuse_across_engines(asked.source, from, destination, &into)?;

    // **Permission first, before a password is fetched or a socket opened.** A sync changes
    // rows in a database somebody is using, so rule 5 applies exactly as it does to a
    // mirror — the name gets typed. What it does *not* do is drop anything, and the words
    // say so.
    let destroying = Destroying {
        named: &into.database,
        noun: "database",
        action: "replacing rows in a database",
    };
    context.consent.checked_early(&destroying)?;

    let secret = |scope, record: &_| {
        mirror::secret_for(&context.registries, context.password_command, scope, record)
    };
    let reading = secret(from_scope, from)?;
    let writing = secret(into_scope, &into)?;

    merge(
        context,
        asked,
        &from.target(&reading),
        &into.target(&writing),
        Some(&destroying),
    )
}

/// `--create`: the destination is made first, and then merged into.
///
/// **A merge into a database that did not exist a moment ago is a copy**, and the numbers say
/// so — every row is new and nothing is kept. That is not a special case in the code and does
/// not need to be: what makes `sync` different from `mirror` is what it does to rows that were
/// already there, and there are none.
fn into_a_new_database(
    context: &mut Context<'_>,
    asked: &Syncing<'_>,
    from_scope: Scope,
    from: &Database,
    label: &str,
) -> Outcome<Exit> {
    let mut reading = None;
    let built = mirror::make_the_destination(
        &mut context.registries,
        context.global,
        context.consent,
        &mirror::Making {
            source: asked.source,
            from,
            label,
            new: &asked.new,
            consequence: SYNCING_ITSELF,
        },
        |registries| {
            let secret =
                mirror::secret_for(registries, context.password_command, from_scope, from)?;
            super::adapter_for(from.engine, context.global).probe(&from.target(&secret))?;
            reading = Some(secret);
            Ok(())
        },
    )?;
    let reading = reading.expect("the source is proved before anything is created");

    // **No `Destroying`.** There is nothing in there to replace: this run made it. The shape
    // it does not have yet is `merge`'s business, and this is not the only way to arrive at
    // a destination with nothing in it — see `Shaped`.
    merge(
        context,
        asked,
        &from.target(&reading),
        &built.record.target(&built.secret),
        None,
    )
}

/// Two engines is `R29`'s question, and until then it is a refusal that says where.
fn refuse_across_engines(
    source: &str,
    from: &Database,
    destination: &str,
    into: &Database,
) -> Outcome<()> {
    if from.engine == into.engine {
        return Ok(());
    }

    Err(Failure::usage(format!(
        "{source} is {} and {destination} is {}",
        from.engine, into.engine
    ))
    .hint("R29 is where copying across engines gets decided; today it is refused"))
}

/// What syncing a database into itself would have done, which is the half worth reading.
const SYNCING_ITSELF: &str =
    "a sync would merge the source into itself, which can only waste the time it takes";

/// The merge itself, once both ends are settled.
fn merge(
    context: &Context<'_>,
    asked: &Syncing<'_>,
    source_target: &Target<'_>,
    destination_target: &Target<'_>,
    destroying: Option<&Destroying<'_>>,
) -> Outcome<Exit> {
    let adapter = super::adapter_for(source_target.engine, context.global);

    anstream::println!(
        "{} {}",
        style::paint("syncing"),
        style::dim(&format!(
            "{} → {}",
            source_target.describe(),
            destination_target.describe()
        ))
    );
    let from_server = adapter.probe(source_target)?;
    let into_server = adapter.probe(destination_target)?;
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "{} {} → {} {}",
            from_server.engine, from_server.version, into_server.engine, into_server.version
        ))
    );

    // **The plan is made and printed before anything is asked for.** What will be merged,
    // in what order, and what will not be — so the question somebody answers is a question
    // about work they have already seen described.
    let plan = Plan::of(adapter.as_ref(), source_target)?;
    plan.announce();

    if plan.merging.is_empty() {
        anstream::println!("{}", style::dim("nothing to merge."));
        return Ok(Exit::Success);
    }

    // **What the destination has now decides what this run is.** Three cases, and they are
    // the ones the owner named: a database with nothing in it gets the source's shape and
    // then every row, which is a copy; one with the same tables gets a merge; one with
    // *some* of them is a database this cannot honestly merge into, and is refused.
    let waiting = Shaped::of(adapter.as_ref(), destination_target, &plan)?;
    waiting.announce();

    if let Some(destroying) = destroying {
        if !context.consent.typed(destroying)?.granted() {
            anstream::println!("{}", style::dim("left alone."));
            return Ok(Exit::Success);
        }
    }

    // The shape goes across before the rows: all of it into a destination that holds
    // nothing, and only the tables it is short of into one that holds some — see `Shaped`.
    if !waiting.missing.is_empty() {
        adapter.copy_schema_into(
            source_target,
            destination_target,
            if waiting.is_bare() {
                &[]
            } else {
                &waiting.missing
            },
        )?;
    }

    carry_out(
        adapter.as_ref(),
        source_target,
        destination_target,
        &plan,
        asked.safe,
    )
}

/// Do the merging, once the plan is settled and permission has been given.
///
/// **Its own function because `run` above is the decisions and this is the work.** Everything
/// before this point can still change its mind; from here the destination is being written
/// to, and the only questions left are what to print and what to exit with.
fn carry_out(
    adapter: &dyn Adapter,
    source: &Target<'_>,
    destination: &Target<'_>,
    plan: &Plan,
    safe: bool,
) -> Outcome<Exit> {
    // `--safe`: a dump of the source taken before anything is written, so a merge that dies
    // halfway can be replayed into the destination by hand. Deleted once the counts agree.
    let kept = if safe {
        Some(net(adapter, source)?)
    } else {
        None
    };

    let started = std::time::Instant::now();
    let merged = match merge_each(adapter, source, destination, plan, kept.as_deref()) {
        Ok(merged) => merged,
        Err(failure) => {
            if let Some(dump) = &kept {
                anstream::eprintln!(
                    "{}",
                    style::dim(&format!(
                        "--safe: the dump is kept at {} — the merge did not finish",
                        dump.display()
                    ))
                );
            }
            return Err(failure);
        }
    };

    anstream::println!(
        "  {}",
        style::dim(&format!(
            "merged in {:.1}s",
            started.elapsed().as_secs_f64()
        ))
    );

    // Sequences last, and only once every row is in: a counter set from a table that is
    // still being filled is a counter that is already wrong.
    for moved in adapter.reset_sequences(destination)? {
        anstream::println!("  {}", style::dim(&moved));
    }

    let exit = report(&merged);
    if let Some(dump) = kept {
        if exit == Exit::Success {
            discard(&dump);
        } else {
            anstream::eprintln!(
                "{}",
                style::dim(&format!(
                    "--safe: the dump is kept at {} — the counts did not agree",
                    dump.display()
                ))
            );
        }
    }

    Ok(exit)
}

/// What a sync is going to do, worked out before it does any of it.
struct Plan {
    /// The tables to merge, parents before children.
    merging: Vec<TableShape>,
    /// The tables with no primary key, which cannot be merged.
    skipping: Vec<Table>,
}

impl Plan {
    /// Read the source's shape and put its tables in an order that can actually be loaded.
    fn of(adapter: &dyn Adapter, source: &Target<'_>) -> Outcome<Self> {
        let shapes = adapter.shapes(source)?;
        let (mergeable, skipping): (Vec<TableShape>, Vec<TableShape>) = shapes
            .into_iter()
            .partition(|shape| !shape.primary_key.is_empty());

        Ok(Self {
            merging: in_dependency_order(mergeable)?,
            skipping: skipping.into_iter().map(|shape| shape.table).collect(),
        })
    }

    /// Say what is about to happen, and what is not.
    fn announce(&self) {
        anstream::println!(
            "  {}",
            style::dim(&format!(
                "merging {}, parents first",
                plural(count(self.merging.len()), "table")
            ))
        );

        // **Named, not counted.** A table that is being skipped is a table whose rows are
        // not going to arrive, and somebody reading this needs to know which one rather
        // than how many.
        for table in &self.skipping {
            anstream::println!(
                "  {}",
                style::dim(&format!(
                    "skipping {table} — it has no primary key, so there is no way to tell \
                     which row is which"
                ))
            );
        }
    }
}

/// What the destination already has, which is what decides whether this run is a merge.
///
/// **The three cases the owner named, and they are not the same run.** *"if db is not present
/// it will create first, and just like mirror it will do; if already present and no data,
/// still same thing like mirror; if data is present then merge."*
///
/// - **Nothing at all** — no tables, whether because `--create` made the database a moment
///   ago or because somebody registered an empty one. The source's shape goes across and then
///   every row, which is a copy: the numbers will say every row is new and nothing was kept.
/// - **The same tables** — a merge, which is what this command is for.
/// - **Some of them** — the ones it is short of are created first, and then the whole set is
///   merged. Only those tables' shapes are carried: copying the source's whole schema over a
///   destination that already holds half of it would fail on the first table that is there.
///   Each one is named before the question, because creating a table is a change to the
///   destination's shape rather than to its rows.
struct Shaped {
    /// The tables the destination is missing that the plan means to merge.
    missing: Vec<Table>,
    /// How many tables the destination has, of any kind.
    has: usize,
}

impl Shaped {
    /// Ask the destination what it holds, and line it up against the plan.
    fn of(adapter: &dyn Adapter, destination: &Target<'_>, plan: &Plan) -> Outcome<Self> {
        let there = adapter.tables(destination)?;
        Ok(Self {
            missing: plan
                .merging
                .iter()
                .map(|shape| &shape.table)
                .filter(|table| !there.contains(table))
                .cloned()
                .collect(),
            has: there.len(),
        })
    }

    /// Is there nothing in there at all?
    const fn is_bare(&self) -> bool {
        self.has == 0
    }

    /// Say which of the three this run is, before anybody agrees to it.
    fn announce(&self) {
        if self.is_bare() {
            anstream::println!(
                "  {}",
                style::dim(
                    "the destination has no tables, so its shape is copied across first and \
                     every row will be new"
                )
            );
            return;
        }

        anstream::println!(
            "  {}",
            style::dim(&format!(
                "the destination has {}",
                plural(count(self.has), "table")
            ))
        );

        // **Named, not counted.** A table the destination is short of is one that is about to
        // be created there, and creating a table changes the destination's shape rather than
        // its rows — so it is said before the question rather than reported after it.
        for table in &self.missing {
            anstream::println!(
                "  {}",
                style::dim(&format!("{table} is not there yet, so it is created first"))
            );
        }
    }
}

/// Sort the tables so that every table comes after the ones it points at.
///
/// **Kahn's algorithm, and the leftovers are the cycle.** When nothing is left with all its
/// parents already placed and tables remain, those tables are exactly the ones caught in a
/// loop — so the refusal can name them rather than saying "there is a cycle somewhere".
///
/// A reference to a table that is not in the set at all is ignored rather than refused: a
/// foreign key pointing at a table sloop is not copying cannot constrain the order of the
/// ones it is.
fn in_dependency_order(mut shapes: Vec<TableShape>) -> Outcome<Vec<TableShape>> {
    let present: BTreeSet<Table> = shapes.iter().map(|shape| shape.table.clone()).collect();
    let mut waiting_for: BTreeMap<Table, BTreeSet<Table>> = shapes
        .iter()
        .map(|shape| {
            // A table that points at itself is filtered here as well as in the adapters'
            // own queries. An employee with a manager is ordinary, the rows inside one
            // table arrive in one statement, and a sort that took it for a dependency
            // would report every such table as an unbreakable cycle.
            let parents = shape
                .references
                .iter()
                .filter(|parent| **parent != shape.table && present.contains(parent))
                .cloned()
                .collect();
            (shape.table.clone(), parents)
        })
        .collect();

    // Alphabetical among equals, so two runs against the same database do the same thing in
    // the same order — which is what makes the log of one comparable with the log of the next.
    let mut placed: Vec<Table> = Vec::with_capacity(shapes.len());
    while placed.len() < shapes.len() {
        let next = waiting_for
            .iter()
            .filter(|(_, parents)| parents.is_empty())
            .map(|(table, _)| table.clone())
            .min();

        let Some(next) = next else {
            let stuck: Vec<String> = waiting_for.keys().map(Table::to_string).collect();
            return Err(Failure::usage(format!(
                "these tables' foreign keys point at each other, so there is no order that \
                 loads them: {}",
                stuck.join(", ")
            ))
            .hint(
                "sync refuses rather than loading half of them. `sloop mirror` replaces the \
                 destination outright and has no such ordering to get right",
            ));
        };

        waiting_for.remove(&next);
        for parents in waiting_for.values_mut() {
            parents.remove(&next);
        }
        placed.push(next);
    }

    let order: BTreeMap<Table, usize> = placed
        .into_iter()
        .enumerate()
        .map(|(at, table)| (table, at))
        .collect();
    shapes.sort_by_key(|shape| order.get(&shape.table).copied().unwrap_or(usize::MAX));
    Ok(shapes)
}

/// Merge every table in the plan's order, reporting each as it lands.
fn merge_each(
    adapter: &dyn Adapter,
    source: &Target<'_>,
    destination: &Target<'_>,
    plan: &Plan,
    kept: Option<&Path>,
) -> Outcome<Vec<Merged>> {
    let mut merged = Vec::with_capacity(plan.merging.len());
    for shape in &plan.merging {
        // **Stop at the first table that fails, and say what has already landed.** Carrying
        // on would load children whose parents never arrived, and a half-merged database
        // that reports success is the failure this command must not have. `--safe` has the
        // dump either way.
        let one = adapter
            .merge_table(source, destination, shape)
            .inspect_err(|_| {
                if kept.is_none() && !merged.is_empty() {
                    anstream::eprintln!(
                        "{}",
                        style::dim(&format!(
                            "{} had already been merged and {} is where it stopped",
                            plural(count(merged.len()), "table"),
                            shape.table
                        ))
                    );
                }
            })?;

        anstream::println!("  {}", style::dim(&describe(&one)));
        merged.push(one);
    }
    Ok(merged)
}

/// One table's line: what arrived, what changed, and what stayed.
fn describe(merged: &Merged) -> String {
    use std::fmt::Write as _;

    let mut said = format!(
        "{}  {} in, {} new, {} replaced",
        merged.table, merged.loaded, merged.inserted, merged.updated
    );
    if merged.kept > 0 {
        // **The notification the scope asks for.** Said per table, where the number can be
        // acted on, and worded so it covers both readings: a row deleted in the source and
        // a row that only ever existed here are the same thing from in here.
        let _ = write!(
            said,
            ", {} kept that the source has no row for",
            merged.kept
        );
    }
    if !merged.adds_up() {
        let _ = write!(
            said,
            "  — counted {} before and {} after, which does not add up",
            merged.before, merged.after
        );
    }
    said
}

/// The totals, and the exit code they come to.
///
/// **Exit `6` when the arithmetic disagrees.** The merge inserts what was not there and
/// replaces what was, so the destination has to end with exactly the rows it started with
/// plus the ones that were new. A table that does not add up means something else was
/// writing to it while this ran — finished, but not proved, which is what `6` is for.
fn report(merged: &[Merged]) -> Exit {
    let total = |what: fn(&Merged) -> u64| merged.iter().map(what).sum::<u64>();
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "{} merged: {} new, {} replaced, {} kept",
            plural(count(merged.len()), "table"),
            total(|one| one.inserted),
            total(|one| one.updated),
            total(|one| one.kept),
        ))
    );

    let wrong: Vec<&Merged> = merged.iter().filter(|one| !one.adds_up()).collect();
    if wrong.is_empty() {
        return Exit::Success;
    }

    anstream::eprintln!(
        "{}",
        style::dim(&format!(
            "{} did not add up: {}",
            plural(count(wrong.len()), "table"),
            wrong
                .iter()
                .map(|one| one.table.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    );
    Exit::Mismatch
}

/// `--safe`: a dump of the source, taken before anything is written.
///
/// The cost is stated where somebody can act on it: for as long as the merge runs there is
/// an **unencrypted** dump of the source on this machine's disk. That is the trade the flag
/// is asking for, and it goes the moment the counts agree.
fn net(adapter: &dyn Adapter, source: &Target<'_>) -> Outcome<PathBuf> {
    let directory = std::env::temp_dir().join(format!(
        "sloop-sync-{}-{}",
        std::process::id(),
        crate::backup::stamp::Stamp::now().utc_path()
    ));
    std::fs::create_dir_all(&directory).map_err(|error| {
        Failure::new(
            Exit::Dump,
            format!("could not create {}: {error}", directory.display()),
        )
    })?;

    let dump = crate::backup::dump_file(&directory);
    anstream::eprintln!(
        "{}",
        style::dim(&format!(
            "--safe: dumping the source to {} first — it is not encrypted, and it goes when \
             the counts agree",
            dump.display()
        ))
    );

    match adapter.dump(source, &dump) {
        Ok(summary) => {
            anstream::println!(
                "  {}",
                style::dim(&format!("dumped {}", describe_bytes(summary.bytes)))
            );
            Ok(directory)
        }
        Err(failure) => {
            discard(&directory);
            Err(failure)
        }
    }
}

/// Remove a temporary dump, and say so if it will not go.
fn discard(directory: &Path) {
    if let Err(error) = std::fs::remove_dir_all(directory) {
        anstream::eprintln!(
            "{}",
            style::dim(&format!(
                "the temporary dump at {} could not be removed: {error}",
                directory.display()
            ))
        );
    }
}

/// A count as the width [`plural`] takes.
fn count(of: usize) -> u64 {
    u64::try_from(of).unwrap_or(u64::MAX)
}
