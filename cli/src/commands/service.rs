//! `sloop service …` — the seven things somebody does to a background service.
//!
//! **`status` asks the machine, never a row in a database.** sloop has a `service` table and
//! it would have been easy to write "installed" into it and read that back — but a row saying
//! installed while systemd has never heard of the unit is worse than having no row, and the
//! case where the two disagree is exactly the case somebody is running `status` to understand.
//! So the service manager is the source of truth for whether it exists and whether it is
//! running, every time.
//!
//! **What the table holds is what the machine genuinely does not know**, which `R25` filled
//! in: which databases are attached, and when a running service last read each attachment.
//! `status` prints both halves together, because *installed, running, and watching nothing*
//! is a state somebody needs to be able to see.
//!
//! **`attach` and `detach` do not need a service manager at all.** They are rows, and they
//! are reached before this machine is asked which of the three it uses — so attaching on a
//! machine where sloop has not been installed yet works, and is said rather than refused.
//! Rule 0d: the feature works, and the thing that is missing is named.

use crate::cli::ServiceCommand;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::locations::Locations;
use crate::registry::store::Store;
use crate::service::mechanism::{Mechanism, State};
use crate::service::schedule;
use crate::service::unit::Definition;
use crate::service::watch::{self, Attached, Attachment};
use crate::service::{credentials, daemon, elevation, key, manage};
use crate::style;

/// Run one of them.
pub fn run(locations: &Locations, command: &ServiceCommand) -> Outcome<Exit> {
    // The daemon first, and before anything that could print: on Windows this hands the
    // thread to the Service Control Manager, which owns the process's streams from then on.
    if let ServiceCommand::Run { store, interval } = command {
        return daemon::run(store.clone(), *interval);
    }

    // **Before the machine is asked what runs things at boot**, because an attachment is a
    // row and does not care. A machine with no service yet can be set up in either order.
    match command {
        ServiceCommand::Attach { name } => return attach(locations, name),
        ServiceCommand::Detach { name } => return detach(locations, name),
        // `R27`. Reading what was recorded needs the store and not the service manager, so a
        // machine whose service is stopped still answers about the months before it stopped.
        ServiceCommand::Activity => return super::activity::run(locations),
        ServiceCommand::Schedule {
            name,
            every,
            keep,
            keep_for_days,
            off,
        } => {
            return schedule(
                locations,
                name,
                &Asked {
                    every: every.as_deref(),
                    keep: *keep,
                    keep_for_days: *keep_for_days,
                    off: *off,
                },
            );
        }
        _ => {}
    }

    let Some(mechanism) = Mechanism::of_this_machine() else {
        return Err(Failure::new(
            Exit::Failure,
            "sloop does not know how this machine starts things at boot",
        )
        .hint(
            "systemd, launchd and the Windows Service Control Manager are the three it \
             knows. Run sloop from this machine's own scheduler instead.",
        ));
    };

    match command {
        ServiceCommand::Install {
            no_start,
            user,
            interval,
        } => {
            // **Asked before anything is done, not discovered halfway through.** Installing
            // writes a key file under `%ProgramData%` or `/etc` and copies credentials into
            // it *before* it reaches the service manager, so an un-elevated run used to
            // print two lines about installing and then fail on a path nobody typed —
            // having already made a directory that was not there, holding a key file it
            // could no longer read back.
            elevation::require(mechanism)?;
            install(locations, mechanism, *no_start, user.as_deref(), *interval)
        }
        ServiceCommand::Uninstall => {
            // **After the state, not before it.** Asking somebody to find an administrator
            // and then telling them there was nothing to remove is two wasted trips. What
            // is installed is readable by anyone; changing it is not.
            if manage::state(mechanism).installed() {
                elevation::require(mechanism)?;
            }
            Ok(uninstall(locations, mechanism))
        }
        ServiceCommand::Start => {
            already(mechanism, true)?;
            elevation::require(mechanism)?;
            manage::start(mechanism)?;
            crate::say!("{} {}", style::heading("Started."), settle(mechanism));
            Ok(Exit::Success)
        }
        ServiceCommand::Stop => {
            already(mechanism, true)?;
            elevation::require(mechanism)?;
            manage::stop(mechanism)?;
            crate::say!(
                "{} {}",
                style::heading("Stopped."),
                style::dim("it still starts again at the next boot")
            );
            Ok(Exit::Success)
        }
        ServiceCommand::Status => Ok(status(locations, mechanism)),
        ServiceCommand::Run { .. }
        | ServiceCommand::Attach { .. }
        | ServiceCommand::Detach { .. }
        | ServiceCommand::Activity
        | ServiceCommand::Schedule { .. } => unreachable!("handled above"),
    }
}

