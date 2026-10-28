//! RFC 3339 timestamps in UTC with second precision, the form every
//! record uses.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY: u64 = 86_400;

/// Formats `time` as `YYYY-MM-DDTHH:MM:SSZ`. Times before the epoch are
/// written as the epoch.
pub(crate) fn format(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (year, month, day) = civil_from_days((secs / DAY) as i64);
    let rest = secs % DAY;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// Parses a timestamp written by [`format`].
pub(crate) fn parse(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    if b.len() != 20 || [b[4], b[7], b[10], b[13], b[16], b[19]] != *b"--T::Z" {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<u64> {
        let digits = s.get(range)?;
        digits
            .bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 59 || year < 1970 {
        return None;
    }
    let days = days_from_civil(year as i64, month as u32, day as u32);
    let secs = u64::try_from(days).ok()? * DAY + hour * 3600 + minute * 60 + second;
    let time = UNIX_EPOCH + Duration::from_secs(secs);
    (format(time) == s).then_some(time)
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
