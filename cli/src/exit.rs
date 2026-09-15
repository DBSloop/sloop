//! The exit codes, frozen at 1.0.
//!
//! Whatever calls `sloop` from a scheduler reads these and nothing else. Adding a code is
//! a feature; changing what one of them means is a breaking release. They are listed in
//! `--help` for the same reason they are listed here: the contract is the product.
//!
//! `1` is deliberately absent from the owner's table. It keeps its universal meaning —
//! something went wrong and it was none of the below — which is also what a process gets
//! from a panic, so nothing has to pretend a crash was a category.

use std::process::ExitCode;

/// Every code `sloop` is allowed to exit with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Exit {
    /// The command did what it said it would.
    Success = 0,
    /// Something went wrong that none of the specific codes describe.
    Failure = 1,
    /// Bad usage, an unknown database name, or a question that could not be asked
    /// because there was no terminal to ask it in.
    Usage = 2,
    /// The database refused or never answered.
    Connect = 3,
    /// The dump failed.
    Dump = 4,
    /// The restore failed.
    Restore = 5,
    /// It finished, and then the row counts disagreed. Neither success nor a crash.
    Mismatch = 6,
    /// Another run holds the lock on this database.
    Locked = 7,
}

impl Exit {
    /// The numeric code, for the places that need it as a number.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        Self::from(exit.code())
    }
}

#[cfg(test)]
mod tests {
    use super::Exit;

    /// If this test is ever edited to make it pass, the edit is a breaking release.
    #[test]
    fn the_table_is_frozen() {
        assert_eq!(Exit::Success.code(), 0);
        assert_eq!(Exit::Failure.code(), 1);
        assert_eq!(Exit::Usage.code(), 2);
        assert_eq!(Exit::Connect.code(), 3);
        assert_eq!(Exit::Dump.code(), 4);
        assert_eq!(Exit::Restore.code(), 5);
        assert_eq!(Exit::Mismatch.code(), 6);
        assert_eq!(Exit::Locked.code(), 7);
    }
}
