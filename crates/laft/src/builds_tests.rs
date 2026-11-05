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
fn the_seed_of_an_equal_offer_follows_the_repository_priority() {
    let dir = laft_test::tempdir();
    let env = env_of(dir.path());
    let entry = entry_of(
        meta_of("mesa", "1.0"),
        laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(b"m")),
    );
    let low = index_of("file:///tmp/low", entry.clone());
    let mut high = index_of("file:///tmp/high", entry);
    high.repo.priority = 9;
    let indexes = vec![low, high];
    let run = Run {
        env: &env,
        snapshots: &BTreeMap::new(),
        indexes: &indexes,
        options: &build_options(),
        values: &BTreeMap::new(),
        built: Built::default(),
        seeded: BTreeSet::new(),
    };
    let reference = PkgRef::new("mesa".parse().expect("a name"), None, Variant::default());
    let (_, index) = run
        .candidate(&reference, "system")
        .expect("an offer exists");
    assert_eq!(index.repo.priority, 9, "the higher priority wins");
}

#[test]
fn a_view_that_refuses_a_candidate_does_not_shadow_another_view() {
    let dir = laft_test::tempdir();
    let env = env_of(dir.path());
    let id = laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(b"m"));
    let mut plain = entry_of(meta_of("mesa", "1.0"), id);
    plain
        .meta
        .options
        .insert("llvm".to_owned(), laft_core::OptionValue::Bool(false));
    let mut llvm = entry_of(meta_of("mesa", "2.0"), id);
    llvm.meta
        .options
        .insert("llvm".to_owned(), laft_core::OptionValue::Bool(true));
    let indexes = vec![
        index_of("file:///tmp/core", plain),
        index_of("file:///tmp/core", llvm),
    ];
    let fixed = |value: bool| laft_config::OptionValues {
        fixed: BTreeMap::from([(
            "mesa".parse().expect("a name"),
            BTreeMap::from([(
                "llvm".to_owned(),
                laft_config::FixedSetting {
                    value: laft_core::OptionValue::Bool(value),
                    origin: "/etc/laft/system.stav:1:1".to_owned(),
                },
            )]),
        )]),
        preferred: BTreeMap::new(),
    };
    let values: BTreeMap<Name, laft_config::OptionValues> = BTreeMap::from([
        ("cli".parse().expect("a name"), fixed(false)),
        ("desk".parse().expect("a name"), fixed(true)),
    ]);
    let run = Run {
        env: &env,
        snapshots: &BTreeMap::new(),
        indexes: &indexes,
        options: &build_options(),
        values: &values,
        built: Built::default(),
        seeded: BTreeSet::new(),
    };
    let reference = PkgRef::new("mesa".parse().expect("a name"), None, Variant::default());
    let (cli_entry, _) = run
        .candidate(&reference, "cli")
        .expect("the cli view finds its candidate");
    assert_eq!(cli_entry.meta.version.as_str(), "1.0", "llvm=false");
    let (desk_entry, _) = run
        .candidate(&reference, "desk")
        .expect("the desk view finds its candidate");
    assert_eq!(desk_entry.meta.version.as_str(), "2.0", "llvm=true");
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
        variant: Variant::default(),
        values: BTreeMap::from([("llvm".to_owned(), OptionValue::Bool(llvm))]),
        view: "system".to_owned(),
        reasons: Vec::new(),
    };
    let snapshot = Blake3Hash::of(b"recipes");
    let parent = (snapshot, "llvm=false".to_owned());
    let child = (snapshot, "llvm=true".to_owned());
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

/// The `laft-build` binary next to this test binary, copied from the
/// target directory, which the workspace tests build first.
fn laft_build_exe() -> PathBuf {
    static ONCE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let exe = std::env::current_exe().expect("the test binary has a path");
        let sibling = exe
            .parent()
            .expect("the test binary has a directory")
            .join("laft-build");
        let built = match std::env::var("CARGO_TARGET_DIR") {
            Ok(custom) => Path::new(&custom).join("debug/laft-build"),
            Err(_) => Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .expect("the workspace")
                .join("target/debug/laft-build"),
        };
        fs::copy(&built, &sibling).expect("laft-build copies next to the test binary");
        sibling
    })
    .clone()
}

