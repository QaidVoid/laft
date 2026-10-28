use std::time::{Duration, UNIX_EPOCH};

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
        Timestamp::from(UNIX_EPOCH - Duration::from_secs(5)).as_secs(),
        0
    );
}

#[test]
fn instants_after_the_year_9999_become_the_latest_timestamp() {
    assert_eq!(Timestamp::MAX.to_string(), "9999-12-31T23:59:59Z");
    assert_eq!(Timestamp::from_secs(u64::MAX), Timestamp::MAX);
    assert_eq!(
        Timestamp::from_secs(Timestamp::MAX.as_secs() + 1),
        Timestamp::MAX
    );
    let far = UNIX_EPOCH + Duration::from_secs(Timestamp::MAX.as_secs() * 2);
    assert_eq!(Timestamp::from(far), Timestamp::MAX);
    assert_eq!("10000-01-01T00:00:00Z".parse::<Timestamp>().ok(), None);
}

proptest! {
    #[test]
    fn formatted_timestamps_parse_back(secs in any::<u64>()) {
        let t = Timestamp::from_secs(secs);
        prop_assert_eq!(t.to_string().parse::<Timestamp>(), Ok(t));
    }
}
