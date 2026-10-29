//! Package builders, fixture repositories and a runner for the binary,
//! shared by the integration tests.

// Each test binary uses a different part of this module.
#![allow(dead_code)]

use std::cmp::Reverse;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use laft_core::{Blake3Hash, Timestamp};
use laft_index::{Defaults, Index, IndexEntry};
use laft_pkg::{Alternative, Arch, Compression, Manifest, Meta, Reader, scan, write_package};

/// A signing time all tests build on.
pub(crate) const T0: u64 = 1_761_652_800;
pub(crate) const HOUR: u64 = 3600;
pub(crate) const DAY: u64 = 86_400;

/// The `--now` argument for `T0 + offset`.
pub(crate) fn at(offset: u64) -> String {
    Timestamp::from_secs(T0 + offset).to_string()
}

/// A package to build.
pub(crate) struct Pkg {
    meta: Meta,
    files: Vec<(String, Vec<u8>)>,
    buildinfo: Option<&'static str>,
}

/// A package `name` of `version`, revision 1, with no payload files yet.
pub(crate) fn pkg(name: &str, version: &str) -> Pkg {
    Pkg {
        meta: Meta {
            format: laft_pkg::FORMAT,
            name: name.parse().expect("valid name"),
            version: version.parse().expect("valid version"),
            rev: 1,
            variant: Default::default(),
            arch: Arch::X86_64V3,
            origin: name.parse().expect("valid name"),
            summary: format!("The {name} package"),
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
            installed_size: 0,
        },
        files: vec![],
        buildinfo: Some(""),
    }
}

impl Pkg {
    pub(crate) fn rev(mut self, rev: u32) -> Self {
        self.meta.rev = rev;
        self
    }

    pub(crate) fn file(mut self, path: &str, content: &str) -> Self {
        self.files.push((path.into(), content.as_bytes().to_vec()));
        self
    }

    pub(crate) fn depends(mut self, dep: &str) -> Self {
        self.meta
            .depends
            .push(dep.parse().expect("valid dependency"));
        self.meta.depends.sort_by_key(ToString::to_string);
        self
    }

    pub(crate) fn provides(mut self, provide: &str) -> Self {
        self.meta
            .provides
            .push(provide.parse().expect("valid provide"));
        self.meta.provides.sort_by_key(ToString::to_string);
        self
    }

    pub(crate) fn conflicts(mut self, dep: &str) -> Self {
        self.meta
            .conflicts
            .push(dep.parse().expect("valid dependency"));
        self.meta.conflicts.sort_by_key(ToString::to_string);
        self
    }

    pub(crate) fn alternative(mut self, group: &str, paths: &[&str]) -> Self {
        self.meta.alternatives.push(Alternative {
            group: group.parse().expect("valid group"),
            paths: paths.iter().map(|p| (*p).into()).collect(),
            priority: 0,
        });
        self
    }

    /// Writes buildinfo that marks the build as made from a dirty recipe
    /// tree.
    pub(crate) fn dirty(mut self) -> Self {
        self.buildinfo = Some("dirty");
        self
    }

    /// Writes no buildinfo.
    pub(crate) fn without_buildinfo(mut self) -> Self {
        self.buildinfo = None;
        self
    }

    /// Writes the package file and its buildinfo into `out`, as a build
    /// does, and returns the package file.
    pub(crate) fn write(mut self, out: &Path) -> PathBuf {
        fs::create_dir_all(out).expect("the temporary directory is usable");
        let payload = laft_test::tempdir();
        for (path, content) in &self.files {
            let full = payload.path().join(path);
            fs::create_dir_all(full.parent().expect("payload paths have a parent"))
                .expect("the temporary directory is usable");
            fs::write(&full, content).expect("the temporary directory is usable");
            fs::set_permissions(&full, fs::Permissions::from_mode(0o444))
                .expect("the temporary directory is usable");
        }
        for dir in walk_dirs(payload.path()) {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755))
                .expect("the temporary directory is usable");
        }
        self.meta.installed_size = self.files.iter().map(|(_, c)| c.len() as u64).sum();
        let entries = scan(payload.path()).expect("test packages are valid");
        let meta_json = self.meta.to_json().expect("test packages are valid");
        let manifest = Manifest::new(self.meta.identity(), Blake3Hash::of(&meta_json), entries)
            .expect("test packages are valid");
        let mut bytes = Vec::new();
        write_package(
            &mut bytes,
            payload.path(),
            &self.meta,
            &manifest,
            Compression::DEVELOPMENT,
        )
        .expect("the temporary directory is usable");
        let identity = self.meta.identity();
        let file = out.join(identity.file_name());
        fs::write(&file, &bytes).expect("the temporary directory is usable");
        if let Some(flag) = self.buildinfo {
            let buildinfo = serde_json::json!({
                "format": 1,
                "package": {"name": self.meta.name, "version": self.meta.version, "rev": self.meta.rev, "variant": self.meta.variant},
                "recipe_repos": [{"name": "core", "commit": "4be1", "dirty": flag == "dirty"}],
                "outputs": [{"name": self.meta.name, "variant": self.meta.variant, "id": manifest.store_id(), "file": identity.file_name()}],
            });
            let name = format!("{identity}.buildinfo.json");
            fs::write(
                out.join(name),
                serde_json::to_vec(&buildinfo).expect("buildinfo serializes"),
            )
            .expect("the temporary directory is usable");
        }
        file
    }
}

