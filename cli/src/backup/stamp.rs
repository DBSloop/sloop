//! The moment a backup was taken, as a directory name.
//!
//! **UTC, and no dependency.** `20260916T031500Z` sorts chronologically as plain text,
//! means the same thing everywhere, and survives a clock going back an hour — none of
//! which a local timestamp manages. Turning seconds-since-the-epoch into a calendar date
//! is pure arithmetic, so it is done here rather than bought in.
//!
//! **Local time is not here yet, and that is a deliberate line.** `CLAUDE.md` says the
//! manifest records the local time and the offset as well, and *that* cannot be computed
//! without asking the operating system which timezone it is in — which means a dependency.
//! Adding one to this project's graph is a decision that belongs to `R9`, the task that
//! writes the manifest, rather than something `R8` slips in to stamp a directory. So this
//! module does the half that needs nothing, and says what the other half will need.

use std::time::{SystemTime, UNIX_EPOCH};

/// A moment, to the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Stamp {
    /// Seconds since 1970-01-01T00:00:00Z. Signed, because arithmetic on it is.
    seconds: i64,
}

impl Stamp {
    /// Now.
    ///
    /// A clock set before 1970 gives a negative value and is handled rather than refused:
    /// the arithmetic below is signed throughout, so a wrong clock produces a wrong
    /// directory name instead of a panic in the middle of a backup.
    #[must_use]
    pub fn now() -> Self {
        let seconds = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
            Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
        };
        Self { seconds }
    }

    /// Build one directly.
    ///
    /// Read by the tests, which are the only thing that can check this arithmetic against
    /// a known calendar — and by `R12`, which has to turn a directory name back into the
    /// moment it stands for in order to prune by age.
    #[allow(dead_code)]
    #[must_use]
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        Self { seconds }
    }

    /// Seconds since the epoch. See [`Stamp::from_unix_seconds`] for who reads it.
    #[allow(dead_code)]
    #[must_use]
    pub const fn unix_seconds(self) -> i64 {
        self.seconds
    }

    /// `20260916T031500Z` — the directory name.
    ///
    /// Compact ISO 8601 with no separators, because a `:` is not allowed in a Windows
    /// filename and a directory that cannot be created on one of the three platforms is
    /// not a format this project can use.
    #[must_use]
    pub fn utc_path(self) -> String {
        let (year, month, day, hour, minute, second) = self.parts();
        format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
    }

    /// `2026-09-16 03:15:00 UTC` — the same moment, for a person to read.
    ///
    /// Still UTC, and it says so. `R9` adds the local rendering that `CLAUDE.md` asks for;
    /// until then this is labelled rather than quietly presented as somebody's wall clock.
    #[must_use]
    pub fn readable_utc(self) -> String {
        let (year, month, day, hour, minute, second) = self.parts();
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
    }

    /// Year, month, day, hour, minute, second, in UTC.
    fn parts(self) -> (i64, u32, u32, u32, u32, u32) {
        // Floor division, not truncating: a negative second count belongs to the day
        // before, and `-1 / 86_400` is `0` in Rust.
        let days = self.seconds.div_euclid(86_400);
        let within = self.seconds.rem_euclid(86_400);

        let (year, month, day) = civil_from_days(days);
        let hour = u32::try_from(within / 3_600).unwrap_or(0);
        let minute = u32::try_from((within % 3_600) / 60).unwrap_or(0);
        let second = u32::try_from(within % 60).unwrap_or(0);

        (year, month, day, hour, minute, second)
    }
}

/// Turn days-since-1970 into a proleptic Gregorian date.
///
/// Howard Hinnant's `civil_from_days`, which is the algorithm every date library uses. It
/// shifts the era so that a four-hundred-year cycle starts on 1 March, which is what makes
/// the leap day fall at the end of a cycle and removes every special case for February.
/// Correct for any year this program will ever be handed.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // 719_468 is the number of days from 0000-03-01 to 1970-01-01.
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097; // [0, 146_096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11], with March as 0
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };

    (
        // January and February belong to the next year in the shifted era.
        if month <= 2 { year + 1 } else { year },
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}
