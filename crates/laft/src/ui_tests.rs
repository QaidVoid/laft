use std::time::Duration;

use super::*;

#[test]
fn sizes_use_binary_units_with_one_decimal() {
    assert_eq!(size(512), "512 B");
    assert_eq!(size(88_290_918), "84.2 MiB");
    assert_eq!(size(3 << 30), "3.0 GiB");
    assert_eq!(size_delta(-2048), "-2.0 KiB");
}

#[test]
fn durations_use_their_two_largest_units() {
    assert_eq!(duration(Duration::from_millis(38)), "38 ms");
    assert_eq!(duration(Duration::from_millis(6800)), "6.8s");
    assert_eq!(duration(Duration::from_secs(8040)), "2h 14m");
    assert_eq!(duration(Duration::from_secs(3 * 86_400 + 7200)), "3d 2h");
}

#[test]
fn tables_align_columns_and_ignore_color_codes() {
    let rows = vec![
        vec![
            "43".into(),
            "\x1b[32m*\x1b[0m d".into(),
            "add ripgrep".into(),
        ],
        vec!["7".into(), "".into(), "".into()],
    ];
    assert_eq!(
        table(&["GEN", "FLAGS", "MESSAGE"], &rows),
        "GEN  FLAGS  MESSAGE\n43   \x1b[32m*\x1b[0m d    add ripgrep\n7\n"
    );
}
