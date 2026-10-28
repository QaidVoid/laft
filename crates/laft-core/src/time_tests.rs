use std::time::{Duration, SystemTime, UNIX_EPOCH};

use proptest::prelude::*;

use super::Timestamp;

#[test]
fn known_instants_format_as_rfc3339_utc() {
    let text = |secs| Timestamp::from_secs(secs).to_string();
    assert_eq!(text(0), "1970-01-01T00:00:00Z");
    assert_eq!(text(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(text(4_102_444_799), "2099-12-31T23:59:59Z");
}

#[test]
fn malformed_timestamps_are_rejected() {
    for text in [
        "",
        "2026-10-28",
        "2026-10-28T12:00:00",
        "2026-10-28 12:00:00Z",
        "2026-02-30T00:00:00Z",
        "2026-13-01T00:00:00Z",
        "2026-10-28T24:00:00Z",
        "1969-12-31T23:59:59Z",
        "+026-10-28T12:00:00Z",
        "2026-10-28T12:00:00+00:00",
    ] {
        assert!(text.parse::<Timestamp>().is_err(), "{text:?}");
    }
}

#[test]
fn system_times_truncate_to_whole_seconds() {
    let time = UNIX_EPOCH + Duration::from_millis(1_500);
    assert_eq!(Timestamp::from(time).as_secs(), 1);
    assert_eq!(
        SystemTime::from(Timestamp::from_secs(1)),
        UNIX_EPOCH + Duration::from_secs(1)
    );
    assert_eq!(
        Timestamp::from(UNIX_EPOCH - Duration::from_secs(5)).as_secs(),
        0
    );
}

proptest! {
    #[test]
    fn formatted_timestamps_parse_back(secs in 0u64..253_402_300_800) {
        let t = Timestamp::from_secs(secs);
        prop_assert_eq!(t.to_string().parse::<Timestamp>(), Ok(t));
    }
}
