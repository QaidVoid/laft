//! Builders and checks shared by the unit tests. Everything happens in a
//! temporary directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use laft_core::{Blake3Hash, Name, StoreId};
use laft_pkg::{Alternative, Arch, Compression, Manifest, Meta, scan, write_package};
use tempfile::TempDir;

use crate::{
    GenerationInput, Init, Lock, PackageInput, PackageRecord, Plan, Root, Transaction, ViewInput,
};

/// A temporary root, plus a directory for package files outside it.
pub(crate) struct Env {
    dir: TempDir,
    pub(crate) root: Root,
}

impl Env {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("can create a temporary directory");
        let root = Root::new(dir.path().join("root"));
        fs::create_dir_all(dir.path().join("root")).expect("can create the root");
        fs::create_dir_all(dir.path().join("pkgs")).expect("can create the package directory");
        Self { dir, root }
    }

    pub(crate) fn lock(&self) -> Lock {
        self.root.lock("test").expect("the lock is free")
    }

    fn pkgs(&self) -> PathBuf {
        self.dir.path().join("pkgs")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        make_writable(self.dir.path());
    }
}

fn make_writable(dir: &Path) {
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    if let Ok(items) = fs::read_dir(dir) {
        for item in items.flatten() {
            if item.file_type().is_ok_and(|t| t.is_dir()) {
                make_writable(&item.path());
            }
        }
    }
}

/// A package to build.
pub(crate) struct Pkg {
    name: String,
    version: String,
    files: Vec<(String, Vec<u8>)>,
    symlinks: Vec<(String, String)>,
    alternatives: Vec<(String, Vec<String>)>,
}

/// A built package file.
pub(crate) struct Built {
    pub(crate) path: PathBuf,
    pub(crate) id: StoreId,
    pub(crate) dir: String,
    pub(crate) manifest: Manifest,
}

impl Pkg {
    pub(crate) fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            files: Vec::new(),
            symlinks: Vec::new(),
            alternatives: Vec::new(),
        }
    }

    /// Adds a file. Files under `usr/bin` are executable.
    pub(crate) fn file(mut self, path: &str, content: &[u8]) -> Self {
        self.files.push((path.into(), content.into()));
        self
    }

    pub(crate) fn symlink(mut self, path: &str, target: &str) -> Self {
        self.symlinks.push((path.into(), target.into()));
        self
    }

    pub(crate) fn alternative(mut self, group: &str, paths: &[&str]) -> Self {
        self.alternatives.push((
            group.into(),
            paths.iter().map(|p| (*p).to_owned()).collect(),
        ));
        self
    }

    pub(crate) fn build(&self, env: &Env) -> Built {
        let payload = env
            .pkgs()
            .join(format!("payload-{}-{}", self.name, self.version));
        fs::create_dir_all(&payload).expect("can create the payload");
        for (path, content) in &self.files {
            let full = payload.join(path);
            fs::create_dir_all(full.parent().expect("payload paths have parents"))
                .expect("can create payload directories");
            fs::write(&full, content).expect("can write a payload file");
            let mode = if path.starts_with("usr/bin/") {
                0o555
            } else {
                0o444
            };
            fs::set_permissions(&full, fs::Permissions::from_mode(mode))
                .expect("can chmod a payload file");
        }
        for (path, target) in &self.symlinks {
            let full = payload.join(path);
            fs::create_dir_all(full.parent().expect("payload paths have parents"))
                .expect("can create payload directories");
            std::os::unix::fs::symlink(target, &full).expect("can create a payload symlink");
        }
        let entries = scan(&payload).expect("the payload scans");
        let entries: Vec<_> = entries
            .into_iter()
            .map(|entry| match entry {
                laft_pkg::Entry::Dir { path, .. } => laft_pkg::Entry::Dir {
                    path,
                    mode: 0o755,
                    owner: None,
                    group: None,
                },
                other => other,
            })
            .collect();
        let mut alternatives: Vec<Alternative> = self
            .alternatives
            .iter()
            .map(|(group, paths)| Alternative {
                group: group.parse().expect("valid group"),
                paths: paths.clone(),
            })
            .collect();
        alternatives.sort_by(|a, b| a.group.cmp(&b.group));
        let size = entries
            .iter()
            .map(|e| match e {
                laft_pkg::Entry::File { size, .. } => *size,
                _ => 0,
            })
            .sum();
        let meta = Meta {
            format: laft_pkg::FORMAT,
            name: self.name.parse().expect("valid name"),
            version: self.version.parse().expect("valid version"),
            rev: 1,
            variant: Default::default(),
            arch: Arch::Any,
            origin: self.name.parse().expect("valid name"),
            summary: format!("Test package {}", self.name),
            homepage: None,
            license: vec!["MIT".into()],
            depends: vec![],
            provides: vec![],
            conflicts: vec![],
            replaces: vec![],
            alternatives,
            triggers: vec![],
            static_deps: vec![],
            abi: Default::default(),
            abi_epoch: 0,
            requested_privileges: vec![],
            installed_size: size,
        };
        let meta_hash = Blake3Hash::of(&meta.to_json().expect("test metadata is valid"));
        let manifest =
            Manifest::new(meta.identity(), meta_hash, entries).expect("test manifest is valid");
        let path = env.pkgs().join(manifest.package().file_name());
        let mut file = fs::File::create(&path).expect("can create the package file");
        write_package(
            &mut file,
            &payload,
            &meta,
            &manifest,
            Compression::DEVELOPMENT,
        )
        .expect("the package writes");
        let id = manifest.store_id();
        let package = manifest.package();
        let dir = id.dir_name(&package.name, &package.version, package.variant);
        Built {
            path,
            id,
            dir,
            manifest,
        }
    }
}