/// A gzip-free recipe source: a zstd tarball of one file, with its
/// SHA-256 hash.
fn source_tarball(dir: &Path) -> (Vec<u8>, String) {
    let mut builder = tar::Builder::new(Vec::new());
    let text = b"hello\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(text.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, "hello-1.0/hello.txt", &text[..])
        .expect("writable");
    let tar = builder.into_inner().expect("writable");
    let mut bytes = Vec::new();
    let mut writer = zstd::stream::write::Encoder::new(&mut bytes, 3).expect("an encoder");
    std::io::Write::write_all(&mut writer, &tar).expect("compressed");
    writer.finish().expect("finished");
    use sha2::{Digest, Sha256};
    let hex: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mirror = dir.join("mirror/sha256");
    fs::create_dir_all(&mirror).expect("writable");
    fs::write(mirror.join(&hex), &bytes).expect("writable");
    (bytes, hex)
}

/// The recipe repository of `hello`, as a snapshot archive published at
/// `repo`, with `hex` as the hash of its source and `mirror` as its
/// mirror. Returns the archive bytes.
fn publish_recipes(dir: &Path, repo: &Path, mirror: &Path, hex: &str) -> Vec<u8> {
    let recipe = format!(
        r#"package hello {{
  version = "1.0"
  summary = "Hello"
  license = [MIT]
  deps = [greet]
  source main {{
    url = ["https://example.org/hello-1.0.tar.zst"]
    sha256 = "sha256:{hex}"
  }}
  build custom {{}}
  env = {{ LD_LIBRARY_PATH = "/usr/lib" }}
  phases {{
    install = "mkdir -p \"$DESTDIR/usr/share/hello\" && printf hello > \"$DESTDIR/usr/share/hello/hello.txt\""
  }}
}}
"#
    );
    let descriptor = format!(
        "recipe_repo core {{\n  source_mirrors = [\"file://{}\"]\n}}\n",
        mirror.display()
    );
    let bytes = archive_of(
        dir,
        &[
            ("recipes.stav", descriptor.as_bytes()),
            ("recipes/hello/package.stav", recipe.as_bytes()),
        ],
    );
    let file = repo.join(format!("recipes/{}.tar.zst", Blake3Hash::of(&bytes)));
    fs::create_dir_all(file.parent().expect("the file has a parent")).expect("writable");
    fs::write(&file, &bytes).expect("writable");
    bytes
}

/// The published package of `name` at 1.0, written into `pool` below
/// `repo`. Returns the index entry for it.
fn publish_empty(repo: &Path, name: &str, summary: &str) -> laft_index::IndexEntry {
    publish_pkg(repo, name, summary, &[])
}

/// As [`publish_empty`], with `files` in the payload. Each file carries the
/// mode to keep, so executable files such as the ELF interpreter stay
/// executable inside the build view.
fn publish_pkg(
    repo: &Path,
    name: &str,
    summary: &str,
    files: &[(&str, &[u8], u32)],
) -> laft_index::IndexEntry {
    let payload = laft_test::tempdir();
    let mut meta = meta_of(name, "1.0");
    meta.summary = summary.to_owned();
    for (path, content, mode) in files {
        let full = payload.path().join(path);
        fs::create_dir_all(full.parent().expect("a file has a parent")).expect("writable");
        fs::write(&full, content).expect("writable");
        fs::set_permissions(&full, fs::Permissions::from_mode(*mode)).expect("chmod");
    }
    let entries = laft_pkg::scan(payload.path()).expect("the payload scans");
    meta.installed_size = files.iter().map(|(_, c, _)| c.len() as u64).sum();
    let meta_json = meta.to_json().expect("valid metadata");
    let manifest = laft_pkg::Manifest::new(meta.identity(), Blake3Hash::of(&meta_json), entries)
        .expect("valid manifest");
    let mut bytes = Vec::new();
    laft_pkg::write_package(
        &mut bytes,
        payload.path(),
        &meta,
        &manifest,
        laft_pkg::Compression::DEVELOPMENT,
    )
    .expect("the package writes");
    let id = manifest.store_id();
    let file = format!("pool/{id}.lpkg");
    fs::create_dir_all(repo.join("pool")).expect("writable");
    fs::write(repo.join(&file), &bytes).expect("writable");
    laft_index::IndexEntry {
        meta,
        id,
        file,
        size: bytes.len() as u64,
        blake3: Blake3Hash::of(&bytes),
    }
}

