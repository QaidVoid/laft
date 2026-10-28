use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;

use super::*;
use crate::test_support::{put, sample_payload};

#[test]
fn lists_every_path_sorted_with_hashes_and_modes() {
    let root = tempfile::tempdir().expect("temporary directory");
    sample_payload(root.path());
    let entries = scan(root.path()).expect("payload scans");
    let paths: Vec<_> = entries.iter().map(Entry::path).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    assert_eq!(paths[0], "etc");
    assert!(entries.contains(&Entry::Symlink {
        path: "usr/bin/ripgrep".into(),
        target: "rg".into()
    }));
    assert!(entries.contains(&Entry::File {
        path: "usr/bin/rg".into(),
        mode: 0o555,
        size: 18,
        hash: Blake3Hash::of(b"#!/bin/sh\necho rg\n"),
        owner: None,
        group: None,
    }));
    assert!(entries.contains(&Entry::Dir {
        path: "usr/bin".into(),
        mode: 0o755,
        owner: None,
        group: None,
    }));
}

#[test]
fn modification_times_do_not_change_the_result() {
    let root = tempfile::tempdir().expect("temporary directory");
    sample_payload(root.path());
    let before = scan(root.path()).expect("payload scans");
    File::open(root.path().join("etc/ripgreprc"))
        .and_then(|f| f.set_modified(std::time::SystemTime::UNIX_EPOCH))
        .expect("can change the modification time");
    assert_eq!(scan(root.path()).expect("payload scans"), before);
}

#[test]
fn sockets_cannot_be_packaged() {
    let root = tempfile::tempdir().expect("temporary directory");
    put(root.path(), "usr/bin/rg", b"rg", 0o555);
    let _listener = std::os::unix::net::UnixListener::bind(root.path().join("usr/sock"))
        .expect("can create a socket");
    let err = scan(root.path()).expect_err("sockets are special files");
    assert_eq!(err.path(), Some("usr/sock"));
}

#[test]
fn non_utf8_paths_and_targets_are_rejected() {
    let root = tempfile::tempdir().expect("temporary directory");
    fs::create_dir(root.path().join("usr")).expect("can create directory");
    let name = std::ffi::OsStr::from_bytes(b"bad\xff");
    fs::write(root.path().join("usr").join(name), b"x").expect("can write file");
    assert!(scan(root.path()).is_err());

    let root = tempfile::tempdir().expect("temporary directory");
    fs::create_dir(root.path().join("usr")).expect("can create directory");
    symlink(
        std::ffi::OsStr::from_bytes(b"bad\xff"),
        root.path().join("usr/link"),
    )
    .expect("can create symlink");
    let err = scan(root.path()).expect_err("target is not UTF-8");
    assert_eq!(err.path(), Some("usr/link"));
}
