//! `sloop backups list` and `sloop backups prune` — what has been taken, and what goes.
//!
//! **Both commands read both stores.** A project's backups live beside the project and the
//! global store's live in the config directory, and somebody asking what they have does not
//! want to be told about only half of it. The search order comes from `R2`'s resolution, so
//! `--global` narrows this exactly as it narrows everything else.
//!
//! **Local time, always.** The directory names are UTC because they have to sort and
//! survive a clock going back an hour; every line printed here is the local clock, which is
//! what somebody remembers about when they took a backup.
//!
//! **Nothing is deleted without being shown first.** `prune` builds the whole plan, prints
//! it, and then asks — and with no terminal to ask at it exits `2` naming `--yes`, because
//! rule 4 outranks the convenience of a scheduled prune that guesses.

#[cfg(test)]
#[path = "backups_tests.rs"]
mod tests;

use std::path::Path;
use std::time::Duration;

use crate::backup::store::{self, Check, Found, Held, Plan, Policy, State, Stored};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::{Registries, Scope};
use crate::style;

use super::backup::{describe_bytes, plural};
use super::confirmed;

/// Everything these two commands need from the outside.
pub struct Context<'a> {
    /// Both registries, already open — read for their roots and their search order.
    pub registries: Registries,
    /// The global store, for saying where a listing came from.
    pub global: &'a Path,
}

/// Everything `prune` was asked for, named rather than positional.
///
/// Six arguments in a row is six chances to pass `--dry-run` where `--yes` goes and delete
/// somebody's backups while telling them nothing happened.
pub struct Pruning<'a> {
    /// Only this database's backups, or all of them.
    pub name: Option<&'a str>,
    /// Keep this many of the newest, per database.
    pub keep: Option<usize>,
    /// Remove anything older than this, as typed — `30d`.
    pub older_than: Option<&'a str>,
    /// Show the plan and stop.
    pub dry_run: bool,
    /// Remove unfinished and damaged directories too.
    pub include_broken: bool,
    /// The answer, given in advance.
    pub yes: bool,
}

/// One store's worth of backups, and which store it was.
struct Store {
    /// Which registry it hangs off.
    scope: Scope,
    /// What was in it.
    found: Found,
}

/// Show what has been taken.
pub fn list(context: &Context<'_>, name: Option<&str>, check: bool) -> Outcome<Exit> {
    let stores = read(context, if check { Check::Checksum } else { Check::Size })?;
    let wanted = name.map(store::label_for);

    let mut tally = Tally::default();

    for store in &stores {
        let backups: Vec<&Stored> = store
            .found
            .backups
            .iter()
            .filter(|stored| wanted.as_ref().is_none_or(|label| *label == stored.label))
            .collect();

        if backups.is_empty() && store.found.strays.is_empty() {
            continue;
        }

        if stores.len() > 1 {
            anstream::println!("{}", style::dim(store.scope.label()));
        }

        for group in grouped(&backups) {
            print_group(&group);
            tally.add(&group);
        }

        for stray in &store.found.strays {
            anstream::println!(
                "  {}",
                style::dim(&format!(
                    "{} is not a backup sloop wrote — left alone",
                    stray.display()
                ))
            );
        }
    }

    if tally.directories == 0 {
        return Ok(nothing_to_show(context, &stores, name));
    }

    anstream::println!();
    anstream::println!("{}", style::dim(&tally.summary(check)));

    if tally.damaged > 0 {
        // Exit 6 is "it finished and then the numbers disagreed", which is exactly this: a
        // manifest and a dump that contradict each other. Reusing it rather than inventing
        // a code keeps rule 7 intact, and a listing that exits 0 over a corrupt backup
        // would be the mistake `doctor` was given exit 8 to avoid.
        return Ok(Exit::Mismatch);
    }

    Ok(Exit::Success)
}

