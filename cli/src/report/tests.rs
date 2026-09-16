//! The settings themselves are process-wide and settled once, so what can be tested here is
//! the part that decides — not the printing, which needs a process of its own.
//!
//! `tests/flags.rs` drives the real binary for the rest.

use super::Asked;

/// The default is the tool as it was before any of these flags existed.
#[test]
fn nothing_asked_for_is_the_tool_as_it_was() {
    let quiet = Asked::default();
    assert!(!quiet.quiet);
    assert!(!quiet.json);
    assert!(!quiet.no_color);
    assert!(quiet.log_file.is_none());
}
