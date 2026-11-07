use super::*;
use laft_core::{Timestamp, Variant};
use std::collections::{BTreeMap, BTreeSet};

use std::fs;

use laft_core::{Blake3Hash, OptionValue};
use laft_index::{Index, PublicKey, Signature};
use laft_pkg::{Compression, Manifest, Meta, scan, write_package};
use minisign::{SecretKey, SecretKeyBox};

/// The package `file` with its buildinfo, as `add` takes them.
fn package_of(file: &Path, buildinfo: &Path) -> PackageFile {
    PackageFile {
        path: file.to_path_buf(),
        buildinfo: buildinfo.to_path_buf(),
    }
}

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
    let id = manifest.store_id();
    let info = laft_build::Buildinfo {
        format: laft_build::BUILDINFO_FORMAT,
        package: laft_build::BuiltPackage {
            name: meta.name.clone(),
            version: meta.version.clone(),
            rev: meta.rev,
            variant: meta.variant,
        },
        build_key: Blake3Hash::of(b"key"),
        plan_hash: Blake3Hash::of(b"plan"),
        recipe_repos: Vec::new(),
        sources: Vec::new(),
        vendors: Vec::new(),
        patches: Vec::new(),
        toolchain_epoch: 1,
        abi_epoch: 0,
        build_view: Vec::new(),
        env: Default::default(),
        outputs: vec![laft_build::Output {
            name: meta.name.clone(),
            variant: meta.variant,
            id,
            file: identity.file_name(),
        }],
        abi: Default::default(),
        host: laft_build::BuildinfoHost {
            laft_build: "0.1.0".into(),
            kernel: "test".into(),
            jobs: 1,
            started: "2026-01-01T00:00:00Z".parse().expect("a timestamp"),
            finished: "2026-01-01T00:00:01Z".parse().expect("a timestamp"),
        },
        bootstrap: false,
    };
    fs::write(&buildinfo, info.to_json()).expect("writable");
    (path, buildinfo, id)
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
fn a_key_is_written_without_following_a_leftover_temporary_path() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("root");
    let state = root.join("laft/state");
    fs::create_dir_all(&state).expect("writable");
    let victim = state.join("victim");
    fs::write(&victim, b"do not touch").expect("writable");
    std::os::unix::fs::symlink(&victim, state.join(".local.key.laft-tmp")).expect("writable");

    let local = Local::of(&root);
    let key = local.public_key().expect("a key");
    assert_eq!(
        fs::read(&victim).expect("readable"),
        b"do not touch",
        "the temporary path of the key file is not followed"
    );
    let entries: BTreeSet<String> = fs::read_dir(&state)
        .expect("readable")
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_string_lossy().into_owned().parse().ok())
        .collect();
    assert_eq!(
        entries,
        BTreeSet::from([
            ".local.key.laft-tmp".to_owned(),
            "local.key".to_owned(),
            "local.key.pub".to_owned(),
            "victim".to_owned(),
        ]),
        "no temporary file is left behind"
    );
    let boxed = SecretKeyBox::from_string(
        &fs::read_to_string(state.join("local.key")).expect("the key exists"),
    )
    .expect("the key is complete");
    let secret = SecretKey::from_unencrypted_box(boxed).expect("the key is readable");
    let public = minisign::PublicKey::from_secret_key(&secret).expect("a public key");
    let listed: PublicKey = public.to_base64().parse().expect("a listed key");
    assert_eq!(listed, key);
}

#[test]
fn an_index_near_expiry_is_resigned_before_it_expires() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, buildinfo, _) = write_package_file(&meta("mesa"), &out);
    let local = Local::of(&dir.path().join("root"));
    local
        .add(&[package_of(&file, &buildinfo)])
        .expect("the package is added");

    // Rewind the expiry to an hour from now, as if nothing had been added
    // for weeks.
    let entry = local.entry();
    let mut index = Index::decode(&fs::read(&entry).expect("readable")).expect("an index");
    index.expires = Timestamp::from_secs(Timestamp::now().as_secs() + 3_600);
    let stale = index.encode().expect("the index encodes");
    write_new(&entry, &stale).expect("writable");

    let before = fs::read(&entry).expect("readable");
    assert!(local.refresh().expect("the index is re-signed"));
    let bytes = fs::read(&entry).expect("readable");
    assert_ne!(bytes, before, "the index is rewritten");
    let index = Index::decode(&bytes).expect("a valid index");
    assert!(
        index.expires.as_secs() >= Timestamp::now().as_secs() + 86_400,
        "the index is valid again"
    );
    let signature = fs::read_to_string(signed(&entry)).expect("the index is signed");
    let decoded = Signature::decode(signature.as_bytes()).expect("a signature");
    assert!(
        local
            .public_key()
            .expect("a key")
            .verifies(&bytes, &decoded),
        "the fresh signature verifies"
    );

    // An index with validity left is left alone.
    assert!(!local.refresh().expect("readable"));
    assert_eq!(fs::read(&entry).expect("readable"), bytes);
}

