use std::path::PathBuf;

use laft_core::Blake3Hash;
use laft_pkg::{Arch, Compression, Manifest, Meta, scan, write_package};

use super::*;
use crate::test_support::{link, mode, write_tree};

/// Writes the package `name` of the payload directory `payload` into `dir`
/// and returns its file.
fn package(dir: &Path, name: &str, payload: &Path) -> PathBuf {
    let entries = scan(payload).unwrap();
    let installed_size = entries
        .iter()
        .map(|e| match e {
            laft_pkg::Entry::File { size, .. } => *size,
            _ => 0,
        })
        .sum();
    let meta = Meta {
        format: laft_pkg::FORMAT,
        name: name.parse().unwrap(),
        version: "1.0".parse().unwrap(),
        rev: 1,
        variant: Default::default(),
        arch: Arch::X86_64V3,
        origin: name.parse().unwrap(),
        summary: "A test package".into(),
        homepage: None,
        license: vec!["MIT".into()],
        depends: vec![],
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
    };
    let manifest = Manifest::new(
        meta.identity(),
        Blake3Hash::of(&meta.to_json().unwrap()),
        entries,
    )
    .unwrap();
    let file = dir.join(manifest.package().file_name());
    let mut bytes = Vec::new();
    write_package(
        &mut bytes,
        payload,
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .unwrap();
    fs::write(&file, bytes).unwrap();
    file
}

/// A package with `usr/bin/tool`, `usr/lib/libtool.so.1` and the link
/// `usr/lib/libtool.so` to it, written into `dir`.
fn tool_package(dir: &Path) -> PathBuf {
    let payload = dir.join("payload");
    write_tree(
        &payload,
        &[
            ("usr/bin/tool", b"new tool"),
            ("usr/lib/libtool.so.1", b"lib"),
        ],
    );
    link(&payload, "usr/lib/libtool.so", "libtool.so.1");
    for (file, mode) in [("usr/bin/tool", 0o555), ("usr/lib/libtool.so.1", 0o444)] {
        fs::set_permissions(payload.join(file), fs::Permissions::from_mode(mode)).unwrap();
    }
    package(dir, "tool", &payload)
}

#[test]
fn a_package_lands_over_what_the_destination_holds() {
    let dir = laft_test::tempdir();
    let file = tool_package(dir.path());
    let dest = dir.path().join("dest");
    write_tree(
        &dest,
        &[("usr/bin/tool", b"old tool"), ("usr/bin/other", b"kept")],
    );
    fs::set_permissions(dest.join("usr/bin"), fs::Permissions::from_mode(0o750)).unwrap();

    unpack_into(&file, &dest).unwrap();
    assert_eq!(fs::read(dest.join("usr/bin/tool")).unwrap(), b"new tool");
    assert_eq!(mode(&dest.join("usr/bin/tool")), 0o555);
    assert_eq!(fs::read(dest.join("usr/bin/other")).unwrap(), b"kept");
    assert_eq!(
        mode(&dest.join("usr/bin")),
        0o750,
        "an existing directory keeps its mode"
    );
    assert_eq!(fs::read(dest.join("usr/lib/libtool.so")).unwrap(), b"lib");
    assert!(
        !dest.join(STAGING).exists(),
        "the staging directory is removed"
    );

    unpack_into(&file, &dest).expect("unpacking again works");
    assert_eq!(fs::read(dest.join("usr/bin/tool")).unwrap(), b"new tool");
}

#[test]
fn links_in_the_destination_are_replaced_and_never_followed() {
    let dir = laft_test::tempdir();
    let file = tool_package(dir.path());
    let dest = dir.path().join("dest");
    let outside = dir.path().join("outside");
    write_tree(&outside, &[("tool", b"outside")]);
    link(
        &dest,
        "usr/bin/tool",
        &outside.join("tool").to_string_lossy(),
    );

    unpack_into(&file, &dest).unwrap();
    assert_eq!(fs::read(dest.join("usr/bin/tool")).unwrap(), b"new tool");
    assert!(!dest.join("usr/bin/tool").is_symlink());
    assert_eq!(fs::read(outside.join("tool")).unwrap(), b"outside");
}

#[test]
fn a_directory_against_anything_else_is_refused() {
    let dir = laft_test::tempdir();
    let file = tool_package(dir.path());

    let linked = dir.path().join("linked");
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    link(&linked, "usr/lib", &elsewhere.to_string_lossy());
    let error = unpack_into(&file, &linked).expect_err("a linked directory is refused");
    assert_eq!(error.code(), Some("B043"), "{error}");
    assert!(error.to_string().contains("usr/lib"), "{error}");
    assert!(
        fs::read_dir(&elsewhere).unwrap().next().is_none(),
        "nothing is written through the link"
    );

    let filled = dir.path().join("filled");
    fs::create_dir_all(filled.join("usr/bin/tool")).unwrap();
    let error = unpack_into(&file, &filled).expect_err("a directory in the way is refused");
    assert_eq!(error.code(), Some("B043"), "{error}");
}
