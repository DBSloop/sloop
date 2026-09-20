//! The moment a backup was taken: UTC for the path, local for every human.
//!
//! **UTC in the directory name, and no dependency for it.** `20260916T031500Z` sorts
//! chronologically as plain text, means the same thing everywhere, and survives a clock
//! going back an hour — none of which a local timestamp manages. Turning
//! seconds-since-the-epoch into a calendar date is pure arithmetic, so it is done here
//! rather than bought in.
//!
//! **The local half is R9's, and it is where the one dependency went.** R8 stopped at UTC
//! and said why: the offset from UTC cannot be computed, only looked up, and looking it up
//! means asking the operating system which timezone it is in. That is `chrono`, and it is
//! used for exactly one thing — [`offset_seconds_at`] — while the calendar arithmetic
//! below stays where it was and stays tested against a known calendar.
//!
//! **The offset is looked up *at the moment in question*, not now.** A backup taken in
//! July still reads back as July's wall clock when it is listed in December, which a
//! single cached "current offset" would get wrong by an hour for half the year. It is
//! recorded in the manifest as well, so a backup copied to a machine in another timezone
//! still says what the clock said where it was taken.

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
    #[must_use]
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        Self { seconds }
    }

    /// Seconds since the epoch. See [`Stamp::from_unix_seconds`] for who reads it.
    #[must_use]
    pub const fn unix_seconds(self) -> i64 {
        self.seconds
    }

    /// How long ago this was, in the words somebody glancing at a menu wants.
    ///
    /// **Rounded down, and coarse on purpose.** A home screen saying *"last backup 2h ago"*
    /// answers the question being asked; one saying *"1h 58m 12s ago"* makes the reader do
    /// the rounding themselves. `backups list` still prints the exact local time — this is
    /// the glance, not the record.
    ///
    /// `now` is passed in rather than read so that the arithmetic can be tested against a
    /// known clock.
    #[must_use]
    pub fn ago(self, now: Self) -> String {
        let seconds = now.seconds.saturating_sub(self.seconds);
        // A backup dated in the future is a clock that moved, not a thing to do maths on.
        if seconds < 0 {
            return "just now".to_owned();
        }
        match seconds {
            0..60 => "just now".to_owned(),
            60..3_600 => format!("{}m ago", seconds / 60),
            3_600..86_400 => format!("{}h ago", seconds / 3_600),
            86_400..604_800 => format!("{}d ago", seconds / 86_400),
            other => format!("{}w ago", other / 604_800),
        }
    }

    /// Read a directory name back into the moment it stands for.
    ///
    /// Exactly the shape [`Stamp::utc_path`] writes — `20260916T031500Z` — and nothing
    /// else. `R12` needs it because a backup that died before its manifest was written has
    /// nothing else left to say when it was taken, and "some time, unknown" is not what a
    /// listing should print beside a directory that is about to be offered for deletion.
    ///
    /// **Validated by round-trip.** Every candidate is turned back into a path and compared
    /// with what came in, so `20260231T000000Z` and `20261332T000000Z` are refused without
    /// a calendar's worth of range checks: there is one calendar in this module and this is
    /// it, asked the other way round.
    #[must_use]
    pub fn from_utc_path(name: &str) -> Option<Self> {
        let bytes = name.as_bytes();
        if bytes.len() != 16 || bytes[8] != b'T' || bytes[15] != b'Z' {
            return None;
        }

        // `parse` alone would take `+3` as three. A backup directory is machine-written and
        // every one of these positions is a digit or the name is not one of ours.
        let digits = |range: std::ops::Range<usize>| -> Option<i64> {
            let text = name.get(range)?;
            text.bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| text.parse().ok())
                .flatten()
        };

        let year = digits(0..4)?;
        let month = u32::try_from(digits(4..6)?).ok()?;
        let day = u32::try_from(digits(6..8)?).ok()?;
        let hour = digits(9..11)?;
        let minute = digits(11..13)?;
        let second = digits(13..15)?;

        let stamp = Self {
            seconds: days_from_civil(year, month, day) * 86_400
                + hour * 3_600
                + minute * 60
                + second,
        };

        (stamp.utc_path() == name).then_some(stamp)
    }

    /// `2026-09-16T03:15:00Z` — the same moment, for a file rather than a directory name.
    ///
    /// Full ISO 8601 with its separators, because a manifest is read by people and by
    /// other programs, and neither of them should have to know that the compact form in
    /// the path is compact because Windows will not take a colon in a filename.
    #[must_use]
    pub fn utc_iso(self) -> String {
        let (year, month, day, hour, minute, second) = self.parts();
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
    }

    /// This moment where the person running sloop actually is.
    #[must_use]
    pub fn local(self) -> Local {
        Local {
            stamp: self,
            offset: offset_seconds_at(self),
        }
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
    /// Still UTC, and it says so, so that nothing is ever quietly presented as somebody's
    /// wall clock. [`Local::readable`] is what a display uses.
    ///
    /// **Nothing in the binary calls this today.** `db drop` did, until the owner decided it
    /// keeps nothing; the tests still do, and they are the reason it stays — one of them
    /// checks the local rendering against this one, which is the only way to tell that the
    /// two agree about the moment and disagree about the clock.
    #[cfg_attr(not(test), allow(dead_code))]
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

/// A moment, and how far the clock where it happened was from UTC.
///
/// The offset travels with the moment rather than being looked up again at display time.
/// That is what makes a manifest readable on another machine in another country: the
/// backup still says what the clock said where it was taken, which is the thing somebody
/// is trying to match against when they go looking for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    stamp: Stamp,
    /// Seconds to add to UTC. `+05:30` is `19_800`; `-04:00` is `-14_400`.
    offset: i32,
}

