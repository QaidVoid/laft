use super::{Config, Filesystem, Luks, LuksOption};

const EXAMPLE: &str = r#"{
  "esp": {"device": "UUID=4A2B-91C0", "options": "", "type": "vfat"},
  "laft": null,
  "luks": [{"device": "UUID=6b1d0b3e-6f5c-4a62-9a3f-1f4f0c8b9e21", "name": "cryptroot", "options": ["discard"]}],
  "modules": [
    "/lib/modules/6.17.5-laft1/kernel/drivers/md/dm-mod.ko",
    "/lib/modules/6.17.5-laft1/kernel/drivers/md/dm-crypt.ko"
  ],
  "release": "6.17.5-laft1",
  "root": {"device": "/dev/mapper/cryptroot", "fsck": "/usr/bin/e2fsck", "options": "noatime", "type": "ext4"},
  "root_wait_secs": 30,
  "shell": true,
  "tries": 3,
  "version": 1
}"#;

#[test]
fn example_configuration_is_read() {
    let config = Config::parse(EXAMPLE.as_bytes(), "6.17.5-laft1").expect("valid");
    assert_eq!(
        config.root,
        Filesystem {
            device: "/dev/mapper/cryptroot".into(),
            fsck: Some("/usr/bin/e2fsck".into()),
            options: "noatime".into(),
            fstype: "ext4".into(),
        }
    );
    assert_eq!(config.esp.fsck, None);
    assert_eq!(config.laft, None);
    assert_eq!(
        config.luks,
        [Luks {
            device: "UUID=6b1d0b3e-6f5c-4a62-9a3f-1f4f0c8b9e21".into(),
            name: "cryptroot".into(),
            options: vec![LuksOption::Discard],
        }]
    );
    assert_eq!(config.modules.len(), 2);
    assert_eq!(
        (config.root_wait_secs, config.shell, config.tries),
        (30, true, 3)
    );
}

#[test]
fn configuration_for_another_kernel_is_rejected() {
    let err = Config::parse(EXAMPLE.as_bytes(), "6.18.0-laft1").unwrap_err();
    assert_eq!(
        err,
        "it is for kernel 6.17.5-laft1 but kernel 6.18.0-laft1 is running"
    );
}

#[test]
fn other_versions_are_rejected_before_their_fields_are_read() {
    let err = Config::parse(br#"{"version": 2, "future": true}"#, "6.17.5-laft1").unwrap_err();
    assert_eq!(err, "unsupported version 2");
}

#[test]
fn unknown_fields_and_bad_devices_are_rejected() {
    let unknown = EXAMPLE.replace(r#""shell": true"#, r#""shell": true, "extra": 1"#);
    assert!(Config::parse(unknown.as_bytes(), "6.17.5-laft1").is_err());
    let by_label = EXAMPLE.replace("/dev/mapper/cryptroot", "LABEL=root");
    assert!(Config::parse(by_label.as_bytes(), "6.17.5-laft1").is_err());
    let bad_name = EXAMPLE.replace(r#""name": "cryptroot""#, r#""name": "a/b""#);
    assert!(Config::parse(bad_name.as_bytes(), "6.17.5-laft1").is_err());
    let bad_option = EXAMPLE.replace(r#"["discard"]"#, r#"["trim"]"#);
    assert!(Config::parse(bad_option.as_bytes(), "6.17.5-laft1").is_err());
}

#[test]
fn every_luks_option_is_read() {
    let all = EXAMPLE.replace(
        r#"["discard"]"#,
        r#"["discard", "no-read-workqueue", "no-write-workqueue"]"#,
    );
    let config = Config::parse(all.as_bytes(), "6.17.5-laft1").expect("valid");
    assert_eq!(
        config.luks[0].options,
        [
            LuksOption::Discard,
            LuksOption::NoReadWorkqueue,
            LuksOption::NoWriteWorkqueue
        ]
    );
}
