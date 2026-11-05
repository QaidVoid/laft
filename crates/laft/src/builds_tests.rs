//! Tests of the pure parts of a build run: requests, constraints, group
//! order, and snapshot verification.

use super::*;
use laft_core::{Blake3Hash, OptionValue, Variant};
use laft_resolve::{BuildRequest, Buildable, Reason};
use std::fs;
use std::fs::File;

/// A buildable package, as a resolution reports it.
fn buildable(name: &str, version: &str, variant: Variant) -> Buildable {
    Buildable {
        name: name.parse().expect("a name"),
        version: version.parse().expect("a version"),
        rev: 1,
        variant,
        defaults: BTreeMap::new(),
    }
}

/// A resolution with builds and nothing else.
fn resolution(view: &str, builds: Vec<BuildRequest>) -> laft_resolve::Resolution<'static> {
    laft_resolve::Resolution {
        view: view.to_owned(),
        packages: BTreeMap::new(),
        alternatives: BTreeMap::new(),
        learned: Vec::new(),
        warnings: Vec::new(),
        builds,
    }
}

/// A build request for `name`, built with one bool option.
fn request_of(name: &str, llvm: bool) -> BuildRequest {
    BuildRequest {
        buildable: buildable(name, "1.0", Variant::default()),
        values: BTreeMap::from([("llvm".to_owned(), OptionValue::Bool(llvm))]),
        reasons: vec![Reason::Requested {
            origin: "/etc/laft/system.stav:1:1".to_owned(),
            text: name.to_owned(),
        }],
    }
}

#[test]
fn a_request_is_written_as_an_item_with_sets_and_a_signature() {
    let values = BTreeMap::from([
        ("llvm".to_owned(), OptionValue::Bool(true)),
        (
            "backends".to_owned(),
            OptionValue::Tag("x11".parse().expect("a tag")),
        ),
    ]);
    let mut request = Request {
        name: "mesa".parse().expect("a name"),
        version: "25.2".parse().expect("a version"),
        variant: Variant::default(),
        values: values.clone(),
        view: "system".to_owned(),
        reasons: Vec::new(),
    };
    assert_eq!(request.item(), "mesa");
    assert_eq!(request.signature(), "backends=x11,llvm=true");
    assert_eq!(
        request.sets(),
        vec!["mesa.backends=x11".to_owned(), "mesa.llvm=true".to_owned()]
    );
    request.variant = Variant::Static;
    assert_eq!(request.item(), "mesa:static");
    let mut static_values = BTreeMap::new();
    static_values.insert("llvm".to_owned(), OptionValue::Bool(false));
    let static_request = Request {
        variant: Variant::Static,
        values: static_values,
        ..request
    };
    assert_eq!(static_request.signature(), "llvm=false");
}

#[test]
fn the_requests_of_a_resolution_come_in_view_order() {
    let resolved = crate::resolve::Resolved {
        system: resolution("system", vec![request_of("mesa", true)]),
        initrd: resolution("initrd", vec![request_of("busybox", false)]),
        views: BTreeMap::from([
            (
                "desk".parse().expect("a name"),
                resolution("desk", vec![request_of("wayland", false)]),
            ),
            (
                "cli".parse().expect("a name"),
                resolution("cli", vec![request_of("neovim", false)]),
            ),
        ]),
    };
    let requests = requests_of(&resolved);
    let views: Vec<&str> = requests
        .iter()
        .map(|request| request.view.as_str())
        .collect();
    assert_eq!(views, vec!["system", "initrd", "cli", "desk"]);
}

#[test]
fn a_constraint_sets_a_value_and_refuses_a_contradiction() {
    let reference = PkgRef::new_with_options(
        "mesa".parse().expect("a name"),
        None,
        Variant::default(),
        laft_core::OptionConstraints::from(vec![laft_core::OptionConstraint::True {
            name: "llvm".to_owned(),
        }]),
    );
    let mut values = BTreeMap::from([("llvm".to_owned(), OptionValue::Bool(false))]);
    constrain(
        &reference.options().as_slice()[0],
        &mut values,
        &BTreeSet::new(),
        &reference,
    )
    .expect("a constraint the values can take");
    assert_eq!(values.get("llvm"), Some(&OptionValue::Bool(true)));
    let fixed = BTreeSet::from(["llvm".to_owned()]);
    values.insert("llvm".to_owned(), OptionValue::Bool(false));
    let error = constrain(
        &reference.options().as_slice()[0],
        &mut values,
        &fixed,
        &reference,
    )
    .expect_err("a fixed value is refused");
    assert!(error.message.contains("fixes it to false"));
}

