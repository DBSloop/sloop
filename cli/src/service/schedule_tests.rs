//! What can be proved about a schedule without a clock or a database.
//!
//! Which runs are owed, which were late, how an interval is spelled, and what an exit code
//! means to a schedule. What needs a real cluster — a backup taken on its schedule with no cron
//! line, a retention policy applied, a collision exiting `7` — is `service::cluster_tests`.

use std::time::Duration;

use super::{Policy, Ran, Scheduled, every_from, every_reads_as};
use crate::backup::stamp::Stamp;
use crate::exit::Exit;

/// A schedule with nothing having happened to it yet.
fn scheduled(every_seconds: Option<i64>, due_at: Option<i64>) -> Scheduled {
    Scheduled {
        label: String::from("orders"),
        every_seconds,
        keep_last: None,
        keep_for_days: None,
        due_at,
        ran_at: None,
        exit_code: None,
        outcome: None,
        was_late: None,
    }
}

const NOW: i64 = 1_789_000_000;

/// **A schedule with no run behind it is due immediately.** Somebody who asks for a daily
/// backup wants the first one today, not tomorrow — and a database with no schedule at all is
/// never due, however long it has been attached.
#[test]
fn a_new_schedule_is_owed_a_backup_at_once_and_an_unscheduled_one_never_is() {
    let now = Stamp::from_unix_seconds(NOW);

    assert!(scheduled(Some(86_400), None).due(now), "a new schedule");
    assert!(
        scheduled(Some(86_400), Some(NOW - 1)).due(now),
        "one that came due a second ago"
    );
    assert!(
        !scheduled(Some(86_400), Some(NOW + 1)).due(now),
        "one that is due in a second"
    );
    assert!(
        !scheduled(None, Some(NOW - 10_000)).due(now),
        "a database nobody asked to have backed up was called due"
    );
}

/// **Late means a whole interval was missed, not that a round fired a moment after the hour.**
/// A laptop asleep through an entire scheduled backup is late; a round that ticks five seconds
/// after the moment is not, and calling it late would make the label mean nothing.
#[test]
fn late_means_a_whole_interval_was_missed() {
    let now = Stamp::from_unix_seconds(NOW);
    let daily = 86_400;

    assert!(
        !scheduled(Some(daily), Some(NOW - 5)).late(now),
        "five seconds behind is not late"
    );
    assert!(
        !scheduled(Some(daily), Some(NOW - daily)).late(now),
        "exactly one interval behind is the edge, and the edge is not late"
    );
    assert!(
        scheduled(Some(daily), Some(NOW - daily - 1)).late(now),
        "a whole missed day is late"
    );
    assert!(
        scheduled(Some(daily), Some(NOW - 7 * daily)).late(now),
        "a week asleep is late"
    );

    // Nothing to be late for.
    assert!(!scheduled(None, Some(NOW - 10 * daily)).late(now));
    assert!(!scheduled(Some(daily), None).late(now));
}

/// **Both retention numbers unset is "keep everything"**, which is exactly what `backups prune`
/// refuses to run with — so a schedule with no policy takes backups and prunes nothing, rather
/// than deleting on a rule nobody set.
#[test]
fn a_schedule_with_no_retention_prunes_nothing() {
    let every = Duration::from_secs(86_400);

    assert!(
        !Policy {
            every,
            keep_last: None,
            keep_for_days: None
        }
        .prunes()
    );
    assert!(
        Policy {
            every,
            keep_last: Some(7),
            keep_for_days: None
        }
        .prunes()
    );
    assert!(
        Policy {
            every,
            keep_last: None,
            keep_for_days: Some(30)
        }
        .prunes()
    );
}

/// The same vocabulary `backups prune --older-than` already takes, because somebody who has
/// typed `30d` once should not have to learn a second spelling for the same idea.
#[test]
fn an_interval_is_spelled_the_way_the_rest_of_the_tool_spells_one() {
    assert_eq!(every_from("30m").unwrap(), Duration::from_secs(1_800));
    assert_eq!(every_from("6h").unwrap(), Duration::from_secs(21_600));
    assert_eq!(every_from("1d").unwrap(), Duration::from_secs(86_400));
    assert_eq!(every_from("2w").unwrap(), Duration::from_secs(1_209_600));
    assert_eq!(every_from(" 1 day ").unwrap(), Duration::from_secs(86_400));
    assert_eq!(every_from("12hours").unwrap(), Duration::from_secs(43_200));

    for nonsense in ["", "d", "1", "1y", "soon", "-1d", "1 fortnight"] {
        let failure = every_from(nonsense).expect_err(nonsense);
        assert_eq!(failure.exit().code(), 2, "{nonsense}");
    }

    // Zero is not a schedule, and saying so is kinder than a backup every no time at all.
    let failure = every_from("0d").expect_err("zero");
    assert!(
        failure.message().contains("not a schedule"),
        "{}",
        failure.message()
    );
}

/// And it reads back the way somebody would say it.
#[test]
fn an_interval_reads_back_as_words() {
    assert_eq!(every_reads_as(Duration::from_secs(86_400)), "every day");
    assert_eq!(every_reads_as(Duration::from_secs(172_800)), "every 2 days");
    assert_eq!(every_reads_as(Duration::from_secs(21_600)), "every 6 hours");
    assert_eq!(every_reads_as(Duration::from_secs(3_600)), "every hour");
    assert_eq!(every_reads_as(Duration::from_secs(604_800)), "every week");
    assert_eq!(
        every_reads_as(Duration::from_secs(1_800)),
        "every 30 minutes"
    );
}

/// **`7` is not a failure and must not read as one.** A scheduled run that collided with a
/// manual one stood aside; the backup somebody was already taking is the backup. `6` is its own
/// thing too — finished, and then the counts disagreed.
#[test]
fn a_collision_is_standing_aside_rather_than_failing() {
    assert_eq!(Ran::describe(Exit::Success), "succeeded");
    assert_eq!(
        Ran::describe(Exit::Locked),
        "stood aside — a run was already going"
    );
    assert_eq!(
        Ran::describe(Exit::Mismatch),
        "finished, but the counts disagreed"
    );
    assert_eq!(Ran::describe(Exit::Dump), "failed");
    assert_eq!(Ran::describe(Exit::Connect), "failed");

    // The words are all different, so nothing on the screen can confuse the three that matter.
    let said = [Exit::Success, Exit::Locked, Exit::Mismatch, Exit::Dump]
        .map(Ran::describe)
        .to_vec();
    let unique: std::collections::BTreeSet<&str> = said.iter().copied().collect();
    assert_eq!(
        unique.len(),
        said.len(),
        "two outcomes read the same: {said:?}"
    );
}

/// A policy survives the trip through the row and back, including the numbers that are not set.
#[test]
fn a_policy_reads_back_out_of_the_row_it_was_written_into() {
    let full = Scheduled {
        every_seconds: Some(86_400),
        keep_last: Some(7),
        keep_for_days: Some(30),
        ..scheduled(None, None)
    };
    let policy = full.policy().expect("a schedule");

    assert_eq!(policy.every, Duration::from_secs(86_400));
    assert_eq!(policy.keep_last, Some(7));
    assert_eq!(policy.keep_for_days, Some(30));
    assert!(policy.prunes());

    // No interval is no schedule, whatever the retention columns say.
    let none = Scheduled {
        every_seconds: None,
        keep_last: Some(7),
        ..scheduled(None, None)
    };
    assert!(none.policy().is_none());
}