/// Register it, and start it unless told not to.
fn install(
    locations: &Locations,
    mechanism: Mechanism,
    no_start: bool,
    user: Option<&str>,
    interval: u64,
) -> Outcome<Exit> {
    let program = std::env::current_exe().map_err(|error| {
        Failure::new(
            Exit::Failure,
            format!("could not work out where sloop itself is: {error}"),
        )
    })?;

    let store = crate::registry::adopt::global(locations)?;

    // Windows services run as `LocalSystem` and are told where the store is instead; naming
    // an account there would be accepted and then ignored, which is worse than refusing it.
    if user.is_some() && mechanism == Mechanism::WindowsService {
        return Err(Failure::usage(
            "--user is not for Windows: a sloop service runs as LocalSystem",
        )
        .hint("it is told where the store is instead, which install does by itself"));
    }

    let account = match mechanism {
        Mechanism::WindowsService => None,
        _ => Some(user.map_or_else(whoami, str::to_owned)),
    };

    crate::say!(
        "{} {}",
        style::heading("Installing"),
        style::dim(&format!("with {}", mechanism.spoken()))
    );

    let key_file = key::write(account.as_deref())?;

    // **Before the unit, because it is the half that actually makes the service work.** A
    // service runs as another account and cannot read the keyring this session can; this is
    // the one moment both are true, so the copy is made now or never. See
    // `service::credentials`.
    credentials::announce(
        credentials::copy_for_the_service(&store, &key_file)?,
        &store,
    );

    let definition = Definition {
        program,
        store,
        account,
        key_file,
        // Clamped here rather than at the flag, so what goes in the unit is what the daemon
        // will actually do — a definition that says 1 and a service that waits 5 is a unit
        // file that lies to whoever reads it.
        interval: daemon::interval(interval).as_secs(),
    };

    manage::install(mechanism, &definition)?;
    if let Some(path) = mechanism.definition_file() {
        crate::say!("  {} {}", style::label("Wrote"), path.display());
    }

    if no_start {
        crate::say!("  {}", style::dim("left stopped, as asked"));
    } else if mechanism != Mechanism::Launchd {
        // launchd's `bootstrap` has already started it; asking again is an error there.
        manage::start(mechanism)?;
    }

    crate::say!("");
    crate::say!("{} {}", style::heading("Done."), settle(mechanism));
    crate::say!(
        "  {}",
        style::dim(&format!(
            "check it yourself with `{}`",
            mechanism.their_own_check()
        ))
    );
    Ok(Exit::Success)
}

/// Take it off, and say what would not go.
fn uninstall(locations: &Locations, mechanism: Mechanism) -> Exit {
    if !manage::state(mechanism).installed() {
        crate::say!(
            "{}",
            style::dim("there is no sloop service on this machine")
        );
        return Exit::Success;
    }

    crate::say!(
        "{} {}",
        style::heading("Removing"),
        style::dim(&format!("from {}", mechanism.spoken()))
    );

    // **The copies before the key file**, because reading them back needs the passphrase that
    // file holds. Best effort: a store that will not open is one this machine put there for
    // another reason, and it is left alone.
    let mut trouble = Vec::new();
    if let Ok(store) = crate::registry::adopt::global(locations) {
        trouble.extend(credentials::remove_the_copies(&store, &key::path()));
    }

    trouble.extend(manage::uninstall(mechanism));
    trouble.extend(key::remove());

    for what in &trouble {
        crate::note!("{}", style::dim(&format!("{what} — remove it by hand")));
    }

    crate::say!("");
    crate::say!(
        "{} {}",
        style::heading("Done."),
        style::dim("nothing sloop recorded was deleted — attachments and history stay")
    );
    Exit::Success
}