/// The build settings the tests use.
fn build_options() -> laft_config::Build {
    laft_config::Build {
        missing: laft_config::Missing::Build,
        dep_options: laft_config::DepOptions::Context,
        jobs: 0,
        workers: 0,
    }
}

/// The environment of a root, quiet so that tests print nothing.
fn env_of(root: &Path) -> Env {
    let global = crate::cli::Global {
        config: None,
        root: Some(root.to_path_buf()),
        json: false,
        color: None,
        ascii: false,
        verbose: 0,
        quiet: true,
        yes: false,
        offline: false,
        no_wait: false,
        no_pager: false,
        message_format: crate::cli::MessageFormat::Human,
    };
    Env::new(&global)
}

/// The metadata of one published package of `name`.
fn meta_of(name: &str, version: &str) -> laft_pkg::Meta {
    laft_pkg::Meta {
        format: laft_pkg::FORMAT,
        name: name.parse().expect("a name"),
        version: version.parse().expect("a version"),
        rev: 1,
        variant: Variant::default(),
        arch: crate::env::ARCH,
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
        options: BTreeMap::new(),
        requested_privileges: Vec::new(),
        installed_size: 0,
    }
}

/// One entry of an index.
fn entry_of(meta: laft_pkg::Meta, id: laft_core::StoreId) -> laft_index::IndexEntry {
    laft_index::IndexEntry {
        meta,
        id,
        file: format!("pool/{id}.lpkg"),
        size: 1,
        blake3: laft_core::Blake3Hash::of(b"m"),
    }
}

/// The accepted index of one repository at `url` that lists `entry`.
fn index_of(url: &str, entry: laft_index::IndexEntry) -> RepoIndex {
    let none: Vec<String> = Vec::new();
    let index = laft_index::Index {
        format: laft_index::Index::FORMAT,
        repo: "core".parse().expect("a name"),
        channel: "current".parse().expect("a name"),
        arch: crate::env::ARCH,
        generation: 1,
        timestamp: laft_core::Timestamp::now(),
        expires: laft_core::Timestamp::from_secs(
            laft_core::Timestamp::now().as_secs() + laft_index::MAX_VALIDITY_SECS,
        ),
        defaults: Default::default(),
        packages: vec![entry],
        yanked: Vec::new(),
        options: BTreeMap::new(),
        recipes: None,
        profile: None,
    };
    RepoIndex {
        name: "core".parse().expect("a name"),
        repo: laft_config::Repo {
            url: url.to_owned(),
            mirrors: Vec::new(),
            key: "RWSO9Zvh1ju0E6MO5PDnzQFYnptNxsLpLdwYSfm5Cs4Jc+14umRuka1M"
                .parse()
                .expect("a key"),
            keys: Vec::new(),
            channel: "current".parse().expect("a name"),
            profile: None,
            priority: 1,
            enable: true,
            allow_http: false,
        },
        accepted: laft_index::AcceptedIndex {
            index,
            record: laft_index::IndexRecord {
                channel: "current".parse().expect("a name"),
                profile: None,
                arch: crate::env::ARCH,
                generation: 1,
                blake3: Some(laft_core::Blake3Hash::of(b"x")),
                signer: "0123456789abcdef".parse().expect("a key id"),
            },
            expired: false,
        },
        mirrors: laft_fetch::Mirrors::new(url, &none, true).expect("a mirror"),
        synced: None,
    }
}

/// The source of the snapshot whose archive is `bytes`, published by
/// `core`.
fn source_of(bytes: &[u8]) -> SnapshotSource {
    SnapshotSource {
        repo: "core".parse().expect("a name"),
        snapshot: laft_index::RecipeSnapshot {
            file: format!("recipes/{}.tar.zst", Blake3Hash::of(bytes)),
            size: bytes.len() as u64,
            blake3: Blake3Hash::of(bytes),
        },
        name: "mesa".parse().expect("a name"),
        version: "1.0".parse().expect("a version"),
        variant: Variant::default(),
        defaults: BTreeMap::new(),
    }
}