fn walk_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(root).expect("the temporary directory is usable") {
        let path = entry.expect("the payload directory is readable").path();
        if path.is_dir() {
            dirs.extend(walk_dirs(&path));
            dirs.push(path);
        }
    }
    dirs
}

/// The index entry of the package file `path`.
pub(crate) fn entry(path: &Path) -> IndexEntry {
    let bytes = fs::read(path).expect("the temporary directory is usable");
    let reader = Reader::new(bytes.as_slice()).expect("test packages are valid");
    let id = reader.manifest().store_id();
    IndexEntry {
        meta: reader.meta().clone(),
        id,
        file: format!("pool/{id}.lpkg"),
        size: bytes.len() as u64,
        blake3: Blake3Hash::of(&bytes),
    }
}

/// Writes a repository by hand, without any check: the package files go
/// into the pool, their buildinfo next to them, and an unsigned index of
/// `channel` with generation `T0 + offset`, after `edit` changed it, becomes
/// the entry point and a snapshot.
pub(crate) fn fixture(
    repo: &Path,
    channel: &str,
    offset: u64,
    packages: &[PathBuf],
    edit: impl FnOnce(&mut Index),
) -> Index {
    let mut index = Index {
        format: Index::FORMAT,
        repo: "core".parse().expect("test names are valid"),
        channel: channel.parse().expect("test names are valid"),
        arch: Arch::X86_64V3,
        generation: T0 + offset,
        timestamp: Timestamp::from_secs(T0 + offset),
        expires: Timestamp::from_secs(T0 + offset + 14 * DAY),
        defaults: Defaults::default(),
        packages: vec![],
        yanked: vec![],
    };
    for path in packages {
        let entry = entry(path);
        fs::create_dir_all(repo.join("pool")).expect("the temporary directory is usable");
        fs::copy(path, repo.join(&entry.file)).expect("the temporary directory is usable");
        let buildinfo = path.with_extension("").with_extension("");
        let buildinfo = PathBuf::from(format!("{}.buildinfo.json", buildinfo.to_string_lossy()));
        if buildinfo.exists() {
            fs::create_dir_all(repo.join("buildinfo")).expect("the temporary directory is usable");
            fs::copy(
                &buildinfo,
                repo.join(format!("buildinfo/{}.json", entry.id)),
            )
            .expect("the temporary directory is usable");
        }
        index.packages.push(entry);
    }
    index.packages.sort_by(|a, b| {
        let key = |e: &IndexEntry| {
            (
                e.meta.name.clone(),
                Reverse((e.meta.version.clone(), e.meta.rev)),
                e.meta.variant,
            )
        };
        key(a).cmp(&key(b))
    });
    edit(&mut index);
    write_index(repo, &index);
    index
}

/// Writes `index` unsigned as the entry point of its channel and as its
/// snapshot.
pub(crate) fn write_index(repo: &Path, index: &Index) {
    let dir = repo.join(format!("channels/{}/{}", index.channel, index.arch));
    fs::create_dir_all(dir.join("snapshots")).expect("the temporary directory is usable");
    let bytes = index.encode().expect("test indexes are valid");
    fs::write(dir.join("index.json.zst"), &bytes).expect("the temporary directory is usable");
    fs::write(
        dir.join(format!("snapshots/{}.json.zst", index.generation)),
        &bytes,
    )
    .expect("the temporary directory is usable");
}

/// Reads the entry point of `channel` in `repo`.
pub(crate) fn read_index(repo: &Path, channel: &str) -> Index {
    let bytes = fs::read(repo.join(format!("channels/{channel}/x86_64-v3/index.json.zst")))
        .expect("the temporary directory is usable");
    Index::decode(&bytes).expect("test indexes are valid")
}

/// Runs `laft-repo` with `args`, never prompting for a password.
pub(crate) fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_laft-repo"))
        .args(args)
        .env_remove("LAFT_SIGNING_KEY_PASSWORD")
        .stdin(Stdio::null())
        .output()
        .expect("the laft-repo binary runs")
}

/// Runs `laft-repo` and returns its standard output, failing the test
/// unless it succeeds.
pub(crate) fn ok(args: &[&str]) -> String {
    let output = run(args);
    assert!(
        output.status.success(),
        "laft-repo {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("laft-repo prints UTF-8")
}

/// Runs `laft-repo` and returns its standard output and error, failing the
/// test unless it exits with status 1.
pub(crate) fn fails(args: &[&str]) -> (String, String) {
    let output = run(args);
    assert_eq!(
        output.status.code(),
        Some(1),
        "laft-repo {args:?} did not fail:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).expect("laft-repo prints UTF-8"),
        String::from_utf8(output.stderr).expect("laft-repo prints UTF-8"),
    )
}

/// Creates an unencrypted key pair `<dir>/<name>.key` and `.pub` and
/// returns the secret key path.
pub(crate) fn keygen(dir: &Path, name: &str) -> PathBuf {
    let prefix = dir.join(name);
    ok(&["keygen", "--unencrypted", "--out", s(&prefix)]);
    prefix.with_extension("key")
}

/// The public key of the secret key file `key`, read from the `.pub` file
/// next to it.
pub(crate) fn public(key: &Path) -> laft_index::PublicKey {
    let text = fs::read_to_string(key.with_extension("pub")).expect("keygen wrote the public key");
    let line = text
        .lines()
        .last()
        .expect("a public key file has a key line");
    line.parse().expect("keygen writes valid public keys")
}

/// The path as a string argument.
pub(crate) fn s(path: &Path) -> &str {
    path.to_str().expect("temporary paths are UTF-8")
}