impl Local {
    /// A moment at an offset that is already known — reading a manifest back, or a test
    /// that has to check a timezone this machine is not in.
    #[must_use]
    pub const fn at(stamp: Stamp, offset_seconds: i32) -> Self {
        Self {
            stamp,
            offset: offset_seconds,
        }
    }

    /// Seconds to add to UTC to get this clock.
    #[must_use]
    pub const fn offset_seconds(self) -> i32 {
        self.offset
    }

    /// `+05:30`, `-04:00`, `+00:00`.
    ///
    /// Always signed and always padded, because a column of times is read by eye and
    /// `+5:30` beside `-04:00` is a column that has to be parsed rather than scanned.
    #[must_use]
    pub fn offset_label(self) -> String {
        let sign = if self.offset < 0 { '-' } else { '+' };
        let total = self.offset.unsigned_abs();
        format!("{sign}{:02}:{:02}", total / 3_600, (total % 3_600) / 60)
    }

    /// `2026-09-16 08:45:00 +05:30` — the line a person reads.
    #[must_use]
    pub fn readable(self) -> String {
        let (year, month, day, hour, minute, second) = self.shifted().parts();
        format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} {}",
            self.offset_label()
        )
    }

    /// `2026-09-16T08:45:00+05:30` — the same, for the manifest.
    #[must_use]
    pub fn iso(self) -> String {
        let (year, month, day, hour, minute, second) = self.shifted().parts();
        format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{}",
            self.offset_label()
        )
    }

    /// The wall clock, as a UTC stamp that happens to read as the local one.
    ///
    /// The whole of the local rendering: move the instant by the offset and then use the
    /// calendar arithmetic that is already tested. There is no second date implementation
    /// in this project, which is the point.
    fn shifted(self) -> Stamp {
        Stamp {
            seconds: self.stamp.seconds + i64::from(self.offset),
        }
    }
}

/// How far this machine's clock was from UTC at that moment.
///
/// **The one thing in this module that is a lookup rather than a calculation**, and the
/// only reason `chrono` is in the dependency graph. It is asked about the moment in
/// question rather than about now, so a backup taken under summer time reads back under
/// summer time for ever.
///
/// A machine whose timezone cannot be determined at all is treated as UTC. That is
/// `chrono`'s own fallback and the right one here: a backup with an offset of `+00:00`
/// is still a backup, where refusing to take one over a missing `/etc/localtime` would be
/// the tool failing at its job to protect a label.
fn offset_seconds_at(stamp: Stamp) -> i32 {
    use chrono::{Offset as _, TimeZone as _};

    chrono::DateTime::from_timestamp(stamp.seconds, 0).map_or(0, |moment| {
        chrono::Local
            .offset_from_utc_datetime(&moment.naive_utc())
            .fix()
            .local_minus_utc()
    })
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

/// Turn a proleptic Gregorian date into days-since-1970.
///
/// Hinnant's `days_from_civil`, the exact inverse of [`civil_from_days`] above and the
/// same shifted era: a four-hundred-year cycle starting on 1 March, so the leap day lands
/// at the end of a cycle and February needs no special case. Kept beside its inverse
/// rather than anywhere else, because the two share one constant — `719_468`, the days
/// from 0000-03-01 to 1970-01-01 — and a copy of that number elsewhere is a bug waiting to
/// be written.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);

    // January and February belong to the previous year of the shifted era.
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let month_prime = if month > 2 { month - 3 } else { month + 9 }; // March is 0
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}
