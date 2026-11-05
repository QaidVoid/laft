use super::*;
use laft_core::Variant;
use std::collections::BTreeMap;

use std::fs;

use laft_core::{Blake3Hash, OptionValue};
use laft_index::{Index, PublicKey, Signature};
use laft_pkg::{Compression, Manifest, Meta, scan, write_package};

/// The metadata of one test package.
fn meta(name: &str) -> Meta {
    Meta {
        format: laft_pkg::FORMAT,
        name: name.parse().expect("a name"),
        version: "1.0".parse().expect("a version"),
        rev: 1,
        variant: Variant::default(),
        arch: ARCH,
        origin: name.parse().expect("a name"),
        summary: format!("The {name} test package"),
        homepage: None,
        license: vec!["MIT".to_owned()],
        depends: Vec::new(),
        provides: Vec::new(),
        conflicts: Vec::new(),
        replaces: Vec::new(),
        alternatives: Vec::new(),
        triggers: Vec::new(),
        static_deps: Vec::new(),
        abi: BTreeMap::new(),
        abi_epoch: 0,
        options: BTreeMap::from([("llvm".to_owned(), OptionValue::Bool(false))]),
        requested_privileges: Vec::new(),
        installed_size: 0,
    }
}

/// Writes the package of `meta`, with a buildinfo next to it, into `out`.
fn write_package_file(meta: &Meta, out: &Path) -> (PathBuf, PathBuf, laft_core::StoreId) {
    let payload = laft_test::tempdir();
    let path = "usr/bin/laft";
    let file = payload.path().join(path);
    fs::create_dir_all(file.parent().expect("a parent")).expect("writable");
    fs::write(&file, b"binary").expect("writable");
    let executable = path.starts_with("usr/bin/") || path.starts_with("usr/lib/laft/");
    let mode = if executable { 0o555 } else { 0o444 };
    fs::set_permissions(&file, fs::Permissions::from_mode(mode)).expect("chmod");
    let entries = scan(payload.path()).expect("the payload scans");
    let installed_size = entries
        .iter()
        .map(|e| match e {
            laft_pkg::Entry::File { size, .. } => *size,
            _ => 0,
        })
        .sum();
    let meta = Meta {
        installed_size,
        ..meta.clone()
    };
    let meta_json = meta.to_json().expect("valid metadata");
    let manifest = Manifest::new(meta.identity(), Blake3Hash::of(&meta_json), entries)
        .expect("valid manifest");
    let identity = meta.identity();
    let path = out.join(identity.file_name());
    let mut bytes = Vec::new();
    write_package(
        &mut bytes,
        payload.path(),
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .expect("the package writes");
    fs::write(&path, &bytes).expect("writable");
    let buildinfo = out.join(format!("{}.buildinfo.json", identity.file_name()));
    let info = serde_json::json!({
        "format": 1,
        "package": {"name": meta.name, "version": meta.version, "rev": meta.rev,
            "variant": meta.variant},
        "recipe_repos": [{"name": "core", "commit": "4be1", "dirty": false}],
        "outputs": [{"name": meta.name, "variant": meta.variant,
            "id": manifest.store_id(), "file": identity.file_name()}],
    });
    fs::write(&buildinfo, serde_json::to_vec(&info).expect("serializes")).expect("writable");
    (path, buildinfo, manifest.store_id())
}

#[test]
fn a_local_key_is_generated_once_and_stays_secret() {
    let dir = laft_test::tempdir();
    let local = Local::of(&dir.path().join("root"));
    let first = local.public_key().expect("a key");
    let secret = dir.path().join("root/laft/state/local.key");
    let mode = fs::metadata(&secret).expect("the key exists").permissions();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(mode.mode() & 0o777, 0o600);
    let second = local.public_key().expect("a key");
    assert_eq!(first, second);
    let public = dir.path().join("root/laft/state/local.key.pub");
    let text = fs::read_to_string(&public).expect("the public key exists");
    let line = text.lines().last().expect("a key line").trim();
    let listed: PublicKey = line.parse().expect("a public key");
    assert_eq!(listed, first);
}

#[test]
fn an_added_package_is_listed_by_a_signed_index() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, _buildinfo, id) = write_package_file(&meta("mesa"), &out);
    let local = Local::of(&dir.path().join("root"));
    local
        .add(std::slice::from_ref(&file))
        .expect("the package is added");
    let index = local.index().expect("readable").expect("an index");
    assert_eq!(index.packages.len(), 1);
    let entry = &index.packages[0];
    assert_eq!(entry.id, id);
    assert_eq!(entry.size, fs::metadata(&file).expect("readable").len());
    assert_eq!(
        entry.blake3,
        laft_core::Blake3Hash::of(&fs::read(&file).expect("readable"))
    );
    let pool = dir.path().join(format!("root/laft/local/pool/{}.lpkg", id));
    assert!(pool.is_file(), "{} is missing", pool.display());
    let info = dir
        .path()
        .join(format!("root/laft/local/buildinfo/{id}.json"));
    assert!(info.is_file(), "{} is missing", info.display());
    let bytes = fs::read(dir.path().join(format!(
        "root/laft/local/channels/local/{}/index.json.zst",
        ARCH
    )))
    .expect("the index exists");
    let signature = fs::read_to_string(dir.path().join(format!(
        "root/laft/local/channels/local/{}/index.json.zst.minisig",
        ARCH
    )))
    .expect("the index is signed");
    let decoded = Signature::decode(signature.as_bytes()).expect("a signature");
    assert!(
        local
            .public_key()
            .expect("a key")
            .verifies(&bytes, &decoded),
        "the signature verifies"
    );
    Index::decode(&bytes).expect("a valid index");
}

#[test]
fn adding_a_package_again_adds_nothing() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, _, _) = write_package_file(&meta("mesa"), &out);
    let local = Local::of(&dir.path().join("root"));
    assert_eq!(local.add(std::slice::from_ref(&file)).expect("added"), 1);
    assert_eq!(
        local.add(std::slice::from_ref(&file)).expect("added again"),
        0
    );
    let index = local.index().expect("readable").expect("an index");
    assert_eq!(index.generation, 1);
    assert_eq!(index.packages.len(), 1);
    let snapshots = fs::read_dir(
        dir.path()
            .join(format!("root/laft/local/channels/local/{}/snapshots", ARCH)),
    )
    .expect("readable")
    .filter_map(Result::ok)
    .filter(|e| e.file_name().to_string_lossy().ends_with(".json.zst"))
    .count();
    assert_eq!(snapshots, 1);
}

#[test]
fn a_corrupt_package_is_refused_and_nothing_is_added() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, _, _) = write_package_file(&meta("mesa"), &out);
    let mut bytes = fs::read(&file).expect("readable");
    let cut = bytes.len() / 2;
    bytes.truncate(cut);
    fs::write(&file, &bytes).expect("writable");
    let local = Local::of(&dir.path().join("root"));
    let error = local
        .add(&[file])
        .expect_err("a corrupt package is refused");
    assert!(error.message.contains("mesa"), "{}", error.message);
    assert!(local.index().expect("readable").is_none());
}
