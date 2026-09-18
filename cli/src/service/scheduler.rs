//! Taking the backups nobody wrote a cron line for — `R27a`.
//!
//! **It runs the commands, it does not reimplement them.** `commands::backup::run` and
//! `commands::backups::prune`, exactly as the menu calls them and with the same arguments a
//! person would have typed. A second implementation of taking a backup is a second set of bugs
//! in the one thing this tool exists to get right, and it would drift the first time either was
//! touched.
//!
//! **Rule 4, at its sharpest.** A service has no terminal, so nothing here may ask anything.
//! `prune` deletes and therefore wants consent; the consent a schedule carries is the schedule
//! itself, which somebody set on a terminal with the retention in front of them — so it is
//! given `--yes` and never `--force`, because `--force` overrides a refusal that is there to
//! protect something and no schedule implies that.
//!
//! **A run that collides with a manual one exits `7` and stands aside.** That comes out of
//! `backup::run`'s own lock, not from anything here, and it is recorded as standing aside
//! rather than as a failure — the backup somebody was already taking *is* the backup.
//!
//! **One catch-up, not a hundred and sixty-eight.** The next run is measured from now, so a
//! machine that was off for a week owes one backup. Late is recorded separately, because a
//! schedule that flatters itself is a schedule nobody can use to find out it has stopped.

use std::path::Path;

use crate::commands;
use crate::consent::Consent;
use crate::exit::Exit;
use crate::registry::Registries;
use crate::ssh::tunnel::Tunnels;

use super::schedule::{self, Ran, Scheduled};
use crate::backup::stamp::Stamp;
use crate::registry::store::Store;

/// What one round of the scheduler did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kept {
    /// One line per run, already fit for a journal.
    pub said: Vec<String>,
}

/// Take whatever backups are owed.
///
/// **Never fails as a whole**, for the reason sampling does not: one database whose server is
/// down must not stop the others being backed up, which is the moment a backup tool is for.
pub fn round(
    store: &Store,
    registries: &Registries,
    tunnels: &Tunnels,
    global: &Path,
    now: Stamp,
) -> Kept {
    let mut kept = Kept::default();

    let due: Vec<Scheduled> = match schedule::all(store) {
        Ok(all) => all.into_iter().filter(|one| one.due(now)).collect(),
        Err(why) => {
            kept.said
                .push(format!("could not read the schedule: {}", why.message()));
            return kept;
        }
    };

    for scheduled in due {
        kept.said
            .push(one(store, registries, tunnels, global, &scheduled, now));
    }

    kept
}

/// One database: back it up, prune it, write down what happened.
fn one(
    store: &Store,
    registries: &Registries,
    tunnels: &Tunnels,
    global: &Path,
    scheduled: &Scheduled,
    now: Stamp,
) -> String {
    let Some(policy) = scheduled.policy() else {
        return format!("{}: no schedule", scheduled.label);
    };
    let late = scheduled.late(now);

    // **The registries are reopened per run rather than shared**, because `backup::run` takes
    // them by value — the same shape the menu uses, which reopens per job for the same reason.
    let taken = match registries.reopened() {
        Ok(fresh) => commands::backup::run(
            &mut commands::backup::Context {
                registries: fresh,
                global,
                password_command: None,
                tunnels,
            },
            Some(&scheduled.label),
            false,
            commands::backup::Mode::Sequential,
        ),
        Err(why) => Err(why),
    };

    let exit = match taken {
        Ok(exit) => exit,
        Err(why) => {
            let exit = why.exit();
            let _ = schedule::record(
                store,
                &scheduled.label,
                &Ran {
                    exit,
                    outcome: Ran::describe(exit),
                    late,
                    every: policy.every,
                },
            );
            return format!("{}: backup {}", scheduled.label, why.message());
        }
    };

    // **Pruned only after a backup that worked.** Deleting old backups because a new one
    // failed is how a retention policy turns into data loss.
    let pruned = if exit == Exit::Success && policy.prunes() {
        prune(registries, global, &scheduled.label, policy)
    } else {
        String::new()
    };

    let _ = schedule::record(
        store,
        &scheduled.label,
        &Ran {
            exit,
            outcome: Ran::describe(exit),
            late,
            every: policy.every,
        },
    );

    format!(
        "{}: backup {}{}{}",
        scheduled.label,
        Ran::describe(exit),
        if late { " (late)" } else { "" },
        pruned
    )
}

/// Apply the retention policy, and say what it came to.
fn prune(registries: &Registries, global: &Path, label: &str, policy: schedule::Policy) -> String {
    let Ok(fresh) = registries.reopened() else {
        return String::from(", and the retention policy could not be read");
    };

    let older_than = policy.keep_for_days.map(|days| format!("{days}d"));
    let done = commands::backups::prune(
        &commands::backups::Context {
            registries: fresh,
            global,
            // **`--yes` and never `--force`.** The schedule is the consent: somebody set this
            // retention on a terminal with the numbers in front of them. `--force` overrides a
            // refusal that exists to protect something, and no schedule implies that.
            consent: Consent::given(true, false, None),
        },
        &commands::backups::Pruning {
            name: Some(label),
            keep: policy.keep_last,
            older_than: older_than.as_deref(),
            dry_run: false,
            // A damaged directory is somebody's to look at, not a schedule's to delete.
            include_broken: false,
        },
    );

    match done {
        Ok(_) => String::from(", retention applied"),
        Err(why) => format!(", but the retention policy failed: {}", why.message()),
    }
}
