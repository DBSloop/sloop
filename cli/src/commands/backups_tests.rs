//! What the two commands decide before they print anything.
//!
//! The printing itself is checked by driving the binary — see `R12`'s run against a
//! throwaway cluster. What is worth pinning down here is the part that would quietly delete
//! somebody's backups if it were wrong: which flags make a policy, and which refuse to.

use std::time::Duration;

use super::{Pruning, policy_from};
use crate::exit::Exit;

/// The flags, with nothing set.
fn nothing() -> Pruning<'static> {
    Pruning {
        name: None,
        keep: None,
        older_than: None,
        dry_run: false,
        include_broken: false,
    }
}

/// **A prune with no rule is refused.** An empty policy would mean "keep none of it", and
/// the one thing this command must never do is read silence as permission.
#[test]
fn a_prune_with_no_rule_is_refused_and_names_both_flags() {
    let failure = policy_from(&nothing()).expect_err("an empty policy has to be refused");

    assert_eq!(failure.exit(), Exit::Usage);
    let hint = failure.hint_text().expect("a hint naming the flags");
    assert!(hint.contains("--keep"), "{hint}");
    assert!(hint.contains("--older-than"), "{hint}");
}

#[test]
fn a_count_alone_is_a_policy() {
    let policy = policy_from(&Pruning {
        keep: Some(7),
        ..nothing()
    })
    .expect("a count is a rule");

    assert_eq!(policy.keep, Some(7));
    assert!(policy.older_than.is_none());
    assert!(!policy.include_broken);
}

#[test]
fn an_age_alone_is_a_policy_and_is_parsed_here() {
    let policy = policy_from(&Pruning {
        older_than: Some("30d"),
        ..nothing()
    })
    .expect("an age is a rule");

    assert_eq!(policy.keep, None);
    assert_eq!(
        policy.older_than,
        Some(Duration::from_secs(30 * 24 * 60 * 60))
    );
}

/// The age is read before anything is scanned, so a typo costs nothing but the message.
#[test]
fn an_age_that_does_not_parse_fails_before_anything_is_read() {
    let failure = policy_from(&Pruning {
        older_than: Some("30 days"),
        ..nothing()
    })
    .expect_err("that is not an age");

    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure.message().contains("30 days"),
        "{}",
        failure.message()
    );
}

/// `--keep 0` is a rule, and a deliberate one: it means keep none of this label, which is
/// how somebody retires a database's history on purpose.
#[test]
fn keeping_none_is_still_a_rule() {
    let policy = policy_from(&Pruning {
        keep: Some(0),
        ..nothing()
    })
    .expect("zero is a number");

    assert_eq!(policy.keep, Some(0));
}

#[test]
fn include_broken_travels_into_the_policy() {
    let policy = policy_from(&Pruning {
        keep: Some(1),
        include_broken: true,
        ..nothing()
    })
    .expect("a rule");

    assert!(policy.include_broken);
}
