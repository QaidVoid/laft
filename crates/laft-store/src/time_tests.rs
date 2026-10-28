use std::time::{Duration, UNIX_EPOCH};

use super::format;

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