/// The files of the host's `/bin/sh` with the libraries it needs, as the
/// payload of the build view's `dash` package. Every file lands below
/// `usr/lib` or `usr/bin` and keeps the executable bit of its host file.
/// The recipe that runs in the view sets `LD_LIBRARY_PATH` to `/usr/lib`,
/// so the loader finds the libraries without `ld.so.cache`.
fn host_shell_files() -> Vec<(String, Vec<u8>, u32)> {
    const LIBRARY_DIRS: [&str; 6] = [
        "/lib64",
        "/usr/lib64",
        "/lib",
        "/usr/lib",
        "/lib/x86_64-linux-gnu",
        "/usr/lib/x86_64-linux-gnu",
    ];
    let mut files: BTreeMap<String, (Vec<u8>, u32)> = BTreeMap::new();
    let mut queue = Vec::new();
    let host = fs::canonicalize("/bin/sh").expect("the host has a shell");
    files.insert(
        "usr/bin/sh".to_owned(),
        (fs::read(&host).expect("the shell reads"), host_mode(&host)),
    );
    queue.push(host);
    let mut seen = BTreeSet::new();
    while let Some(file) = queue.pop() {
        let Some(elf) =
            laft_elf::inspect(&fs::read(&file).expect("the file reads")).expect("the ELF inspects")
        else {
            continue;
        };
        let found = elf.needed.iter().filter_map(|lib| {
            LIBRARY_DIRS
                .iter()
                .map(|dir| Path::new(dir).join(lib.as_str()))
                .find(|path| path.exists())
        });
        for dep in elf.interpreter.iter().map(PathBuf::from).chain(found) {
            if !seen.insert(dep.clone()) {
                continue;
            }
            let host = dep.to_str().expect("a UTF-8 library path");
            let host = host.strip_prefix('/').expect("an absolute library path");
            let under = host.strip_prefix("usr/").unwrap_or(host);
            let rel = match under.split_once('/') {
                Some(("lib" | "lib64", tail)) => format!("usr/lib/{tail}"),
                _ => format!("usr/{under}"),
            };
            files.insert(
                rel,
                (fs::read(&dep).expect("the library reads"), host_mode(&dep)),
            );
            queue.push(dep);
        }
    }
    files
        .into_iter()
        .map(|(path, (bytes, mode))| (path, bytes, mode))
        .collect()
}

/// The mode to keep for a copy of `file`: executable when the host file
/// has an execute bit, read-only otherwise.
fn host_mode(file: &Path) -> u32 {
    let mode = fs::metadata(file)
        .expect("the host file stats")
        .permissions()
        .mode();
    if mode & 0o111 != 0 { 0o555 } else { 0o444 }
}

/// The host's tools as the payload of the build view's `coreutils`
/// package, keeping the executable bits of the host files.
fn host_tool_files(tools: &[&str]) -> Vec<(String, Vec<u8>, u32)> {
    tools
        .iter()
        .map(|tool| {
            let host = Path::new("/usr/bin").join(tool);
            (
                format!("usr/bin/{tool}"),
                fs::read(&host).expect("the tool reads"),
                host_mode(&host),
            )
        })
        .collect()
}

