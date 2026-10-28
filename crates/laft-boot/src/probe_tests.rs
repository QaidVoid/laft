use super::{PROBE_LEN, Probe, probe};

const UUID: [u8; 16] = [
    0x6b, 0x1d, 0x0b, 0x3e, 0x6f, 0x5c, 0x4a, 0x62, 0x9a, 0x3f, 0x1f, 0x4f, 0x0c, 0x8b, 0x9e, 0x21,
];
const UUID_TEXT: &str = "6b1d0b3e-6f5c-4a62-9a3f-1f4f0c8b9e21";

fn image(fields: &[(usize, &[u8])]) -> Vec<u8> {
    let mut head = vec![0u8; PROBE_LEN];
    for (offset, bytes) in fields {
        head[*offset..*offset + bytes.len()].copy_from_slice(bytes);
    }
    head
}

fn found(kind: &'static str, uuid: &str) -> Option<Probe> {
    Some(Probe {
        kind,
        uuid: uuid.to_owned(),
    })
}

#[test]
fn ext_superblock_gives_type_by_features_and_uuid() {
    let magic = 0xEF53u16.to_le_bytes();
    let ext2 = image(&[(1024 + 0x38, &magic), (1024 + 0x68, &UUID)]);
    assert_eq!(probe(&ext2), found("ext2", UUID_TEXT));
    let ext3 = image(&[
        (1024 + 0x38, &magic),
        (1024 + 0x5C, &4u32.to_le_bytes()),
        (1024 + 0x68, &UUID),
    ]);
    assert_eq!(probe(&ext3), found("ext3", UUID_TEXT));
    let ext4 = image(&[
        (1024 + 0x38, &magic),
        (1024 + 0x5C, &4u32.to_le_bytes()),
        (1024 + 0x60, &0x2C2u32.to_le_bytes()),
        (1024 + 0x68, &UUID),
    ]);
    assert_eq!(probe(&ext4), found("ext4", UUID_TEXT));
}

#[test]
fn xfs_superblock_gives_uuid() {
    let head = image(&[(0, b"XFSB"), (32, &UUID)]);
    assert_eq!(probe(&head), found("xfs", UUID_TEXT));
}

#[test]
fn btrfs_superblock_gives_fsid() {
    let head = image(&[(0x10040, b"_BHRfS_M"), (0x10020, &UUID)]);
    assert_eq!(probe(&head), found("btrfs", UUID_TEXT));
}

#[test]
fn f2fs_superblock_gives_uuid() {
    let head = image(&[(1024, &0xF2F5_2010u32.to_le_bytes()), (1024 + 0x6C, &UUID)]);
    assert_eq!(probe(&head), found("f2fs", UUID_TEXT));
}

#[test]
fn fat32_serial_is_written_high_word_first() {
    let head = image(&[(0x52, b"FAT32   "), (0x43, &[0xC0, 0x91, 0x2B, 0x4A])]);
    assert_eq!(probe(&head), found("vfat", "4A2B-91C0"));
}

#[test]
fn fat16_serial_is_read_from_the_older_boot_sector() {
    let head = image(&[(0x36, b"FAT16   "), (0x27, &[0x0d, 0xf0, 0xad, 0x0b])]);
    assert_eq!(probe(&head), found("vfat", "0BAD-F00D"));
}

#[test]
fn luks_headers_give_version_and_stored_uuid() {
    let uuid = b"6b1d0b3e-6f5c-4a62-9a3f-1f4f0c8b9e20\0\0\0\0";
    let luks1 = image(&[(0, b"LUKS\xba\xbe\x00\x01"), (168, uuid)]);
    assert_eq!(
        probe(&luks1),
        found("luks1", "6b1d0b3e-6f5c-4a62-9a3f-1f4f0c8b9e20")
    );
    let luks2 = image(&[(0, b"LUKS\xba\xbe\x00\x02"), (168, uuid)]);
    assert_eq!(
        probe(&luks2),
        found("luks2", "6b1d0b3e-6f5c-4a62-9a3f-1f4f0c8b9e20")
    );
}

#[test]
fn unknown_or_short_devices_give_nothing() {
    assert_eq!(probe(&image(&[])), None);
    assert_eq!(probe(&[]), None);
    let cut = image(&[(1024 + 0x38, &0xEF53u16.to_le_bytes())]);
    assert_eq!(probe(&cut[..1024 + 0x60]), None);
}

#[test]
fn unknown_luks_version_is_not_luks() {
    let head = image(&[(0, b"LUKS\xba\xbe\x00\x07")]);
    assert_eq!(probe(&head), None);
}