/// Watch a database.
///
/// **Its own store, opened here rather than passed in.** `main` dispatches `service` before it
/// opens a registry, because `install`, `start`, `stop` and `run` have no use for one — and
/// two subcommands that do is not a reason for the other four to carry it.
fn attach(locations: &Locations, name: &str) -> Outcome<Exit> {
    let store = Store::require(&crate::registry::adopt::global(locations)?)?;

    let attached = watch::attach(&store, name)?;

    // A rehearsal has already said its piece — `--dry-run` printed what it would have done and
    // recorded it — so there is nothing true left for this to add.
    if attached == Attached::Rehearsed {
        return Ok(Exit::Success);
    }

    crate::report::result(serde_json::json!({
        "attached": name,
        "already": attached == Attached::Already,
    }));

    match attached {
        Attached::Now => crate::say!(
            "{} {}",
            style::heading("Attached."),
            style::dim(&format!("the service watches {name} from its next round"))
        ),
        Attached::Already => crate::say!(
            "{}",
            style::dim(&format!("{name} was already attached — nothing changed"))
        ),
        Attached::Rehearsed => unreachable!("returned above"),
    }

    // Said after the fact rather than instead of it: the row is the instruction, and an
    // instruction is worth leaving for a service that is not installed yet.
    not_running_yet();
    Ok(Exit::Success)
}

/// Stop watching one, and say what was kept.
fn detach(locations: &Locations, name: &str) -> Outcome<Exit> {
    let store = Store::require(&crate::registry::adopt::global(locations)?)?;

    let detached = watch::detach(&store, name)?;
    if detached.rehearsed {
        return Ok(Exit::Success);
    }

    crate::report::result(serde_json::json!({
        "detached": name,
        "was_attached": detached.was_attached,
        "days_of_history": detached.days_of_history,
    }));

    if detached.was_attached {
        crate::say!(
            "{} {}",
            style::heading("Detached."),
            style::dim(&format!(
                "the service stops watching {name} at its next round"
            ))
        );
    } else {
        crate::say!(
            "{}",
            style::dim(&format!("{name} was not attached — nothing changed"))
        );
    }

    // **The `Done when`, said out loud.** Detaching stops the samples and deletes nothing:
    // the history is keyed by the database rather than by the attachment, so there is no
    // path from here to it. `sloop backups prune` is what deletes things, when asked.
    crate::say!(
        "  {}",
        style::dim(&match detached.days_of_history {
            0 => String::from("nothing had been recorded about it yet"),
            1 => String::from("the one day of activity already recorded is kept"),
            days => format!("the {days} days of activity already recorded are kept"),
        })
    );
    Ok(Exit::Success)
}

/// What `service schedule` was asked for.
pub struct Asked<'a> {
    /// How often, as typed.
    pub every: Option<&'a str>,
    /// Keep this many of the newest.
    pub keep: Option<usize>,
    /// Prune anything older than this many days.
    pub keep_for_days: Option<u32>,
    /// Stop backing it up.
    pub off: bool,
}

/// Set, change or clear one database's backup schedule.
///
/// **`R27a`'s whole point, in one command.** The schedule is a row, not a crontab line — and
/// what somebody types here is the same vocabulary `backups prune` already takes, because
/// learning a second spelling for the same idea is a cost with nothing on the other side.
fn schedule(locations: &Locations, name: &str, asked: &Asked<'_>) -> Outcome<Exit> {
    let store = Store::require(&crate::registry::adopt::global(locations)?)?;

    // The name has to be attached: a schedule on a database the service is not watching is a
    // backup that will never be taken, and silently accepting one would be the worst kind of
    // working.
    let attached = watch::attachments(&store)?;
    if !attached.iter().any(|one| one.label == name) {
        return Err(Failure::usage(format!(
            "{name} is not attached to the service, so nothing would take its backups"
        ))
        .hint(format!(
            "`sloop service attach {name}` first — `sloop service status` lists what is attached"
        )));
    }

    if asked.off {
        if crate::report::would(&format!("stop backing {name} up on a schedule")) {
            return Ok(Exit::Success);
        }
        schedule::set(&store, name, None)?;
        crate::report::result(serde_json::json!({ "scheduled": name, "off": true }));
        crate::say!(
            "{} {}",
            style::heading("Stopped."),
            style::dim(&format!(
                "{name} is still attached and still sampled — nothing backs it up now"
            ))
        );
        return Ok(Exit::Success);
    }

    let Some(every) = asked.every else {
        return Err(
            Failure::usage("a schedule needs to know how often").hint(
                "`--every 1d` is once a day, `--every 6h` is four times a day, and `--off`                  stops it",
            ),
        );
    };

    // **The key has to have been kept before a schedule is worth setting**, and this is the
    // only moment there is a terminal to say so at.
    //
    // Driving `R27a` is what found it: `R11` refuses the first encrypted backup until the key
    // has been exported or explicitly declined, and a service has no terminal — so a schedule
    // set on a registry with a fresh key would have produced a backup that never happened, once
    // a minute, for ever. Refusing here is refusing in the one place somebody can do something
    // about it. See "The service could never take its first backup" in
    // `docs/OWNER-DECISIONS.md`.
    key_has_been_kept(locations)?;

    let policy = schedule::Policy {
        every: schedule::every_from(every)?,
        keep_last: asked.keep,
        keep_for_days: asked.keep_for_days,
    };

    if crate::report::would(&format!(
        "back {name} up {}",
        schedule::every_reads_as(policy.every)
    )) {
        return Ok(Exit::Success);
    }

    schedule::set(&store, name, Some(policy))?;
    crate::report::result(serde_json::json!({
        "scheduled": name,
        "every_seconds": policy.every.as_secs(),
        "keep_last": policy.keep_last,
        "keep_for_days": policy.keep_for_days,
    }));

    crate::say!(
        "{} {}",
        style::heading("Scheduled."),
        style::dim(&format!(
            "{name} is backed up {}",
            schedule::every_reads_as(policy.every)
        ))
    );
    crate::say!(
        "  {}",
        style::dim(&match (policy.keep_last, policy.keep_for_days) {
            (None, None) => String::from(
                "nothing is pruned — `--keep 7` or `--keep-for-days 30` sets a retention policy"
            ),
            (Some(keep), None) => format!("the newest {keep} are kept, the rest are pruned"),
            (None, Some(days)) => format!("anything older than {days} days is pruned"),
            (Some(keep), Some(days)) =>
                format!("the newest {keep} are kept, and anything older than {days} days is pruned"),
        })
    );
    crate::say!(
        "  {}",
        style::dim("no cron line, no scheduled task — the service takes it")
    );

    not_running_yet();
    Ok(Exit::Success)
}

