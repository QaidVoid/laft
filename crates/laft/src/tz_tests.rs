use super::*;

/// TZif version 1 data with two types, UTC+1 and UTC+2, and one
/// transition to UTC+2 at `at`.
fn zone(at: i32) -> Vec<u8> {
    let mut out = b"TZif\0".to_vec();
    out.extend_from_slice(&[0; 15]);
    for count in [0u32, 0, 0, 1, 2, 4] {
        out.extend_from_slice(&count.to_be_bytes());
    }
    out.extend_from_slice(&at.to_be_bytes());
    out.push(1);
    for (offset, dst) in [(3600i32, 0u8), (7200, 1)] {
        out.extend_from_slice(&offset.to_be_bytes());
        out.push(dst);
        out.push(0);
    }
    out.extend_from_slice(b"CET\0");
    out
}

#[test]
fn the_offset_follows_the_last_transition_before_the_time() {
    let data = zone(1000);
    assert_eq!(offset_in(&data, 999), Some(3600));
    assert_eq!(offset_in(&data, 1000), Some(7200));
    assert_eq!(offset_in(&data, 5_000_000), Some(7200));
}

#[test]
fn data_that_is_not_tzif_has_no_offset() {
    assert_eq!(offset_in(b"not a zone", 0), None);
    assert_eq!(offset_in(&zone(1000)[..30], 0), None);
}
