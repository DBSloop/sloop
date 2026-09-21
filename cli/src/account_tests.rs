//! What can be checked from any machine.
//!
//! **The rest of this module is the operating system, and is checked by running it.** Whether
//! `useradd` makes an account, whether a dropped uid can create a data directory, whether a
//! postmaster started that way comes up — none of that has an answer a unit test can give,
//! and inventing a fake `id` to assert against would prove only that the fake works. Those
//! are `R31`'s cluster checks, on Linux, as root. See the entry.

use super::{SHARED_DIR, SHARED_FILE, Service, how_to_join, parse_number};

#[test]
fn an_id_is_the_number_it_printed() {
    assert_eq!(parse_number("0\n"), Some(0));
    assert_eq!(parse_number("998"), Some(998));
    assert_eq!(parse_number("  1001  \n"), Some(1001));
}

#[test]
fn anything_that_is_not_a_number_is_no_answer() {
    // `id` on an account that does not exist prints its complaint to stderr and nothing to
    // stdout, so this is the shape of "no such account" by the time it reaches here.
    assert_eq!(parse_number(""), None);
    assert_eq!(parse_number("\n"), None);
    assert_eq!(parse_number("id: 'sloop': no such user"), None);
    assert_eq!(parse_number("-1"), None);
}

#[test]
fn windows_is_never_root() {
    // Not a tautology: it is what keeps the whole of `R31` off a platform that has no uid to
    // drop to. If this ever answered `true` there, `pick` would send a Windows store to
    // `/var/lib/sloop`.
    if cfg!(windows) {
        assert!(!super::is_root());
    }
}

#[test]
fn the_shared_store_carries_its_group_down() {
    // The setgid bit is not decoration: it is what makes a backup written by one account
    // readable by the next without anybody setting a umask. `share_file` also reads it back
    // as the mark of a shared store, so losing it here would quietly stop that too.
    assert_eq!(SHARED_DIR & 0o2000, 0o2000, "setgid");
    assert_eq!(
        SHARED_DIR & 0o070,
        0o070,
        "the group can read, write and enter"
    );
    assert_eq!(SHARED_DIR & 0o007, 0o005, "and everybody else only looks");

    assert_eq!(SHARED_FILE & 0o060, 0o060, "the group can read and write");
    assert_eq!(SHARED_FILE & 0o007, 0, "nobody outside it can do either");
}

#[test]
fn joining_the_group_is_one_line_and_says_to_log_in_again() {
    let said = how_to_join(&Service {
        name: "sloop".to_owned(),
        uid: 998,
        gid: 998,
    });

    assert!(said.contains("usermod -aG sloop"), "{said}");
    assert!(
        said.contains("new login"),
        "a shell that was open before the account joined does not have the group yet: {said}"
    );
}
