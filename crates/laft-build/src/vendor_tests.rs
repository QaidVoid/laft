use std::io::Read;

use super::*;
use crate::test_support::write_tree;

fn tree(dir: &Path, modes: u32) -> PathBuf {
    let root = dir.join("vendor");
    let long = format!("deep/{}/file.rs", "x".repeat(150));
    write_tree(
        &root,
        &[
            ("b/lib.rs", b"lib"),
            ("a/build.sh", b"#!/bin/sh"),
            (&long, b"long"),
        ],
    );
    fsx::set_mode(&root.join("a/build.sh"), modes).unwrap();
    std::os::unix::fs::symlink("lib.rs", root.join("b/alias.rs")).unwrap();
    root
}

#[test]
fn the_same_tree_always_gives_the_same_tarball() {
    let (one, two) = (laft_test::tempdir(), laft_test::tempdir());
    let (a, b) = (tree(one.path(), 0o700), tree(two.path(), 0o755));
    filetime_touch(&a.join("b/lib.rs"));
    normalized_tarball(&a, &one.path().join("v.tar")).unwrap();
    normalized_tarball(&b, &two.path().join("v.tar")).unwrap();
    assert_eq!(
        fs::read(one.path().join("v.tar")).unwrap(),
        fs::read(two.path().join("v.tar")).unwrap()
    );
}

/// Moves the modification time of `path` far from the other files.
fn filetime_touch(path: &Path) {
    let file = File::options().append(true).open(path).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000))
        .unwrap();
}

#[test]
fn entries_are_sorted_and_normalized() {
    let dir = laft_test::tempdir();
    let root = tree(dir.path(), 0o700);
    let file = dir.path().join("v.tar");
    normalized_tarball(&root, &file).unwrap();
    let mut archive = tar::Archive::new(File::open(&file).unwrap());
    let mut seen = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let header = entry.header().clone();
        assert_eq!(header.mtime().unwrap(), SOURCE_DATE_EPOCH);
        assert_eq!((header.uid().unwrap(), header.gid().unwrap()), (0, 0));
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        seen.push((path, header.mode().unwrap()));
        let mut sink = Vec::new();
        entry.read_to_end(&mut sink).unwrap();
    }
    let long = format!("deep/{}/file.rs", "x".repeat(150));
    assert_eq!(
        seen,
        [
            ("a/".to_owned(), 0o755),
            ("a/build.sh".to_owned(), 0o755),
            ("b/".to_owned(), 0o755),
            ("b/alias.rs".to_owned(), 0o777),
            ("b/lib.rs".to_owned(), 0o644),
            ("deep/".to_owned(), 0o755),
            (format!("deep/{}/", "x".repeat(150)), 0o755),
            (long, 0o644),
        ]
    );
}