pub(crate) fn inputs(packages: &[&Built]) -> Vec<PackageInput> {
    packages
        .iter()
        .map(|p| PackageInput {
            dir: p.dir.clone(),
            reasons: vec![crate::Reason::Requested {
                origin: "test".into(),
                text: p.manifest.package().name.to_string(),
            }],
        })
        .collect()
}

pub(crate) fn name(s: &str) -> Name {
    s.parse().expect("valid name")
}

pub(crate) fn gen_input(packages: &[&Built], views: &[(&str, u64)]) -> GenerationInput {
    GenerationInput {
        packages: inputs(packages),
        alternatives: BTreeMap::new(),
        views: views.iter().map(|(n, v)| (name(n), *v)).collect(),
        message: Some("test".into()),
        laft_version: "0.1.0".into(),
        arch: "x86_64-v3".into(),
        init: Init {
            backend: "runit".into(),
            exec: vec!["/usr/bin/runit".into()],
        },
        config: b"{\"packages\": []}\n".to_vec(),
        config_files: vec![],
        indexes: vec![],
    }
}

pub(crate) fn view_input(view: &str, packages: &[&Built]) -> ViewInput {
    ViewInput {
        name: name(view),
        packages: inputs(packages),
        alternatives: BTreeMap::new(),
        users: BTreeMap::from([("qaid".into(), 1000)]),
        groups: BTreeMap::from([("wheel".into(), 10)]),
    }
}

pub(crate) fn plan(root: &Root, views: &[&str]) -> Plan {
    Plan {
        command: "apply".into(),
        previous: root.default_generation().expect("default is readable"),
        views: views.iter().map(|v| name(v)).collect(),
    }
}

/// Runs a whole transaction that adds `packages`, assembles them as view
/// `view` when given and as the generation, and makes it the default.
pub(crate) fn apply(lock: &Lock, packages: &[&Built], view: Option<&str>) -> crate::Result<u64> {
    let views: Vec<&str> = view.into_iter().collect();
    let mut tx = Transaction::begin(lock, &plan(lock.root(), &views))?;
    for package in packages {
        tx.add(&package.path, &package.id)?;
    }
    let mut versions = Vec::new();
    if let Some(view) = view {
        versions.push((view, tx.assemble_view(&view_input(view, packages))?));
    }
    tx.assemble_generation(&gen_input(packages, &versions))?;
    tx.make_default()
}