/// The repository tree that publishes the archive `bytes` under
/// `dir/repo`, with its `file://` URL.
fn publish_snapshot(dir: &Path, bytes: &[u8]) -> String {
    let repo = dir.join("repo");
    let file = repo.join(format!("recipes/{}.tar.zst", Blake3Hash::of(bytes)));
    fs::create_dir_all(file.parent().expect("the file has a parent")).expect("writable");
    fs::write(file, bytes).expect("writable");
    format!("file://{}", repo.display())
}

/// The bytes of a tar.zst holding `files`.
fn archive_of(dir: &Path, files: &[(&str, &[u8])]) -> Vec<u8> {
    let raw = dir.join("snapshot.tar");
    {
        let file = File::create(&raw).expect("writable");
        let mut builder = tar::Builder::new(file);
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name, *content)
                .expect("writable");
        }
        builder
            .into_inner()
            .expect("writable")
            .sync_all()
            .expect("writable");
    }
    let mut bytes = Vec::new();
    let mut writer = zstd::stream::write::Encoder::new(&mut bytes, 3).expect("an encoder");
    std::io::copy(&mut File::open(&raw).expect("readable"), &mut writer).expect("compressed");
    writer.finish().expect("finished");
    bytes
}

#[test]
fn a_snapshot_that_cannot_be_unpacked_leaves_no_tree_behind() {
    let dir = laft_test::tempdir();
    let env = env_of(dir.path());
    let bytes = b"this is not an archive".as_slice();
    let url = publish_snapshot(dir.path(), bytes);
    let source = source_of(bytes);
    let mut run = Run {
        env: &env,
        snapshots: &BTreeMap::new(),
        indexes: &[index_of(
            &url,
            entry_of(
                meta_of("mesa", "1.0"),
                laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(b"m")),
            ),
        )],
        options: &build_options(),
        values: &BTreeMap::new(),
        built: Built::default(),
        seeded: BTreeSet::new(),
    };
    run.unpack(&source)
        .expect_err("the bytes are not a tarball");
    let target = snapshot_dir(&env, &source.snapshot.blake3);
    assert!(!target.exists(), "no tree is left behind: {target:?}");
    let staging = target.with_file_name(format!(
        "{}.part",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
    ));
    assert!(!staging.exists(), "the staging tree is removed");
}

#[test]
fn a_snapshot_unpacks_into_the_cache_of_its_hash() {
    let dir = laft_test::tempdir();
    let env = env_of(dir.path());
    let bytes = archive_of(
        dir.path(),
        &[("recipes/mesa/package.stav", b"package mesa {}")],
    );
    let url = publish_snapshot(dir.path(), &bytes);
    let source = source_of(&bytes);
    let mut run = Run {
        env: &env,
        snapshots: &BTreeMap::new(),
        indexes: &[index_of(
            &url,
            entry_of(
                meta_of("mesa", "1.0"),
                laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(b"m")),
            ),
        )],
        options: &build_options(),
        values: &BTreeMap::new(),
        built: Built::default(),
        seeded: BTreeSet::new(),
    };
    run.unpack(&source).expect("the snapshot unpacks");
    let target = snapshot_dir(&env, &source.snapshot.blake3);
    assert!(
        target.join("recipes/mesa/package.stav").is_file(),
        "{target:?}"
    );
    let downloaded = run.built.bytes;
    run.unpack(&source).expect("the snapshot is cached");
    assert_eq!(run.built.bytes, downloaded, "nothing downloads twice");
}

#[test]
fn a_published_dependency_is_not_marked_before_it_is_seeded() {
    let dir = laft_test::tempdir();
    let env = env_of(dir.path());
    let indexes = vec![index_of(
        "file:///tmp/none",
        entry_of(
            meta_of("mesa", "1.0"),
            laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(b"m")),
        ),
    )];
    let mut run = Run {
        env: &env,
        snapshots: &BTreeMap::new(),
        indexes: &indexes,
        options: &build_options(),
        values: &BTreeMap::new(),
        built: Built::default(),
        seeded: BTreeSet::new(),
    };
    let reference = PkgRef::new("mesa".parse().expect("a name"), None, Variant::default());
    match run.depends_on(&reference, "system").expect("decided") {
        Where::Published => {}
        other => panic!("expected a published reference, got {other:?}"),
    }
    assert!(
        run.seeded.is_empty(),
        "the reference is marked only when it is actually seeded"
    );
}

