use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::test_support::{build, decompress, members, package, put, sample_payload};

#[test]
fn packing_the_same_payload_twice_gives_identical_bytes() {
    let root = laft_test::tempdir();
    sample_payload(root.path());
    let (_, manifest, first) = build(root.path());
    File::open(root.path().join("usr/bin/rg"))
        .and_then(|f| f.set_modified(std::time::SystemTime::UNIX_EPOCH))
        .expect("can change the modification time");
    let (_, again, second) = build(root.path());
    assert_eq!(first, second);
    assert_eq!(manifest.store_id(), again.store_id());
}

#[test]
fn members_follow_the_manifest_order_with_normalized_headers() {
    let root = laft_test::tempdir();
    sample_payload(root.path());
    let (meta, manifest, bytes) = build(root.path());
    let members = members(&decompress(&bytes));
    let paths: Vec<_> = members
        .iter()
        .map(|m| String::from_utf8(m.header.path.clone()).expect("UTF-8 path"))
        .collect();
    let mut expected = vec![META_PATH.to_owned(), MANIFEST_PATH.to_owned()];
    expected.extend(manifest.entries().iter().map(|e| e.path().to_owned()));
    assert_eq!(paths, expected);
    assert_eq!(members[0].content, meta.to_json().expect("valid metadata"));
    assert_eq!(members[1].content, manifest.to_json());
    for (member, entry) in members[2..].iter().zip(manifest.entries()) {
        let h = &member.header;
        assert_eq!((h.uid, h.gid), (0, 0));
        match entry {
            Entry::Dir { mode, .. } => assert_eq!((h.kind, h.mode), (Kind::Dir, *mode)),
            Entry::Symlink { target, .. } => {
                assert_eq!((h.kind, h.mode), (Kind::Symlink, 0o777));
                assert_eq!(h.link, target.as_bytes());
            }
            Entry::File { mode, .. } => assert_eq!(h.mode, *mode),
        }
    }
    let raw = decompress(&bytes);
    let mut at = 0;
    while at < raw.len() && raw[at..at + tar::BLOCK].iter().any(|&b| b != 0) {
        let header =
            tar::decode(raw[at..at + tar::BLOCK].try_into().expect("block")).expect("valid header");
        assert_eq!(&raw[at + 136..at + 148], b"00000000000\0", "mtime is zero");
        assert!(
            raw[at + 265..at + 329].iter().all(|&b| b == 0),
            "names are empty"
        );
        at += tar::BLOCK + header.size as usize + tar::padding(header.size);
    }
}

#[test]
fn identical_files_with_identical_metadata_become_hard_links() {
    let root = laft_test::tempdir();
    put(root.path(), "usr/a", b"same", 0o444);
    put(root.path(), "usr/b", b"same", 0o444);
    put(root.path(), "usr/c", b"same", 0o555);
    fs::set_permissions(root.path().join("usr"), fs::Permissions::from_mode(0o755))
        .expect("can chmod");
    let (_, _, bytes) = build(root.path());
    let members = members(&decompress(&bytes));
    let kinds: Vec<_> = members[3..]
        .iter()
        .map(|m| (m.header.kind, m.header.link.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            (Kind::File, Vec::new()),
            (Kind::HardLink, b"usr/a".to_vec()),
            (Kind::File, Vec::new()),
        ]
    );
}

#[test]
fn payload_changes_after_scanning_name_the_package_and_path() {
    let root = laft_test::tempdir();
    put(root.path(), "usr/a", b"original", 0o444);
    fs::set_permissions(root.path().join("usr"), fs::Permissions::from_mode(0o755))
        .expect("can chmod");
    let (meta, manifest) = package(crate::scan(root.path()).expect("payload scans"));
    put(root.path(), "usr/a", b"modified", 0o444);
    let err = write_package(
        &mut Vec::new(),
        root.path(),
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .expect_err("content changed");
    assert_eq!(err.package(), Some("ripgrep-14.1.1-r1"));
    assert_eq!(err.path(), Some("usr/a"));
    assert!(err.to_string().contains("content hash"), "{err}");

    put(root.path(), "usr/a", b"longer content", 0o444);
    let err = write_package(
        &mut Vec::new(),
        root.path(),
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .expect_err("size changed");
    assert!(err.to_string().contains("changed size"), "{err}");

    fs::remove_file(root.path().join("usr/a")).expect("can remove");
    std::os::unix::fs::symlink("elsewhere", root.path().join("usr/a")).expect("can symlink");
    let err = write_package(
        &mut Vec::new(),
        root.path(),
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .expect_err("type changed");
    assert_eq!(err.path(), Some("usr/a"));
}

#[test]
fn metadata_must_be_the_one_the_manifest_names() {
    let root = laft_test::tempdir();
    sample_payload(root.path());
    let (mut meta, manifest, _) = build(root.path());
    meta.abi_epoch = 1;
    let err = write_package(
        &mut Vec::new(),
        root.path(),
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .expect_err("metadata differs");
    assert!(err.to_string().contains("metadata hash"), "{err}");
}

#[test]
fn compression_sets_the_checksum_flag_and_level() {
    let root = laft_test::tempdir();
    sample_payload(root.path());
    let (meta, manifest, development) = build(root.path());
    let mut publish = Vec::new();
    write_package(
        &mut publish,
        root.path(),
        &meta,
        &manifest,
        Compression::PUBLISH,
    )
    .expect("package writes");
    for bytes in [&development, &publish] {
        assert_eq!(bytes[..4], [0x28, 0xb5, 0x2f, 0xfd]);
        assert_ne!(bytes[4] & 0x04, 0, "content checksum flag");
    }
    assert_ne!(development, publish);
    assert_eq!(decompress(&development), decompress(&publish));
}