/// Applies `packages` under a fresh lock and returns the generation.
pub(crate) fn commit(env: &Env, packages: &[&Built]) -> u64 {
    let lock = env.lock();
    lock.recover().expect("recovery succeeds");
    apply(&lock, packages, None).expect("the transaction succeeds")
}

/// Checks that the tree under `root` is consistent: no transaction is
/// pending, the staging directory is empty, every store entry matches its
/// manifest, every generation and view version is complete and made of
/// links to its packages' store files, and the default generation exists.
pub(crate) fn assert_consistent(root: &Root) {
    let leftovers = |dir: PathBuf| -> Vec<String> {
        fs::read_dir(dir)
            .expect("the directory exists")
            .map(|i| {
                i.expect("readable")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    };
    assert_eq!(leftovers(root.journal_dir()), Vec::<String>::new());
    assert_eq!(leftovers(root.tmp_dir()), Vec::<String>::new());
    let store = root.store();
    for dir in store.list().expect("the store lists") {
        assert_eq!(
            store.verify(&dir, true).expect("verifiable"),
            vec![],
            "{dir}"
        );
    }
    for number in root.generations().expect("generations list") {
        let generation = root
            .generation(number)
            .expect("every generation is complete");
        let dir = root.gen_dir(number);
        assert_mode(&dir, 0o555);
        assert_tree(root, &dir.join("usr"), "usr", &generation.packages);
        for (name, version) in &generation.views {
            root.view(name, *version).expect("referenced views exist");
        }
    }
    if let Some(default) = root.default_generation().expect("default is readable") {
        root.generation(default)
            .expect("the default generation is complete");
    }
    for name in crate::journal::view_names(&root.views_dir()).expect("views list") {
        for version in root.view_versions(&name).expect("versions list") {
            let view = root
                .view(&name, version)
                .expect("every view version is complete");
            let dir = root.view_dir(&name, version);
            assert_mode(&dir, 0o555);
            assert_tree(root, &dir.join("usr"), "usr", &view.packages);
        }
    }
}

pub(crate) fn assert_mode(path: &Path, mode: u32) {
    let actual = fs::symlink_metadata(path).expect("path exists").mode() & 0o7777;
    assert_eq!(actual, mode, "{}", path.display());
}

/// Checks that every path of `tree` comes from one of `packages`: files are
/// the store's inodes, symlinks have a store symlink's target, and
/// directories are sealed.
fn assert_tree(root: &Root, tree: &Path, rel: &str, packages: &[PackageRecord]) {
    assert_mode(tree, 0o555);
    let store = root.store();
    for item in fs::read_dir(tree).expect("the tree reads") {
        let item = item.expect("readable");
        let path = item.path();
        let rel = format!("{rel}/{}", item.file_name().to_string_lossy());
        let metadata = fs::symlink_metadata(&path).expect("readable");
        if metadata.is_dir() {
            assert_tree(root, &path, &rel, packages);
            continue;
        }
        let sources: BTreeSet<_> = packages
            .iter()
            .filter_map(|p| fs::symlink_metadata(store.path(&p.dir).join(&rel)).ok())
            .filter(|source| {
                if metadata.is_symlink() {
                    source.is_symlink()
                } else {
                    source.ino() == metadata.ino() && source.dev() == metadata.dev()
                }
            })
            .map(|source| source.ino())
            .collect();
        assert!(!sources.is_empty(), "{rel} does not come from the store");
        if metadata.is_symlink() {
            let target = fs::read_link(&path).expect("readable");
            assert!(
                packages.iter().any(
                    |p| fs::read_link(store.path(&p.dir).join(&rel)).is_ok_and(|t| t == target)
                ),
                "{rel} points elsewhere than its store symlink"
            );
        }
    }
}
