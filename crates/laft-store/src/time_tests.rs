use std::time::{Duration, UNIX_EPOCH};

use super::{format, parse};

#[test]
fn known_instants_format_as_rfc3339_utc() {
    assert_eq!(format(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(
        format(UNIX_EPOCH + Duration::from_secs(951_782_400)),
        "2000-02-29T00:00:00Z"
    );
    assert_eq!(
        format(UNIX_EPOCH + Duration::from_secs(4_102_444_799)),
        "2099-12-31T23:59:59Z"
    );
}

#[test]
fn formatted_timestamps_parse_back() {
    for secs in [0, 59, 86_399, 951_782_400, 1_761_652_812, 4_102_444_799] {
        let time = UNIX_EPOCH + Duration::from_secs(secs);
        assert_eq!(parse(&format(time)), Some(time), "{secs}");
    }
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
    ] {
        assert_eq!(parse(text), None, "{text:?}");
    }
}