/// Delete what a retention rule says to.
pub fn prune(context: &Context<'_>, asked: &Pruning<'_>) -> Outcome<Exit> {
    let policy = policy_from(asked)?;
    let stores = read(context, Check::Size)?;
    let wanted = asked.name.map(store::label_for);

    let mut plans: Vec<(Scope, Plan)> = Vec::new();
    for store in stores {
        let backups: Vec<Stored> = store
            .found
            .backups
            .into_iter()
            .filter(|stored| wanted.as_ref().is_none_or(|label| *label == stored.label))
            .collect();

        if backups.is_empty() {
            continue;
        }

        plans.push((
            store.scope,
            store::plan(backups, crate::backup::stamp::Stamp::now(), &policy),
        ));
    }

    let (removing, total) = preview(&plans);

    if removing == 0 {
        anstream::println!(
            "{}",
            style::dim("nothing to prune — the policy removes none of what is there")
        );
        return Ok(Exit::Success);
    }

    anstream::println!();
    anstream::println!(
        "{}",
        style::dim(&format!(
            "{} would go, freeing {}",
            plural(count(removing), "backup"),
            describe_bytes(total)
        ))
    );

    if asked.dry_run {
        anstream::println!("{}", style::dim("--dry-run: nothing was removed."));
        return Ok(Exit::Success);
    }

    if !confirmed(asked.yes, "Remove them?", "--yes")? {
        anstream::println!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    }

    let mut removed = 0_usize;
    let mut freed = 0_u64;
    let mut failed = 0_usize;

    for (_, plan) in &plans {
        for stored in &plan.remove {
            // A size read before the directory goes, because afterwards there is nothing
            // left to measure.
            let size = stored.bytes();
            match store::remove(stored) {
                Ok(()) => {
                    removed += 1;
                    freed += size;
                }
                Err(failure) => {
                    // One directory that will not go is not a reason to stop: the rest of
                    // the retention still has to be applied, exactly as `backup --all`
                    // keeps going and reports what failed.
                    failed += 1;
                    anstream::eprintln!("{}", style::dim(failure.message()));
                }
            }
        }
    }

    anstream::println!(
        "{}",
        style::dim(&format!(
            "removed {}, freed {}",
            plural(count(removed), "backup"),
            describe_bytes(freed)
        ))
    );

    if failed > 0 {
        return Err(Failure::new(
            Exit::Failure,
            format!("{} could not be removed", plural(count(failed), "backup")),
        )
        .hint("the paths are above — something else may have them open"));
    }

    Ok(Exit::Success)
}

/// Turn the flags into a policy, or refuse.
fn policy_from(asked: &Pruning<'_>) -> Outcome<Policy> {
    let older_than: Option<Duration> = asked.older_than.map(store::parse_age).transpose()?;

    if asked.keep.is_none() && older_than.is_none() {
        // **A prune with no rule would delete everything.** Refusing is the only safe
        // reading of an empty policy, and rule 4's shape applies: name the flags.
        return Err(
            Failure::usage("prune needs a rule, or it would have nothing to keep").hint(
                "`--keep 7`, `--older-than 30d`, or both — `sloop backups prune --help` has \
                 what both together mean",
            ),
        );
    }

    Ok(Policy {
        keep: asked.keep,
        older_than,
        include_broken: asked.include_broken,
    })
}

/// Read every store the resolution makes relevant.
fn read(context: &Context<'_>, check: Check) -> Outcome<Vec<Store>> {
    let mut stores = Vec::new();

    for scope in context.registries.resolution().search_order() {
        let Some(root) = context.registries.root_in(*scope) else {
            continue;
        };
        stores.push(Store {
            scope: *scope,
            found: store::scan(&root, check)?,
        });
    }

    Ok(stores)
}

/// Split a newest-first listing into label blocks, each still newest-first.
///
/// The groups come out in the order their newest backup did, so the database backed up most
/// recently is at the top — which is the question somebody scanning this is usually asking.
fn grouped<'a>(backups: &[&'a Stored]) -> Vec<Vec<&'a Stored>> {
    let mut groups: Vec<Vec<&'a Stored>> = Vec::new();

    for stored in backups {
        match groups.iter_mut().find(|group| {
            group
                .first()
                .is_some_and(|first| first.group() == stored.group())
        }) {
            Some(group) => group.push(stored),
            None => groups.push(vec![stored]),
        }
    }

    groups
}

/// Print one label's backups under one heading.
fn print_group(group: &[&Stored]) {
    let Some(first) = group.first() else {
        return;
    };

    let bytes: u64 = group.iter().map(|stored| stored.bytes()).sum();
    anstream::println!(
        "{}  {}",
        style::paint(&first.label),
        style::dim(&format!(
            "{} · {} · {}",
            first.engine,
            plural(count(group.len()), "backup"),
            describe_bytes(bytes)
        ))
    );

    for stored in group {
        anstream::println!("  {}", line_for(stored));
    }
}

/// When a backup was taken, on the clock of the machine that took it.
///
/// **The manifest's offset when there is one**, because that is the offset where the backup
/// happened and it travels with the file; this machine's current one otherwise, which is all
/// a directory with no manifest has left to say.
fn when_of(stored: &Stored) -> String {
    stored.manifest.as_ref().map_or_else(
        || stored.taken.local().readable(),
        |manifest| manifest.taken_locally().readable(),
    )
}

/// One backup, as one line.
fn line_for(stored: &Stored) -> String {
    let when = when_of(stored);

    let detail = match (&stored.state, &stored.manifest) {
        (State::Complete, Some(manifest)) => format!(
            "{}{} · {}",
            describe_bytes(manifest.dump.bytes),
            if manifest.dump.encryption.is_some() {
                ", encrypted"
            } else {
                ""
            },
            plural(manifest.rows, "row"),
        ),
        _ => stored
            .problem()
            .unwrap_or("nothing readable in this directory")
            .to_owned(),
    };

    format!("{when}  {}", style::dim(&detail))
}

