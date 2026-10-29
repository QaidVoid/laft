use super::*;

fn spec(text: &str) -> DeviceSpec {
    text.parse().unwrap_or_else(|e| panic!("{text}: {e}"))
}

#[test]
fn every_fstab_form_round_trips() {
    for text in [
        "/dev/nvme0n1p2",
        "/dev/mapper/cryptroot",
        "/dev/disk/by-id/nvme-x-part2",
        "UUID=4A2B-91C0",
        "LABEL=laft",
        "LABEL=données",
        "PARTUUID=0f3e8c1a-02",
        "PARTLABEL=swap",
    ] {
        assert_eq!(spec(text).to_string(), text);
    }
    assert_eq!(spec("LABEL=swap"), DeviceSpec::Label("swap".into()));
    assert_eq!(spec("PARTUUID=ab-01"), DeviceSpec::PartUuid("ab-01".into()));
}

#[test]
fn malformed_names_are_rejected() {
    for text in [
        "",
        "sda1",
        "/",
        "/dev/",
        "/dev/..",
        "/dev/.",
        "/dev//sda",
        "/dev/sda/",
        "/dev/a b",
        "UUID=",
        "LABEL=",
        "PARTUUID=",
        "PARTLABEL=",
        "UUID=a b",
        "UUID=é",
        "LABEL=a\tb",
        "uuid=abc",
    ] {
        assert!(text.parse::<DeviceSpec>().is_err(), "{text:?} was accepted");
    }
}

#[test]
fn the_initramfs_resolves_uuids_kernel_names_and_mapper_names() {
    assert_eq!(spec("/dev/sda1").kernel_name(), Some("sda1"));
    assert_eq!(spec("/dev/mapper/root").mapper_name(), Some("root"));
    assert_eq!(spec("/dev/mapper/root").kernel_name(), None);
    for text in ["UUID=4A2B-91C0", "/dev/sda1", "/dev/mapper/root"] {
        assert!(spec(text).resolved_at_boot(), "{text}");
    }
    for text in [
        "LABEL=laft",
        "PARTUUID=0f3e8c1a-02",
        "PARTLABEL=root",
        "/dev/disk/by-id/nvme-x",
        "/swapfile",
    ] {
        assert!(!spec(text).resolved_at_boot(), "{text}");
    }
}
