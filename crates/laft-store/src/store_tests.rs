use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use crate::test_support::{Env, Pkg, assert_mode, commit, plan};
use crate::{Error, Transaction};

fn sample(env: &Env) -> crate::test_support::Built {
    Pkg::new("ripgrep", "14.1.1")
        .file("usr/bin/rg", b"#!/bin/sh\necho rg\n")
        .file("usr/share/doc/ripgrep/README", b"ripgrep\n")
        .file("usr/share/doc/ripgrep/COPYING", b"ripgrep\n")
        .file("etc/ripgreprc", b"smart-case\n")
        .symlink("usr/bin/ripgrep", "rg")
        .build(env)
}

fn add(env: &Env, package: &crate::test_support::Built) -> crate::Result<String> {
    let lock = env.lock();
    let mut tx = Transaction::begin(&lock, &plan(&env.root, &[]))?;
    tx.add(&package.path, &package.id)
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for item in fs::read_dir(dir).expect("readable") {
        let path = item.expect("readable").path();
        out.push(path.clone());
        if fs::symlink_metadata(&path).expect("readable").is_dir() {
            walk(&path, out);
        }
    }
}

fn tmp_is_empty(env: &Env) -> bool {
    fs::read_dir(env.root.tmp_dir())
        .expect("the staging directory exists")
        .next()
        .is_none()
}

#[test]
fn added_entry_matches_its_manifest() {
    let env = Env::new();
    let package = sample(&env);
    let dir = add(&env, &package).expect("the package is added");
    assert_eq!(dir, package.dir);
    let store = env.root.store();
    assert_eq!(store.list().expect("lists"), vec![dir.clone()]);
    assert_eq!(store.manifest(&dir).expect("readable"), package.manifest);
    assert_eq!(store.meta(&dir).expect("readable").name.as_str(), "ripgrep");
    assert_eq!(store.verify(&dir, true).expect("verifiable"), vec![]);

    let entry = store.path(&dir);
    assert_mode(&entry, 0o555);
    assert_mode(&entry.join("usr/bin/rg"), 0o555);
    assert_mode(&entry.join("usr/share/doc/ripgrep/README"), 0o444);
    assert_mode(&entry.join(".laft/manifest.json"), 0o444);
    assert_eq!(
        fs::read(entry.join(".laft/manifest.json")).expect("readable"),
        package.manifest.to_json()
    );
    assert_eq!(
        fs::read_link(entry.join("usr/bin/ripgrep")).expect("a symlink"),
        Path::new("rg")
    );
    let mut paths = vec![entry.clone()];
    walk(&entry, &mut paths);
    for path in paths {
        let metadata = fs::symlink_metadata(&path).expect("readable");
        assert_eq!(metadata.mtime(), 1, "{}", path.display());
        if metadata.is_dir() {
            assert_mode(&path, 0o555);
        } else if metadata.is_file() {
            assert_eq!(metadata.mode() & 0o222, 0, "{}", path.display());
        }
    }
    assert!(tmp_is_empty(&env));
}

#[test]
fn adding_an_existing_entry_changes_nothing() {
    let env = Env::new();
    let package = sample(&env);
    commit(&env, &[&package]);
    let dir = package.dir.clone();
    let file = env.root.store().path(&dir).join("usr/bin/rg");
    let inode = fs::metadata(&file).expect("exists").ino();
    let lock = env.lock();
    lock.recover().expect("recovers");
    let mut tx = Transaction::begin(&lock, &plan(&env.root, &[])).expect("begins");
    assert_eq!(tx.add(&package.path, &package.id).expect("added"), dir);
    drop(tx);
    // The interrupted transaction did not add the entry, so recovery keeps it.
    lock.recover().expect("recovers");
    assert_eq!(fs::metadata(&file).expect("exists").ino(), inode);
    assert_eq!(env.root.store().list().expect("lists"), vec![dir]);
}

#[test]
fn mismatched_store_id_leaves_nothing_behind() {
    let env = Env::new();
    let package = sample(&env);
    let other = Pkg::new("other", "1")
        .file("usr/bin/other", b"x")
        .build(&env);
    let lock = env.lock();
    let mut tx = Transaction::begin(&lock, &plan(&env.root, &[])).expect("begins");
    let error = tx
        .add(&package.path, &other.id)
        .expect_err("the id differs");
    assert!(
        matches!(error, Error::IdMismatch { expected, actual } if expected == other.id && actual == package.id)
    );
    assert_eq!(
        env.root.store().list().expect("lists"),
        Vec::<String>::new()
    );
    assert!(tmp_is_empty(&env));
}

#[test]
fn corrupt_package_leaves_nothing_behind() {
    let env = Env::new();
    let package = sample(&env);
    let bytes = fs::read(&package.path).expect("readable");
    fs::write(&package.path, &bytes[..bytes.len() - 40]).expect("writable");
    let error = add(&env, &package).expect_err("the package is truncated");
    assert!(matches!(error, Error::Package(_)), "{error}");
    assert_eq!(
        env.root.store().list().expect("lists"),
        Vec::<String>::new()
    );
    assert!(tmp_is_empty(&env));
}

#[test]
fn verify_reports_changed_and_unlisted_paths() {
    let env = Env::new();
    let package = sample(&env);
    let dir = add(&env, &package).expect("the package is added");
    let store = env.root.store();
    let entry = store.path(&dir);
    let set_mode = |path: &Path, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    };

    let config = entry.join("etc/ripgreprc");
    set_mode(&config, 0o644);
    fs::write(&config, b"SMART-CASE\n").expect("writable");
    let docs = entry.join("usr/share/doc/ripgrep");
    set_mode(&docs, 0o755);
    fs::write(docs.join("extra"), b"x").expect("writable");
    set_mode(&docs, 0o555);

    let problems = |full| -> Vec<String> {
        store
            .verify(&dir, full)
            .expect("verifiable")
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    let quick = problems(false);
    assert_eq!(
        quick,
        vec![
            "etc/ripgreprc: mode is 0o644, the manifest says 0o444".to_owned(),
            "usr/share/doc/ripgrep/extra: is not in the manifest".to_owned(),
        ]
    );
    let full = problems(true);
    assert_eq!(full.len(), 3);
    assert!(full[1].starts_with("etc/ripgreprc: content hash is b3:"));
}

#[test]
fn verify_names_the_document_that_fails_to_load() {
    let env = Env::new();
    let package = sample(&env);
    let dir = add(&env, &package).expect("the package is added");
    let store = env.root.store();
    let meta = store.path(&dir).join(".laft/meta.json");
    fs::set_permissions(&meta, fs::Permissions::from_mode(0o644)).expect("chmod");
    fs::write(&meta, b"{}").expect("writable");
    let problems = store.verify(&dir, false).expect("verifiable");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].path, ".laft/meta.json");

    let manifest = store.path(&dir).join(".laft/manifest.json");
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644)).expect("chmod");
    fs::write(&manifest, b"{}").expect("writable");
    let problems = store.verify(&dir, false).expect("verifiable");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].path, ".laft/manifest.json");
}

#[test]
fn entry_names_cannot_leave_the_store() {
    let env = Env::new();
    let store = env.root.store();
    for name in ["", "..", "../state", "a/b", ".laft"] {
        assert!(
            matches!(store.manifest(name), Err(Error::MissingEntry(_))),
            "{name:?}"
        );
    }
}