#[test]
fn refreshing_a_repository_without_an_index_does_nothing() {
    let dir = laft_test::tempdir();
    let local = Local::of(&dir.path().join("root"));
    assert!(!local.refresh().expect("nothing to refresh"));
}

#[test]
fn an_unreadable_key_is_never_replaced() {
    if rustix::process::getuid().is_root() {
        return;
    }
    let dir = laft_test::tempdir();
    let root = dir.path().join("root");
    let local = Local::of(&root);
    local.public_key().expect("a key");
    let key = root.join("laft/state/local.key");
    let public = root.join("laft/state/local.key.pub");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&key, fs::Permissions::from_mode(0o000)).expect("chmod");
    fs::remove_file(&public).expect("the public key is removed");
    local
        .public_key()
        .expect_err("the key cannot be read, and no key is generated");
    assert!(
        fs::read(&key).is_err(),
        "the unreadable key is still in place"
    );
}

#[test]
fn a_public_key_that_vanished_is_derived_from_the_key() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("root");
    let local = Local::of(&root);
    let first = local.public_key().expect("a key");
    let key = root.join("laft/state/local.key");
    let public = root.join("laft/state/local.key.pub");
    let before = fs::read(&key).expect("readable");
    fs::remove_file(&public).expect("the public key is removed");
    let second = local.public_key().expect("a key");
    assert_eq!(first, second);
    assert_eq!(
        before,
        fs::read(&key).expect("readable"),
        "the key is not rewritten"
    );
}

#[test]
fn an_added_package_is_listed_by_a_signed_index() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, buildinfo, id) = write_package_file(&meta("mesa"), &out);
    let local = Local::of(&dir.path().join("root"));
    local
        .add(&[package_of(&file, &buildinfo)])
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
        "root/laft/local/channels/current/{}/index.json.zst",
        ARCH
    )))
    .expect("the index exists");
    let signature = fs::read_to_string(dir.path().join(format!(
        "root/laft/local/channels/current/{}/index.json.zst.minisig",
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
    let decoded_index = Index::decode(&bytes).expect("a valid index");
    assert_eq!(decoded_index.repo.to_string(), "local");
    assert_eq!(decoded_index.channel.to_string(), "current");
}

#[test]
fn adding_a_package_again_adds_nothing() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, buildinfo, _) = write_package_file(&meta("mesa"), &out);
    let local = Local::of(&dir.path().join("root"));
    assert_eq!(
        local
            .add(&[package_of(&file, &buildinfo)])
            .expect("added")
            .len(),
        1
    );
    assert_eq!(
        local
            .add(&[package_of(&file, &buildinfo)])
            .expect("added again")
            .len(),
        0
    );
    let index = local.index().expect("readable").expect("an index");
    assert_eq!(index.generation, 1);
    assert_eq!(index.packages.len(), 1);
    let snapshots = fs::read_dir(dir.path().join(format!(
        "root/laft/local/channels/current/{}/snapshots",
        ARCH
    )))
    .expect("readable")
    .filter_map(Result::ok)
    .filter(|e| e.file_name().to_string_lossy().ends_with(".json.zst"))
    .count();
    assert_eq!(snapshots, 1);
}

#[test]
fn a_newer_version_of_a_listed_package_is_added() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let older = Meta {
        version: "25.1".parse().expect("a version"),
        ..meta("mesa")
    };
    let newer = Meta {
        version: "25.2".parse().expect("a version"),
        ..meta("mesa")
    };
    let local = Local::of(&dir.path().join("root"));
    for m in [&older, &newer] {
        let (file, buildinfo, _) = write_package_file(m, &out);
        local.add(&[package_of(&file, &buildinfo)]).expect("added");
    }
    let index = local.index().expect("readable").expect("an index");
    assert_eq!(
        index
            .packages
            .iter()
            .map(|e| e.meta.version.to_string())
            .collect::<Vec<_>>(),
        ["25.2", "25.1"],
        "{index:?}"
    );
}