/// Refuse a schedule while the backup key is still only on this machine.
///
/// **A registry with no key at all is fine**: the first scheduled backup makes one, and the
/// same refusal fires then — but it fires inside the daemon, where nobody can answer it. So the
/// question is asked here, where somebody is standing: has this registry a key, and has it been
/// copied anywhere?
fn key_has_been_kept(locations: &Locations) -> Outcome<()> {
    let global = crate::registry::adopt::global(locations)?;
    let registries =
        crate::registry::Registries::open(crate::registry::Resolution::global_only(), &global)?;

    let kept = registries
        .in_scope(crate::registry::Scope::Global)
        .and_then(|registry| registry.encryption())
        .map(|encryption| encryption.key_kept.is_some());

    match kept {
        // A key that has been exported, or one somebody was told the cost of and kept anyway.
        Some(true) => Ok(()),
        // There is a key and nobody has copied it. `R11`'s refusal, moved to where it can be
        // answered.
        Some(false) => Err(Failure::usage(
            "this registry's backup key has not been copied anywhere yet, and a scheduled \
             backup has no terminal to ask at",
        )
        .hint(
            "`sloop key export > backup-key.txt` writes it out — keep that somewhere other \
             than this machine, then set the schedule again",
        )),
        // No key yet. The first backup would make one and then refuse, inside a daemon.
        None => Err(Failure::usage(
            "this registry has no backup key yet, and the first scheduled backup would make \
             one and then stop to ask about it with no terminal to ask at",
        )
        .hint(
            "`sloop backup <name>` once by hand makes the key and asks the question, or \
             `sloop key export` makes it and writes it out",
        )),
    }
}

/// A line, when there is no service to pick an attachment up.
///
/// Not a failure and not a warning: attaching before installing is an ordinary order to do
/// things in, and the only thing wrong with it is that nobody said what happens next. Which
/// of the two sentences it is matters — *stopped* and *never installed* are `R24`'s two
/// different states, and the commands that fix them are different commands.
fn not_running_yet() {
    let state = Mechanism::of_this_machine().map(manage::state);

    let next = match state {
        Some(State::Running) => return,
        Some(State::NotInstalled) | None => {
            "no sloop service is installed here — `sloop service install` registers one and \
             starts it"
        }
        _ => "the sloop service is installed and stopped — `sloop service start` starts it",
    };

    crate::say!("  {}", style::dim(next));
}