#[test]
fn a_round_seeds_builds_publishes_and_a_second_round_rests() {
    if !Path::new("/bin/sh").exists() {
        println!("skipping: the host has no /bin/sh");
        return;
    }
    let dir = laft_test::tempdir();
    let probe = dir.path().join("probe");
    fs::create_dir_all(probe.join("build")).expect("writable");
    fs::create_dir_all(probe.join("out")).expect("writable");
    match laft_sandbox::Sandbox::new("/usr", probe.join("build"), probe.join("out"))
        .command("/usr/bin/true")
        .status()
    {
        Ok(status) if status.success() => {}
        other => {
            println!("skipping: the sandbox is unavailable: {other:?}");
            return;
        }
    }
    laft_build_exe();
    let env = env_of(dir.path());
    let (_, hex) = source_tarball(dir.path());
    let www = dir.path().join("www");
    let url = format!("file://{}", www.display());
    let snapshot = publish_recipes(dir.path(), &www, &dir.path().join("mirror"), &hex);
    let greet = publish_empty(&www, "greet", "The greeting library");
    if !Path::new("/bin/sh").exists() {
        println!("skipping: the host has no /bin/sh");
        return;
    }
    let dash_files = host_shell_files();
    let dash_refs: Vec<(&str, &[u8], u32)> = dash_files
        .iter()
        .map(|(path, bytes, mode)| (path.as_str(), bytes.as_slice(), *mode))
        .collect();
    let mut entries = vec![greet.clone()];
    entries.push(publish_pkg(
        &www,
        "dash",
        "The POSIX shell of the build view",
        &dash_refs,
    ));
    let tool_files = host_tool_files(&["mkdir", "printf"]);
    let tool_refs: Vec<(&str, &[u8], u32)> = tool_files
        .iter()
        .map(|(path, bytes, mode)| (path.as_str(), bytes.as_slice(), *mode))
        .collect();
    entries.push(publish_pkg(
        &www,
        "coreutils",
        "The tools of the build view",
        &tool_refs,
    ));
    for name in laft_build::view::BUILD_BASE {
        if *name != "dash" && *name != "coreutils" {
            entries.push(publish_empty(&www, name, "The build view of the tests"));
        }
    }
    let mut index = index_of(&url, greet.clone());
    index.accepted.index.packages = entries;
    index.accepted.index.recipes = Some(laft_index::RecipeSnapshot {
        file: format!("recipes/{}.tar.zst", Blake3Hash::of(&snapshot)),
        size: snapshot.len() as u64,
        blake3: Blake3Hash::of(&snapshot),
    });
    let indexes = vec![index];
    let root = laft_build::BuildRoot::new(env.path("/laft/build"));
    for entry in &indexes[0].accepted.index.packages {
        if entry.meta.name != greet.meta.name {
            laft_build::seed(&root, &www.join(&entry.file)).expect("the build view is seeded");
        }
    }
    let hello: Name = "hello".parse().expect("a name");
    let source = SnapshotSource {
        repo: "core".parse().expect("a name"),
        snapshot: laft_index::RecipeSnapshot {
            file: format!("recipes/{}.tar.zst", Blake3Hash::of(&snapshot)),
            size: snapshot.len() as u64,
            blake3: Blake3Hash::of(&snapshot),
        },
        name: hello.clone(),
        version: "1.0".parse().expect("a version"),
        variant: Variant::default(),
        defaults: BTreeMap::new(),
    };
    let snapshots = SnapshotMap::from_iter([((hello.clone(), Variant::default()), source)]);
    let request = Request {
        name: hello.clone(),
        variant: Variant::default(),
        values: BTreeMap::new(),
        view: "system".to_owned(),
        reasons: Vec::new(),
    };
    let built = match run(
        &env,
        std::slice::from_ref(&request),
        &snapshots,
        &indexes,
        &build_options(),
        &BTreeMap::new(),
    ) {
        Ok(built) => built,
        Err(failure) => {
            fn dump(dir: &Path, depth: usize) {
                if depth == 0 {
                    return;
                }
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            dump(&path, depth - 1);
                        } else {
                            println!(
                                "--- {}: {}",
                                path.display(),
                                fs::read_to_string(&path).unwrap_or_default()
                            );
                        }
                    }
                }
            }
            dump(&env.path("/laft/build/logs"), 4);
            panic!("the first round failed to build: {failure:?}");
        }
    };
    assert_eq!(built.names, vec![hello.clone()], "the build is reported");
    assert!(built.bytes > 0, "the recipe snapshot was downloaded");

    let local = Local::of(env.root.path());
    let published = local
        .source()
        .expect("the local key works")
        .expect("a local index exists");
    let entries: Vec<&laft_index::IndexEntry> = published
        .accepted
        .index
        .packages
        .iter()
        .filter(|entry| entry.meta.name == hello)
        .collect();
    assert_eq!(entries.len(), 1, "hello is published once: {entries:?}");
    assert_eq!(entries[0].meta.version.as_str(), "1.0");
    assert!(
        env.root
            .path()
            .join("laft/local")
            .join(&entries[0].file)
            .is_file(),
        "the package file is in the local repository pool"
    );

    let again = run(
        &env,
        &[request],
        &snapshots,
        &indexes,
        &build_options(),
        &BTreeMap::new(),
    )
    .expect("the second round succeeds");
    assert_eq!(again.bytes, 0, "nothing downloads twice");
    assert_eq!(again.names, vec![hello.clone()], "the build repeats");
    let once_more = local
        .source()
        .expect("the key works")
        .expect("a local index exists");
    assert_eq!(
        once_more.accepted.index.generation, published.accepted.index.generation,
        "the same build publishes nothing new"
    );
    let locals: Vec<&laft_index::IndexEntry> = once_more
        .accepted
        .index
        .packages
        .iter()
        .filter(|entry| entry.meta.name == hello)
        .collect();
    assert_eq!(locals.len(), 1, "no duplicate is published");
}

