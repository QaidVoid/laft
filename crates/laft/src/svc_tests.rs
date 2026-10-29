use super::*;

#[test]
fn the_runsv_status_record_gives_the_pid_the_wish_and_the_time() {
    let mut bytes = vec![0u8; 20];
    let tai: u64 = 4_611_686_018_427_387_914 + 1_761_652_800;
    bytes[0..8].copy_from_slice(&tai.to_be_bytes());
    bytes[12..16].copy_from_slice(&412u32.to_le_bytes());
    bytes[17] = b'u';
    assert_eq!(
        parse_status(&bytes),
        Some(Supervised {
            since: 1_761_652_800,
            pid: 412,
            want_up: true
        })
    );
    bytes[17] = b'd';
    assert!(parse_status(&bytes).is_some_and(|s| !s.want_up));
    assert_eq!(parse_status(&bytes[..10]), None);
}

#[test]
fn log_labels_become_times() {
    let label = format!("@{:016x}00000000", 4_611_686_018_427_387_914u64 + 86_400);
    assert_eq!(tai64n(&label), Some(Timestamp::from_secs(86_400)));
    assert_eq!(tai64n("no label"), None);
    assert_eq!(log_line("plain text"), "plain text");
}