/// Installed, running, and coming back at boot — each one separately, because they are
/// separate questions and a monitoring system needs them apart.
fn status(locations: &Locations, mechanism: Mechanism) -> Exit {
    let state = manage::state(mechanism);

    crate::say!("{}", style::heading("Service"));
    crate::say!("  {} {}", style::label("State"), state.spoken());
    crate::say!("  {} {}", style::label("Managed by"), mechanism.spoken());

    if state.installed() {
        crate::say!(
            "  {} {}",
            style::label("At boot"),
            if manage::starts_at_boot(mechanism) {
                "starts by itself"
            } else {
                "does not start by itself"
            }
        );
        if let Some(path) = mechanism.definition_file() {
            crate::say!("  {} {}", style::label("Defined in"), path.display());
        }
        crate::say!(
            "  {}",
            style::dim(&format!(
                "this machine's own answer: `{}`",
                mechanism.their_own_check()
            ))
        );
    } else {
        crate::say!(
            "  {}",
            style::dim("`sloop service install` registers it with this machine")
        );
    }

    watching(locations, state.running());

    // **Not an error, whatever the answer.** `status` is what a monitoring system runs, and a
    // command that exits non-zero because the thing it reports on is stopped cannot be used
    // to find out that it is stopped. That is why this returns a code rather than an
    // `Outcome`: there is no path through it that can fail.
    Exit::Success
}

/// The other half of `status`: what is attached, and whether the service has picked it up.
///
/// **It cannot fail either, for the same reason the half above it cannot.** A PostgreSQL that
/// is down is something `status` should *say*, not something that should stop it answering
/// about systemd. So every way this can go wrong becomes a dim line and the exit code stays
/// `0`.
fn watching(locations: &Locations, running: bool) {
    let (attachments, last_seen, schedules) = match read(locations) {
        Ok(Some(both)) => both,
        // **`null`, not `[]`, and the difference is the whole point.** An empty list means
        // nothing is attached; a null means sloop could not find out. A monitoring script that
        // could not tell those apart would read a machine with no store as a machine with no
        // databases to back up.
        otherwise => {
            crate::report::result(serde_json::json!({
                "running": running,
                "last_seen": serde_json::Value::Null,
                "watching": serde_json::Value::Null,
            }));
            crate::say!("");
            crate::say!(
                "{} {}",
                style::heading("Watching"),
                style::dim(&match otherwise {
                    Err(failure) => format!("unknown — {}", failure.message()),
                    _ => String::from(
                        "unknown — this machine has not been set up. Run `sloop setup`."
                    ),
                })
            );
            return;
        }
    };

    let schedule_json = |label: &str| {
        schedules
            .iter()
            .find(|one| one.label == label)
            .and_then(|one| {
                let policy = one.policy()?;
                Some(serde_json::json!({
                    "every_seconds": policy.every.as_secs(),
                    "keep_last": policy.keep_last,
                    "keep_for_days": policy.keep_for_days,
                    "last_run_at": one.ran_stamp().map(|at| at.local().iso()),
                    "next_run_at": one.due_stamp().map(|at| at.local().iso()),
                    "last_outcome": one.outcome,
                    "last_exit_code": one.exit_code,
                    "last_was_late": one.was_late,
                }))
            })
    };

    crate::report::result(serde_json::json!({
        "running": running,
        "last_seen": last_seen.map(|stamp| stamp.local().iso()),
        "watching": attachments
            .iter()
            .map(|one| serde_json::json!({
                "label": one.label,
                "attached_at": one.attached_at.local().iso(),
                "seen_at": one.seen_at.map(|stamp| stamp.local().iso()),
                "days_of_history": one.days_of_history,
                "schedule": schedule_json(&one.label),
            }))
            .collect::<Vec<_>>(),
    }));

    crate::say!("");
    crate::say!(
        "{} {}",
        style::heading("Watching"),
        if attachments.is_empty() {
            style::dim("nothing — `sloop service attach <name>` attaches a database")
        } else {
            style::dim(&plural(attachments.len()))
        }
    );

    for one in &attachments {
        crate::say!("  {}", line_for(one));

        // **Rule 14: the schedule is checked, not trusted.** A schedule that has silently
        // stopped is visible here rather than being discovered by the backup nobody took.
        if let Some(scheduled) = schedules.iter().find(|s| s.label == one.label) {
            crate::say!("  {}", style::dim(&schedule_line(scheduled)));
        }
    }

    if attachments.is_empty() {
        return;
    }

    // **Local time, like every other moment this tool shows a person.** The column is
    // `TIMESTAMPTZ` and the paths are UTC; what a human reads is their own clock.
    crate::say!(
        "  {}",
        style::dim(&match last_seen {
            Some(stamp) => format!(
                "the service last read this list at {}",
                stamp.local().readable()
            ),
            None => String::from("no service has ever read this list"),
        })
    );
}