#[test]
fn the_option_sets_of_one_revision_stay_in_the_index_order() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let with_llvm = meta("mesa");
    let default = Meta {
        options: BTreeMap::new(),
        ..meta("mesa")
    };
    let local = Local::of(&dir.path().join("root"));
    for m in [&with_llvm, &default] {
        let (file, buildinfo, _) = write_package_file(m, &out);
        local.add(&[package_of(&file, &buildinfo)]).expect("added");
    }
    let index = local.index().expect("readable").expect("an index");
    assert_eq!(
        index
            .packages
            .iter()
            .map(|e| e.meta.options.is_empty())
            .collect::<Vec<_>>(),
        [true, false],
        "{index:?}"
    );
}

#[test]
fn a_symlink_at_a_package_path_is_refused() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, _, _) = write_package_file(&meta("mesa"), &out);
    let link = out.join("linked.lpkg");
    std::os::unix::fs::symlink(&file, &link).expect("the symlink is created");
    let local = Local::of(&dir.path().join("root"));
    let error = local
        .add(&[PackageFile {
            path: link.clone(),
            buildinfo: link,
        }])
        .expect_err("a symlinked package path is refused");
    assert!(error.message.contains("symlink"), "{}", error.message);
    assert!(local.index().expect("readable").is_none());
}

#[test]
fn a_package_file_with_setuid_mode_lands_as_0644_in_the_pool() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, buildinfo, id) = write_package_file(&meta("mesa"), &out);
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&file, fs::Permissions::from_mode(0o4755)).expect("chmod");
    let local = Local::of(&dir.path().join("root"));
    local
        .add(&[package_of(&file, &buildinfo)])
        .expect("the package is added");
    let pool = dir
        .path()
        .join("root")
        .join("laft/local/pool")
        .join(format!("{id}.lpkg"));
    assert_eq!(
        fs::metadata(&pool)
            .expect("the pool file exists")
            .permissions()
            .mode()
            & 0o7777,
        0o644,
        "the pool file has the mode of the source"
    );
    let info = dir
        .path()
        .join("root")
        .join("laft/local/buildinfo")
        .join(format!("{id}.json"));
    assert_eq!(
        fs::metadata(&info)
            .expect("the buildinfo exists")
            .permissions()
            .mode()
            & 0o7777,
        0o644,
        "the published buildinfo has another mode"
    );
    assert_eq!(
        fs::read(&info).expect("readable"),
        fs::read(&buildinfo).expect("readable"),
        "the published buildinfo is not the checked file"
    );
}

#[test]
fn a_fifo_at_a_package_path_is_refused_without_hanging() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, _, _) = write_package_file(&meta("mesa"), &out);
    let fifo = out.join(file.file_name().expect("a file name"));
    fs::remove_file(&fifo).expect("removed");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .expect("the fifo is created");
    let local = Local::of(&dir.path().join("root"));
    let (done, received) = std::sync::mpsc::channel();
    let root = dir.path().join("root");
    let path = fifo.clone();
    std::thread::spawn(move || {
        let local = Local::of(&root);
        done.send(
            local
                .add(&[PackageFile {
                    path: path.clone(),
                    buildinfo: path,
                }])
                .is_err(),
        )
        .expect("sending");
    });
    let refused = received
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("add returns instead of hanging on the fifo");
    assert!(refused, "the fifo is refused");
    assert!(local.index().expect("readable").is_none());
}

#[test]
fn a_buildinfo_naming_other_outputs_is_refused() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("build/out");
    fs::create_dir_all(&out).expect("writable");
    let (file, buildinfo, id) = write_package_file(&meta("mesa"), &out);
    let other = laft_core::StoreId::from_manifest_hash(&Blake3Hash::of(b"another package"));
    let mut info = laft_build::Buildinfo::from_json(&fs::read(&buildinfo).expect("readable"))
        .expect("a buildinfo");
    info.outputs[0].id = other;
    fs::write(&buildinfo, info.to_json()).expect("writable");
    let local = Local::of(&dir.path().join("root"));
    let error = local
        .add(&[package_of(&file, &buildinfo)])
        .expect_err("a buildinfo with other outputs is refused");
    assert!(error.message.contains(&id.to_string()), "{}", error.message);
    assert!(local.index().expect("readable").is_none());
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
        .add(&[PackageFile {
            path: file.clone(),
            buildinfo: file,
        }])
        .expect_err("a corrupt package is refused");
    assert!(error.message.contains("mesa"), "{}", error.message);
    assert!(local.index().expect("readable").is_none());
}
