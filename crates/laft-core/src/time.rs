use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ParseError;

const DAY: u64 = 86_400;

/// A point in time with second precision, the form every Laft record and
/// signed document uses.
///
/// The text form is RFC 3339 in UTC, `YYYY-MM-DDTHH:MM:SSZ`. Parsing accepts
/// exactly that form for years 1970 to 9999, so every instant has one
/// spelling. Every timestamp lies in that range, from the epoch to
/// [`Self::MAX`], so every timestamp has a text form.
///
/// ```
/// use laft_core::Timestamp;
///
/// let t: Timestamp = "2025-10-28T12:00:00Z".parse()?;
/// assert_eq!(t.as_secs(), 1_761_652_800);
/// assert_eq!(t.to_string(), "2025-10-28T12:00:00Z");
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(u64);

impl Timestamp {
    /// The latest timestamp, `9999-12-31T23:59:59Z`.
    pub const MAX: Self = Self(253_402_300_799);

    /// The instant `secs` seconds after the Unix epoch, or [`Self::MAX`]
    /// when that is later.
    pub const fn from_secs(secs: u64) -> Self {
        if secs > Self::MAX.0 {
            Self::MAX
        } else {
            Self(secs)
        }
    }

    /// The number of seconds since the Unix epoch.
    pub const fn as_secs(self) -> u64 {
        self.0
    }

    /// The current time of the system clock.
    pub fn now() -> Self {
        SystemTime::now().into()
    }
}

impl From<SystemTime> for Timestamp {
    /// Truncates to whole seconds. Times before the epoch become the epoch,
    /// and times after [`Timestamp::MAX`] become [`Timestamp::MAX`].
    fn from(time: SystemTime) -> Self {
        Self::from_secs(time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = civil_from_days((self.0 / DAY) as i64);
        let rest = self.0 % DAY;
        write!(
            f,
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rest / 3600,
            rest / 60 % 60,
            rest % 60
        )
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Timestamp {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || {
            ParseError::new(
                "timestamp",
                s,
                "expected an RFC 3339 UTC time `YYYY-MM-DDTHH:MM:SSZ` from 1970 on",
            )
        };
        let b = s.as_bytes();
        if b.len() != 20 || [b[4], b[7], b[10], b[13], b[16], b[19]] != *b"--T::Z" {
            return Err(err());
        }
        let num = |range: std::ops::Range<usize>| -> Option<u64> {
            let digits = s.get(range)?;
            digits
                .bytes()
                .all(|c| c.is_ascii_digit())
                .then(|| digits.parse().ok())?
        };
        let field = |range| num(range).ok_or_else(err);
        let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
        let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
        if year < 1970 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return Err(err());
        }
        if hour > 23 || minute > 59 || second > 59 {
            return Err(err());
        }
        let days = days_from_civil(year as i64, month as u32, day as u32);
        let days = u64::try_from(days).map_err(|_| err())?;
        let t = Self(days * DAY + hour * 3600 + minute * 60 + second);
        // Days past the end of a month roll over into the next one, so they
        // would give an instant a second spelling.
        if t.to_string() != s {
            return Err(err());
        }
        Ok(t)
    }
}

/// The proleptic Gregorian date of the day `days` after 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The number of days from 1970-01-01 to the given date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
