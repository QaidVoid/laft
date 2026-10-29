use std::fs;
use std::path::Path;
use std::time::Duration;

use laft_core::DeviceSpec;

use super::{Devices, boot_spec};
use crate::probe::PROBE_LEN;

fn spec(text: &str) -> DeviceSpec {
    boot_spec(text).expect("valid device")
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
fn the_initramfs_reads_only_the_forms_it_resolves() {
    for text in ["UUID=4A2B-91C0", "/dev/nvme0n1p2", "/dev/mapper/cryptroot"] {
        assert_eq!(spec(text).to_string(), text);
    }
    for text in [
        "LABEL=laft",
        "PARTUUID=1234",
        "PARTLABEL=root",
        "/dev/disk/by-id/nvme-x",
        "/dev/..",
        "/dev/mapper/",
        "UUID=",
        "sda1",
    ] {
        assert!(boot_spec(text).is_err(), "{text} was accepted");
    }
}

#[test]
fn uuid_is_found_case_insensitively_among_probed_devices() {
    let tmp = laft_test::tempdir();
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
    let tmp = laft_test::tempdir();
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
    let tmp = laft_test::tempdir();
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
    let tmp = laft_test::tempdir();
    announce(tmp.path(), "sda1", 4096, &[0; 512]);
    let devices = Devices::new(tmp.path().join("sys"), tmp.path().join("dev"));
    assert_eq!(devices.find(&spec("/dev/sda1")), None);
    assert_eq!(devices.find(&spec("/dev/mapper/cryptroot")), None);
}

#[test]
fn waiting_returns_the_nodes_in_the_order_of_the_specs() {
    let tmp = laft_test::tempdir();
    announce(tmp.path(), "vda1", 4096, &xfs([0x11; 16]));
    announce(tmp.path(), "vdb1", 4096, &xfs([0x22; 16]));
    announce(tmp.path(), "vdc1", 4096, &xfs([0x33; 16]));
    let mut devices = Devices::new(tmp.path().join("sys"), tmp.path().join("dev"));
    let specs = [
        spec("UUID=33333333-3333-3333-3333-333333333333"),
        spec("UUID=22222222-2222-2222-2222-222222222222"),
        spec("UUID=11111111-1111-1111-1111-111111111111"),
    ];
    let found = devices
        .wait(&specs, Duration::ZERO, Duration::ZERO)
        .expect("every device is present");
    assert_eq!(
        found,
        ["vdc1", "vdb1", "vda1"].map(|n| tmp.path().join("dev").join(n))
    );
    let err = devices
        .wait(
            &[spec("UUID=44444444-4444-4444-4444-444444444444")],
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap_err();
    assert!(
        err.starts_with("timed out after 0s waiting for UUID=44444444"),
        "{err}"
    );
}