/// What to say when there is nothing to list.
fn nothing_to_show(context: &Context<'_>, stores: &[Store], name: Option<&str>) -> Exit {
    let anywhere: Vec<&Stored> = stores
        .iter()
        .flat_map(|store| store.found.backups.iter())
        .collect();

    if let Some(name) = name {
        anstream::println!(
            "{}",
            style::dim(&format!("nothing has been backed up under {name}."))
        );
        if !anywhere.is_empty() {
            let mut labels: Vec<&str> = anywhere
                .iter()
                .map(|stored| stored.label.as_str())
                .collect();
            labels.sort_unstable();
            labels.dedup();
            anstream::println!(
                "{}",
                style::dim(&format!("there are backups under: {}", labels.join(", ")))
            );
        }
        return Exit::Success;
    }

    anstream::println!(
        "{}",
        style::dim(&format!(
            "nothing has been backed up into {} yet.",
            context.registries.resolution().describe(context.global)
        ))
    );
    anstream::println!(
        "{}",
        style::dim("`sloop backup <name>` takes one — `sloop db list` shows the names.")
    );

    Exit::Success
}

/// A count as the width [`plural`] takes.
///
/// `as` would be a lint and a lie on a 32-bit build; a number of backups that does not fit
/// in a `u64` is not a case this tool has.
fn count(of: usize) -> u64 {
    u64::try_from(of).unwrap_or(u64::MAX)
}

/// Print what a prune would do, and hand back how much of it there is.
///
/// **This is the "previews before it deletes" half of `R12`**, and it is the whole of the
/// output either way: `--dry-run` stops after it, and a real prune asks its question
/// underneath it. What comes back is the count and the bytes, so the summary line and the
/// confirmation are talking about the same numbers that were just shown.
fn preview(plans: &[(Scope, Plan)]) -> (usize, u64) {
    let several = plans.len() > 1;
    let mut removing = 0_usize;
    let mut total = 0_u64;

    for (scope, plan) in plans {
        if several {
            anstream::println!("{}", style::dim(scope.label()));
        }

        for stored in &plan.remove {
            anstream::println!(
                "  {} {}  {}",
                style::paint("remove"),
                line_for(stored),
                style::dim(&stored.taken.utc_path())
            );
        }
        removing += plan.remove.len();
        total += plan.bytes();

        // The ones worth naming rather than counting: something is wrong with them, or
        // something may still be writing them. A half-written dump is reported, never
        // silently passed over.
        for (stored, held) in plan
            .held
            .iter()
            .filter(|(_, held)| matches!(held, Held::Broken | Held::MaybeRunning))
        {
            // The held reason and nothing else. `line_for` would print what is wrong with
            // the directory as well, and two overlapping explanations of the same thing is
            // how a preview stops being read.
            anstream::println!(
                "  {} {}  {}",
                style::dim("  keep"),
                when_of(stored),
                style::dim(held.why())
            );
        }

        let ordinary = plan
            .held
            .iter()
            .filter(|(_, held)| matches!(held, Held::Newest | Held::Young))
            .count();
        if ordinary > 0 {
            anstream::println!(
                "  {}",
                style::dim(&format!(
                    "{} kept by the policy",
                    plural(count(ordinary), "backup")
                ))
            );
        }
    }

    (removing, total)
}

/// The counts under a listing.
///
/// **Complete backups and everything else are counted apart, on purpose.** A footer reading
/// "4 backups" under a list where one of them is a half-written directory would be the
/// silent counting `R12` exists to stop — so the number is what a restore could use, and
/// the rest are named as what they are.
#[derive(Debug, Default)]
struct Tally {
    /// Directories shown, whatever state they were in.
    directories: usize,
    /// Backups a restore could use.
    complete: usize,
    /// Manifest and dump disagree — the ones that set exit `6`.
    damaged: usize,
    /// No manifest, or one nothing can read.
    broken: usize,
    /// The size of the complete ones.
    bytes: u64,
}

impl Tally {
    /// Count a printed group.
    fn add(&mut self, group: &[&Stored]) {
        for stored in group {
            self.directories += 1;
            if stored.is_complete() {
                self.complete += 1;
                self.bytes += stored.bytes();
            } else if stored.is_damaged() {
                self.damaged += 1;
            } else {
                self.broken += 1;
            }
        }
    }

    /// The line under the list.
    fn summary(&self, check: bool) -> String {
        // Writing into a `String` cannot fail, and the alternative is an allocation per
        // clause for a line that is mostly one clause long.
        use std::fmt::Write as _;

        let mut summary = format!(
            "{}, {}",
            plural(count(self.complete), "backup"),
            describe_bytes(self.bytes)
        );

        // clause for a line that is mostly one clause long.
        if self.damaged > 0 {
            let _ = write!(summary, " · {} damaged", self.damaged);
        }
        if self.broken > 0 {
            let _ = write!(summary, " · {} unfinished", self.broken);
        }

        summary.push_str(if check {
            " · every dump hashed"
        } else {
            " · sizes checked, --check hashes them"
        });

        summary
    }
}
