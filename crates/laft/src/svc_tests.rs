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

#[test]
fn a_service_without_a_log_file_says_where_its_output_goes() {
    let mut spec: Spec = serde_json::from_value(json!({
        "spec_version": 1, "name": "boot-complete", "kind": "oneshot", "early": false,
        "exec": [], "up": ["/usr/bin/true"], "down": [], "user": "root", "group": null,
        "groups": [], "env": {}, "cwd": "/", "umask": 18, "after": [],
        "ready": {"type": "immediate"}, "log": {"type": "console"}, "state_dirs": [],
        "runtime_dirs": [], "restart": "always", "restart_delay_ms": 1000, "stop_signal": 15,
        "stop_timeout_ms": 10000,
        "limits": {"nofile": null, "nproc": null, "memlock": null, "core": null},
        "on_change": "ignore"
    }))
    .expect("a spec");
    let console = unlogged(&spec).expect("not logged");
    assert!(console.contains("logs to the console"), "{console}");
    assert!(console.contains("/dev/console"), "{console}");
    spec.log = SpecLog::Discard;
    assert!(unlogged(&spec).is_some_and(|why| why.contains("discards its output")));
    spec.log = SpecLog::Logged {
        dir: "/var/log/boot-complete".into(),
        size: 1,
        count: 1,
    };
    assert_eq!(unlogged(&spec), None);
}