#[test]
fn the_seed_runs_against_the_build_root() {
    let arguments = seed_arguments(
        Path::new("/laft/build"),
        &[Path::new("/tmp/one.lpkg"), Path::new("/tmp/two.lpkg")],
    );
    assert_eq!(
        arguments,
        [
            "--root".to_owned(),
            "/laft/build".to_owned(),
            "seed".to_owned(),
            "/tmp/one.lpkg".to_owned(),
            "/tmp/two.lpkg".to_owned(),
        ]
    );
}

#[test]
fn groups_build_after_the_groups_whose_packages_they_wait_for() {
    let request = |name: &str| Request {
        name: name.parse().expect("a name"),
        version: "1.0".parse().expect("a version"),
        variant: Variant::default(),
        values: BTreeMap::new(),
        view: "system".to_owned(),
        reasons: Vec::new(),
    };
    let first = (Blake3Hash::of(b"zlib"), "zlib".to_owned());
    let second = (Blake3Hash::of(b"mesa"), "mesa".to_owned());
    let groups = BTreeMap::from([
        (first.clone(), vec![request("zlib")]),
        (second.clone(), vec![request("mesa")]),
    ]);
    let mut waiting = BTreeMap::new();
    waiting.insert(second.clone(), BTreeSet::from([first.clone()]));
    let ordered = order(&groups, &waiting).expect("an order exists");
    assert_eq!(ordered, vec![first.clone(), second.clone()]);
    let mut both = BTreeMap::new();
    both.insert(first.clone(), BTreeSet::from([second.clone()]));
    both.insert(second.clone(), BTreeSet::from([first.clone()]));
    assert!(order(&groups, &both).is_err());
}

#[test]
fn groups_of_one_snapshot_build_in_the_order_of_their_values() {
    let request = |name: &str, llvm: bool| Request {
        name: name.parse().expect("a name"),
        version: "1.0".parse().expect("a version"),
        variant: Variant::default(),
        values: BTreeMap::from([("llvm".to_owned(), OptionValue::Bool(llvm))]),
        view: "system".to_owned(),
        reasons: Vec::new(),
    };
    let snapshot = Blake3Hash::of(b"recipes");
    let parent = (snapshot.clone(), "llvm=false".to_owned());
    let child = (snapshot.clone(), "llvm=true".to_owned());
    let groups = BTreeMap::from([
        (parent.clone(), vec![request("zlib", false)]),
        (child.clone(), vec![request("mesa", true)]),
    ]);
    let mut waiting = BTreeMap::new();
    waiting.insert(child.clone(), BTreeSet::from([parent.clone()]));
    let ordered = order(&groups, &waiting).expect("an order exists");
    assert_eq!(ordered, vec![parent, child]);
}

#[test]
fn a_cached_snapshot_is_used_only_when_its_hash_matches() {
    let dir = laft_test::tempdir();
    let path = dir.path().join("snapshot.tar.zst");
    fs::write(&path, b"snapshot bytes").expect("writable");
    let source = SnapshotSource {
        repo: "core".parse().expect("a name"),
        snapshot: laft_index::RecipeSnapshot {
            file: "recipes/abc.tar.zst".to_owned(),
            size: 14,
            blake3: Blake3Hash::of(b"snapshot bytes"),
        },
        name: "mesa".parse().expect("a name"),
        version: "25.2".parse().expect("a version"),
        variant: Variant::default(),
        defaults: BTreeMap::new(),
    };
    assert!(verified(&path, &source));
    fs::write(&path, b"other bytes___").expect("writable");
    assert!(!verified(&path, &source));
}

#[test]
fn a_reason_is_one_line() {
    let requested = Reason::Requested {
        origin: "/etc/laft/system.stav:1:1".to_owned(),
        text: "mesa@25".to_owned(),
    };
    assert_eq!(reason_text(&requested), "mesa@25");
    let dependency = Reason::Dependency {
        by: "chromium".parse().expect("a name"),
        dep: "so:libGL.so.1".parse().expect("a dep"),
    };
    assert_eq!(
        reason_text(&dependency),
        "a dependency of chromium: so:libGL.so.1"
    );
}

#[test]
fn the_values_of_a_view_are_found_by_name() {
    let values = BTreeMap::from([(
        "system".parse::<Name>().expect("a name"),
        laft_config::OptionValues::default(),
    )]);
    assert!(view_values(&values, "system").is_some());
    assert!(view_values(&values, "desk").is_none());
}
