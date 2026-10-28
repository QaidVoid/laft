use std::fs;
use std::path::Path;
use std::time::Duration;

use super::{DeviceSpec, Devices};
use crate::config::Config;
use crate::probe::PROBE_LEN;

fn spec(text: &str) -> DeviceSpec {
    DeviceSpec::parse(text).expect("valid device")
}

/// Announces device `name` of `size` sectors whose node holds `content`.
fn announce(root: &Path, name: &str, size: u64, content: &[u8]) {
    let entry = root.join("sys").join(name);
    fs::create_dir_all(&entry).unwrap();
    fs::write(entry.join("size"), format!("{size}\n")).unwrap();
    fs::create_dir_all(root.join("dev")).unwrap();
    fs::write(root.join("dev").join(name), content).unwrap();
}

fn xfs(uuid: [u8; 16]) -> Vec<u8> {
    let mut head = vec![0u8; PROBE_LEN];
    head[..4].copy_from_slice(b"XFSB");
    head[32..48].copy_from_slice(&uuid);
    head
}

#[test]
fn accepted_device_forms_round_trip() {
    for text in ["UUID=4A2B-91C0", "/dev/nvme0n1p2", "/dev/mapper/cryptroot"] {
        assert_eq!(spec(text).to_string(), text);
    }
    assert_eq!(spec("/dev/sda1"), DeviceSpec::Kernel("sda1".into()));
    assert_eq!(
        spec("/dev/mapper/cryptroot"),
        DeviceSpec::Mapper("cryptroot".into())
    );
}

#[test]
fn other_device_forms_are_rejected() {
    for text in [
        "LABEL=laft",
        "PARTUUID=1234",
        "/dev/disk/by-id/nvme-x",
        "/dev/",
        "/dev/..",
        "/dev/.",
        "/dev/mapper/",
        "/dev/mapper/.",
        "/dev/mapper/..",
        "UUID=",
        "sda1",
    ] {
        assert!(DeviceSpec::parse(text).is_err(), "{text} was accepted");
    }
}

#[test]
fn uuid_is_found_case_insensitively_among_probed_devices() {
    let tmp = tempfile::tempdir().unwrap();
    announce(tmp.path(), "vda", 4096, &[0; 512]);
    announce(tmp.path(), "vda1", 4096, &xfs([0xab; 16]));
    let mut devices = Devices::new(tmp.path().join("sys"), tmp.path().join("dev"));
    let found = devices
        .wait(
            &[spec("UUID=ABABABAB-ABAB-ABAB-ABAB-ABABABABABAB")],
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("the device is present");
    assert_eq!(found, [tmp.path().join("dev/vda1")]);
}

#[test]
fn timeout_names_the_missing_device_and_lists_what_was_seen() {
    let tmp = tempfile::tempdir().unwrap();
    announce(tmp.path(), "vda", 4096, &[0; 512]);
    announce(tmp.path(), "vda1", 4096, &xfs([0x11; 16]));
    let mut devices = Devices::new(tmp.path().join("sys"), tmp.path().join("dev"));
    let err = devices
        .wait(
            &[
                spec("UUID=11111111-1111-1111-1111-111111111111"),
                spec("UUID=0000"),
            ],
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap_err();
    assert_eq!(
        err,
        "timed out after 0s waiting for UUID=0000\n\
         block devices seen:\n  \
         vda          (no filesystem)\n  \
         vda1         xfs    11111111-1111-1111-1111-111111111111"
    );
}

#[test]
fn empty_devices_are_probed_once_they_have_a_size() {
    let tmp = tempfile::tempdir().unwrap();
    announce(tmp.path(), "sr0", 0, &[]);
    let mut devices = Devices::new(tmp.path().join("sys"), tmp.path().join("dev"));
    devices.refresh();
    assert_eq!(devices.listing(), "block devices seen: none");
    announce(tmp.path(), "sr0", 2048, &xfs([0x22; 16]));
    devices.refresh();
    assert!(
        devices
            .find(&spec("UUID=22222222-2222-2222-2222-222222222222"))
            .is_some()
    );
}

#[test]
fn kernel_names_must_be_block_devices() {
    let tmp = tempfile::tempdir().unwrap();
    announce(tmp.path(), "sda1", 4096, &[0; 512]);
    let devices = Devices::new(tmp.path().join("sys"), tmp.path().join("dev"));
    assert_eq!(devices.find(&spec("/dev/sda1")), None);
    assert_eq!(devices.find(&spec("/dev/mapper/cryptroot")), None);
}

#[test]
fn each_luks_device_is_found_through_its_own_spec() {
    let tmp = tempfile::tempdir().unwrap();
    announce(tmp.path(), "vda1", 4096, &xfs([0x11; 16]));
    announce(tmp.path(), "vdb1", 4096, &xfs([0x22; 16]));
    announce(tmp.path(), "vdc1", 4096, &xfs([0x33; 16]));
    let config = Config::parse(
        br#"{
          "esp": {"device": "UUID=4A2B-91C0", "options": "", "type": "vfat"},
          "laft": {"device": "/dev/mapper/cryptlaft", "options": "", "type": "ext4"},
          "luks": [
            {"device": "UUID=33333333-3333-3333-3333-333333333333", "name": "cryptlaft", "options": []},
            {"device": "UUID=22222222-2222-2222-2222-222222222222", "name": "cryptswap", "options": []}
          ],
          "modules": [],
          "release": "6.17.5-laft1",
          "root": {"device": "UUID=11111111-1111-1111-1111-111111111111", "options": "", "type": "xfs"},
          "root_wait_secs": 0,
          "shell": true,
          "tries": 3,
          "version": 1
        }"#,
        "6.17.5-laft1",
    )
    .expect("valid configuration");
    let mut devices = Devices::new(tmp.path().join("sys"), tmp.path().join("dev"));
    let found = devices
        .wait_for_unlock(&config, Duration::ZERO, Duration::ZERO)
        .expect("every device is present");
    let pairs: Vec<_> = found
        .iter()
        .map(|(luks, node)| (luks.name.as_str(), node.clone()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("cryptlaft", tmp.path().join("dev/vdc1")),
            ("cryptswap", tmp.path().join("dev/vdb1")),
        ]
    );
}

#[test]
fn unlocking_waits_for_the_root_too() {
    let tmp = tempfile::tempdir().unwrap();
    announce(tmp.path(), "vdb1", 4096, &xfs([0x22; 16]));
    let config = Config::parse(
        br#"{
          "esp": {"device": "UUID=4A2B-91C0", "options": "", "type": "vfat"},
          "laft": null,
          "luks": [{"device": "UUID=22222222-2222-2222-2222-222222222222", "name": "cryptswap", "options": []}],
          "modules": [],
          "release": "6.17.5-laft1",
          "root": {"device": "UUID=11111111-1111-1111-1111-111111111111", "options": "", "type": "xfs"},
          "root_wait_secs": 0,
          "shell": true,
          "tries": 3,
          "version": 1
        }"#,
        "6.17.5-laft1",
    )
    .expect("valid configuration");
    let mut devices = Devices::new(tmp.path().join("sys"), tmp.path().join("dev"));
    let err = devices
        .wait_for_unlock(&config, Duration::ZERO, Duration::ZERO)
        .unwrap_err();
    assert!(
        err.starts_with("timed out after 0s waiting for UUID=11111111-1111-1111-1111-111111111111"),
        "{err}"
    );
}
