//! Builders shared by the unit tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use laft_core::Blake3Hash;

use crate::{Arch, Entry, FORMAT, Meta};

pub(crate) fn meta(installed_size: u64) -> Meta {
    Meta {
        format: FORMAT,
        name: "ripgrep".parse().expect("valid name"),
        version: "14.1.1".parse().expect("valid version"),
        rev: 1,
        variant: Default::default(),
        arch: Arch::X86_64V3,
        origin: "ripgrep".parse().expect("valid name"),
        summary: "Recursively search directories for a regex pattern".into(),
        homepage: Some("https://github.com/BurntSushi/ripgrep".into()),
        license: vec!["MIT".into(), "Unlicense".into()],
        depends: vec![
            "so:libc.so.6".parse().expect("valid dependency"),
            "so:libgcc_s.so.1".parse().expect("valid dependency"),
        ],
        provides: vec![],
        conflicts: vec![],
        replaces: vec![],
        alternatives: vec![],
        triggers: vec![],
        static_deps: vec![],
        abi: Default::default(),
        abi_epoch: 0,
        requested_privileges: vec![],
        installed_size,
    }
}

pub(crate) fn dir(path: &str) -> Entry {
    Entry::Dir {
        path: path.into(),
        mode: 0o755,
        owner: None,
        group: None,
    }
}

pub(crate) fn file(path: &str, content: &[u8]) -> Entry {
    Entry::File {
        path: path.into(),
        mode: 0o444,
        size: content.len() as u64,
        hash: Blake3Hash::of(content),
        owner: None,
        group: None,
    }
}

pub(crate) fn symlink(path: &str, target: &str) -> Entry {
    Entry::Symlink {
        path: path.into(),
        target: target.into(),
    }
}

/// Writes or replaces a file in a payload directory, creating its parents,
/// with the given mode.
pub(crate) fn put(root: &Path, path: &str, content: &[u8], mode: u32) {
    let full = root.join(path);
    fs::create_dir_all(full.parent().expect("payload paths have parents"))
        .expect("can create payload directories");
    if full.exists() {
        fs::remove_file(&full).expect("can replace payload file");
    }
    fs::write(&full, content).expect("can write payload file");
    fs::set_permissions(&full, fs::Permissions::from_mode(mode)).expect("can chmod payload file");
}

/// A payload with directories, files, a duplicate file, a symlink and a
/// path longer than a ustar name.
pub(crate) fn sample_payload(root: &Path) {
    put(root, "usr/bin/rg", b"#!/bin/sh\necho rg\n", 0o555);
    put(root, "usr/share/doc/ripgrep/README", b"ripgrep\n", 0o444);
    put(root, "usr/share/doc/ripgrep/COPYING", b"ripgrep\n", 0o444);
    put(root, "etc/ripgreprc", b"smart-case\n", 0o444);
    let long = format!("usr/share/{}/deep", "d".repeat(120));
    put(root, &long, b"deep\n", 0o444);
    std::os::unix::fs::symlink("rg", root.join("usr/bin/ripgrep")).expect("can create symlink");
    for dir in ["usr", "usr/bin", "usr/share", "etc"] {
        fs::set_permissions(root.join(dir), fs::Permissions::from_mode(0o755))
            .expect("can chmod payload directory");
    }
}
