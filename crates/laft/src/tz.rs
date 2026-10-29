//! The local time zone offset, read from the TZif file of the zone.

use std::fs;

/// The offset from UTC in seconds of the local time zone at `secs` after
/// the epoch: from the zone `$TZ` names, else `/etc/localtime`. UTC when
/// neither can be read.
pub(crate) fn offset_at(secs: u64) -> i64 {
    let path = match std::env::var("TZ") {
        Ok(tz) if tz.is_empty() || tz == "UTC" || tz == ":UTC" => return 0,
        Ok(tz) => {
            let name = tz.trim_start_matches(':');
            if name.starts_with('/') {
                name.to_owned()
            } else {
                format!("/usr/share/zoneinfo/{name}")
            }
        }
        Err(_) => "/etc/localtime".to_owned(),
    };
    fs::read(path)
        .ok()
        .and_then(|bytes| offset_in(&bytes, i64::try_from(secs).ok()?))
        .unwrap_or(0)
}

/// The offset in effect at `time` according to the TZif data `bytes`, or
/// `None` when they are not TZif data.
pub(crate) fn offset_in(bytes: &[u8], time: i64) -> Option<i64> {
    let header = Header::read(bytes)?;
    let (header, data, wide) = if header.version >= b'2' {
        let rest = bytes.get(44 + header.v1_len()..)?;
        (Header::read(rest)?, rest.get(44..)?, true)
    } else {
        (header, bytes.get(44..)?, false)
    };
    let time_len = if wide { 8 } else { 4 };
    let times = data.get(..header.timecnt * time_len)?;
    let types = data.get(times.len()..times.len() + header.timecnt)?;
    let infos_at = times.len() + types.len();
    let infos = data.get(infos_at..infos_at + header.typecnt * 6)?;
    let info = |index: usize| -> Option<i64> {
        let raw = infos.get(index * 6..index * 6 + 4)?;
        Some(i32::from_be_bytes(raw.try_into().ok()?).into())
    };
    let transition = |i: usize| -> Option<i64> {
        let raw = times.get(i * time_len..(i + 1) * time_len)?;
        Some(if wide {
            i64::from_be_bytes(raw.try_into().ok()?)
        } else {
            i32::from_be_bytes(raw.try_into().ok()?).into()
        })
    };
    let mut current = None;
    for i in 0..header.timecnt {
        if transition(i)? > time {
            break;
        }
        current = Some(usize::from(*types.get(i)?));
    }
    info(current.unwrap_or(0))
}

struct Header {
    version: u8,
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

impl Header {
    fn read(bytes: &[u8]) -> Option<Self> {
        if bytes.get(..4)? != b"TZif" {
            return None;
        }
        let count = |i: usize| -> Option<usize> {
            let raw = bytes.get(20 + i * 4..24 + i * 4)?;
            usize::try_from(u32::from_be_bytes(raw.try_into().ok()?)).ok()
        };
        Some(Self {
            version: *bytes.get(4)?,
            isutcnt: count(0)?,
            isstdcnt: count(1)?,
            leapcnt: count(2)?,
            timecnt: count(3)?,
            typecnt: count(4)?,
            charcnt: count(5)?,
        })
    }

    /// The length of the version 1 data block after this header.
    fn v1_len(&self) -> usize {
        self.timecnt * 5
            + self.typecnt * 6
            + self.charcnt
            + self.leapcnt * 8
            + self.isstdcnt
            + self.isutcnt
    }
}

#[cfg(test)]
#[path = "tz_tests.rs"]
mod tests;
