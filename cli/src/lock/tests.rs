//! That a lock is a lock, and that a second taker is refused with the frozen code.
//!
//! **Two handles in one process is the same test as two processes**, because
//! `File::try_lock` is an operating-system lock on a file rather than anything this program
//! keeps track of — which is the whole reason it was chosen. `tests/automation.rs` runs the
//! real binary twice at once as well, because a claim like this deserves both.

use super::{path_for, take};
use crate::exit::Exit;

/// A directory of this test's own.
fn scratch(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sloop-lock-{}-{label}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&path).expect("a temporary directory");
    path
}

/// **The `Done when`: one gets it, the other gets 7.**
#[test]
fn a_second_taker_is_refused_with_seven() {
    let store = scratch("held");

    let first = take(&store, "orders", "backup").expect("nobody has it");
    let second = take(&store, "orders", "backup").expect_err("the first one has it");

    assert_eq!(second.exit(), Exit::Locked);
    assert_eq!(second.exit().code(), 7);
    assert!(second.message().contains("orders"), "{}", second.message());
    assert!(
        second.message().contains("sloop backup"),
        "the refusal has to name what is holding it: {}",
        second.message()
    );
    assert!(
        second
            .hint_text()
            .is_some_and(|hint| hint.contains("did not start")),
        "exit 7 has to say it is not a failure: {:?}",
        second.hint_text()
    );

    drop(first);
    let _ = std::fs::remove_dir_all(&store);
}

/// Released when the holder goes, so the next run is an ordinary one.
///
/// **Given a moment, and the moment is the operating system's.** What sloop guarantees is
/// that the lock goes when the handle does; that the *next* `flock` in the same process
/// observes it having gone within zero nanoseconds is a timing property no operating system
/// promises, and macOS intermittently does not provide it under a loaded parallel test run —
/// which is what kept `test (macos-latest)` red while the other two were green.
///
/// So this waits, and the bound is the point. Two seconds is far longer than any real gap
/// this matters for — a lock that is still held a second after its holder went would be a
/// defect, and this still fails on one — while a lock that never goes, which is the failure
/// worth catching, fails exactly as loudly as before. How long it actually took is printed
/// whenever it is not instant, so the magnitude is in the log rather than in somebody's
/// guess.
#[test]
fn the_lock_goes_when_the_holder_does() {
    let store = scratch("released");

    drop(take(&store, "orders", "backup").expect("nobody has it"));

    let waited = std::time::Instant::now();
    let mut last = None;
    let again = loop {
        match take(&store, "orders", "backup") {
            Ok(held) => break held,
            Err(refusal) => {
                assert!(
                    waited.elapsed() < std::time::Duration::from_secs(2),
                    "the lock did not go when its holder did: {}",
                    refusal.message()
                );
                last = Some(refusal);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    };

    if last.is_some() {
        eprintln!(
            "the lock took {:?} to be released after its holder was dropped",
            waited.elapsed()
        );
    }

    drop(again);
    let _ = std::fs::remove_dir_all(&store);
}

/// **One lock per database, not one per machine.** Two databases back up at once, which is
/// the whole reason this is keyed by name.
#[test]
fn two_databases_do_not_wait_for_each_other() {
    let store = scratch("two");

    let orders = take(&store, "orders", "backup").expect("nobody has orders");
    let app = take(&store, "app", "backup").expect("nobody has app, either");

    drop((orders, app));
    let _ = std::fs::remove_dir_all(&store);
}

/// A label that is not a filename still has to produce one, and the same one every time —
/// otherwise the lock locks nothing.
#[test]
fn an_awkward_label_still_names_one_file() {
    let store = scratch("awkward");

    for label in ["Test Sloop DB 2", "a/b", "..", "with:colon"] {
        let path = path_for(&store, label);
        assert_eq!(
            path.parent().and_then(std::path::Path::file_name),
            Some(std::ffi::OsStr::new("locks")),
            "{label} escaped the locks directory: {}",
            path.display()
        );
        assert_eq!(path_for(&store, label), path, "{label} is not stable");

        let held = take(&store, label, "backup").expect("nobody has it");
        take(&store, label, "backup").expect_err("this one does");
        drop(held);
    }

    let _ = std::fs::remove_dir_all(&store);
}

/// The file says who has it, so the run that is refused can name them.
#[test]
fn the_file_says_which_run_is_holding_it() {
    let store = scratch("who");

    let held = take(&store, "orders", "backup").expect("nobody has it");
    // Beside the lock, not inside it: on Windows the lock covers the bytes too, and this is
    // the one moment somebody needs to read them.
    let said = std::fs::read_to_string(super::beside(held.path())).expect("it wrote something");

    assert!(said.contains("sloop backup"), "{said}");
    assert!(
        said.contains(&std::process::id().to_string()),
        "it does not say which process: {said}"
    );

    drop(held);
    let _ = std::fs::remove_dir_all(&store);
}