/// The attachment list and the heartbeat, or `None` on a machine that was never set up.
///
/// **Both in one function so `status` opens the store once.** `Ok(None)` is "there is nothing
/// to read from", which is every machine before `sloop setup` and is not an error; `Err` is a
/// store that exists and would not answer, which is.
type Watched = (
    Vec<Attachment>,
    Option<crate::backup::stamp::Stamp>,
    Vec<schedule::Scheduled>,
);

fn read(locations: &Locations) -> Outcome<Option<Watched>> {
    let global = crate::registry::adopt::global(locations)?;
    let Some(store) = Store::open(&global)? else {
        return Ok(None);
    };

    // The heartbeat is allowed to be missing and the list is not: `last_seen` comes back `None`
    // on a machine where the daemon has never run, which is an answer rather than a failure.
    Ok(Some((
        watch::attachments(&store)?,
        watch::last_seen(&store)?,
        schedule::all(&store)?,
    )))
}

/// What a database's schedule is doing, in one line under it.
///
/// **Last and next, because they answer different questions.** Rule 14 asks that a schedule be
/// checked rather than trusted, and *"it ran at 3am"* does not say whether the next one is due
/// in an hour or was due yesterday.
fn schedule_line(scheduled: &schedule::Scheduled) -> String {
    let Some(policy) = scheduled.policy() else {
        return String::from("    no backup schedule — `sloop service schedule <name> --every 1d`");
    };

    let last = match (scheduled.ran_stamp(), scheduled.outcome.as_deref()) {
        (Some(when), Some(outcome)) => format!(
            "last {} {}{}",
            when.local().readable(),
            outcome,
            if scheduled.was_late == Some(true) {
                " (late)"
            } else {
                ""
            }
        ),
        _ => String::from("never run"),
    };

    let next = scheduled.due_stamp().map_or_else(
        || String::from("no next run"),
        |when| format!("next {}", when.local().readable()),
    );

    format!(
        "    backed up {} · {last} · {next}",
        schedule::every_reads_as(policy.every)
    )
}

/// One attachment, as one line.
///
/// **`seen_at` is the column that answers `R25`'s question**, so it is what the line ends on:
/// a database attached to a running service says when that service picked it up, and one
/// attached while nothing was running says plainly that nothing has.
fn line_for(one: &Attachment) -> String {
    let picked_up = match one.seen_at {
        Some(stamp) => format!("picked up {}", stamp.local().readable()),
        None => String::from("not picked up yet"),
    };

    format!(
        "{}  {}",
        style::paint(&one.label),
        style::dim(&format!(
            "attached {} · {picked_up} · {}",
            one.attached_at.local().readable(),
            match one.days_of_history {
                0 => String::from("no activity recorded"),
                1 => String::from("1 day of activity"),
                days => format!("{days} days of activity"),
            }
        ))
    )
}

/// `1 database` / `2 databases`, because a count with the wrong noun beside it reads as a bug.
fn plural(count: usize) -> String {
    if count == 1 {
        String::from("1 database")
    } else {
        format!("{count} databases")
    }
}

/// Refuse the commands that only make sense against something already installed.
fn already(mechanism: Mechanism, needed: bool) -> Outcome<()> {
    if needed && !manage::state(mechanism).installed() {
        return Err(Failure::usage("there is no sloop service on this machine")
            .hint("`sloop service install` registers it first"));
    }
    Ok(())
}

/// What the machine says now, in one clause, for the end of a command that changed it.
fn settle(mechanism: Mechanism) -> String {
    match manage::state(mechanism) {
        State::Running => String::from("it is running."),
        State::Stopped => String::from("it is installed and stopped."),
        State::NotInstalled => String::from("this machine still does not have it."),
        State::Unclear(what) => format!("the machine says: {what}"),
    }
}

/// Whoever is running this, for the account the unit names.
///
/// `id -un` rather than `$USER`: a sudo'd shell keeps `$USER` as the original user on some
/// systems and rewrites it on others, and the unit has to name the account that will actually
/// own the store.
fn whoami() -> String {
    std::process::Command::new("id")
        .arg("-un")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| std::env::var("SUDO_USER").ok())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| String::from("root"))
}