/// The builds run as the static `laft-build` account of the registry.
#[test]
fn the_build_account_is_the_static_laft_build_account() {
    assert_eq!(laft_core::system_uid("laft-build"), Some(BUILD_UID));
    assert_eq!(laft_core::system_gid("laft-build"), Some(BUILD_GID));
}

/// The build view of a dependency holds the option values of the view the
/// package is for under `dep_options = context`, and the snapshot's
/// defaults under `dep_options = defaults`.
#[test]
fn the_build_view_options_follow_the_dep_options_setting() {
    let dir = laft_test::tempdir();
    let env = env_of(dir.path());
    let id = laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(b"m"));
    let mut free = entry_of(meta_of("gtk4", "1.0"), id);
    free.meta
        .options
        .insert("x11".to_owned(), laft_core::OptionValue::Bool(true));
    let mut without_x11 = entry_of(meta_of("gtk4", "2.0"), id);
    without_x11
        .meta
        .options
        .insert("x11".to_owned(), laft_core::OptionValue::Bool(false));
    let indexes = vec![
        index_of("file:///tmp/core", free),
        index_of("file:///tmp/core", without_x11),
    ];
    let source = SnapshotSource {
        repo: "core".parse().expect("a name"),
        snapshot: laft_index::RecipeSnapshot {
            file: "recipes/x.tar.zst".to_owned(),
            size: 1,
            blake3: laft_core::Blake3Hash::of(b"x"),
        },
        name: "gtk4".parse().expect("a name"),
        version: "3.0".parse().expect("a version"),
        variant: Variant::default(),
        defaults: BTreeMap::from([("x11".to_owned(), laft_core::OptionValue::Bool(true))]),
    };
    let snapshots = SnapshotMap::from_iter([(
        ("gtk4".parse().expect("a name"), Variant::default()),
        source,
    )]);
    let fixed = laft_config::OptionValues {
        fixed: BTreeMap::from([(
            "gtk4".parse().expect("a name"),
            BTreeMap::from([(
                "x11".to_owned(),
                laft_config::FixedSetting {
                    value: laft_core::OptionValue::Bool(false),
                    origin: "/etc/laft/system.stav:1:1".to_owned(),
                },
            )]),
        )]),
        preferred: BTreeMap::new(),
    };
    let values = BTreeMap::from([("system".parse().expect("a name"), fixed)]);
    let reference = PkgRef::new("gtk4".parse().expect("a name"), None, Variant::default());

    let mut context = build_options();
    context.dep_options = laft_config::DepOptions::Context;
    let run = Run {
        env: &env,
        snapshots: &snapshots,
        indexes: &indexes,
        options: &context,
        values: &values,
        built: Built::default(),
        seeded: BTreeSet::new(),
    };
    let source = &snapshots[&("gtk4".parse().expect("a name"), Variant::default())];
    let request = run
        .dependency(&reference, source, "system")
        .expect("the context builds a request");
    assert_eq!(
        request.values.get("x11"),
        Some(&laft_core::OptionValue::Bool(false)),
        "the view's fixed value replaces the snapshot default"
    );
    let (entry, _) = run
        .candidate(&reference, "system")
        .expect("the context finds a candidate");
    assert_eq!(entry.meta.version.as_str(), "2.0", "the X11-free build");

    let mut defaults = build_options();
    defaults.dep_options = laft_config::DepOptions::Defaults;
    let run = Run {
        env: &env,
        snapshots: &snapshots,
        indexes: &indexes,
        options: &defaults,
        values: &values,
        built: Built::default(),
        seeded: BTreeSet::new(),
    };
    let request = run
        .dependency(&reference, source, "system")
        .expect("the defaults build a request");
    assert_eq!(
        request.values.get("x11"),
        Some(&laft_core::OptionValue::Bool(true)),
        "the snapshot default, though the view drops X11"
    );
    let (entry, _) = run
        .candidate(&reference, "system")
        .expect("the defaults find a candidate");
    assert_eq!(
        entry.meta.version.as_str(),
        "2.0",
        "the newest published build, though the view drops X11"
    );
}
