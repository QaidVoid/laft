//! Roots, accounts and generations shared by the unit tests. Everything
//! happens in a temporary directory.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use laft_store::{Lock, Root};
use tempfile::TempDir;

use crate::{Group, User, Users};

/// A temporary root.
pub(crate) struct Env {
    _dir: TempDir,
    pub(crate) root: Root,
}

impl Env {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("can create a temporary directory");
        let root = Root::new(dir.path());
        fs::create_dir_all(dir.path().join("etc")).expect("can create etc");
        Self { _dir: dir, root }
    }

    pub(crate) fn lock(&self) -> Lock {
        self.root.lock("test").expect("the lock is free")
    }

    /// The path of `relative` below the root.
    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    /// The text of `relative` below the root.
    pub(crate) fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path(relative)).expect("the file is readable")
    }

    /// Writes `text` to `relative` below the root, creating directories.
    pub(crate) fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().expect("has a parent")).expect("can create dirs");
        fs::write(path, text).expect("can write");
    }
}

/// `root`, `wheel`, `shadow`, `audio`, `users`, `sshd`, `nobody` and
/// `nogroup`, as the base part declares them.
pub(crate) fn base_users() -> Users {
    let mut users = Users::default();
    users.accounts.insert(
        "root".into(),
        User {
            uid: Some(0),
            home: Some("/root".into()),
            create_home: Some(false),
            ..User::default()
        },
    );
    users.accounts.insert("sshd".into(), system_user());
    users.accounts.insert(
        "nobody".into(),
        User {
            group: Some("nogroup".into()),
            ..system_user()
        },
    );
    for name in [
        "root", "wheel", "shadow", "audio", "users", "sshd", "nogroup",
    ] {
        users.groups.insert(
            name.into(),
            Group {
                system: name != "root",
                ..Group::default()
            },
        );
    }
    users
}

fn system_user() -> User {
    User {
        system: true,
        ..User::default()
    }
}

/// The base accounts plus `extra` regular users.
pub(crate) fn users_with(extra: &[(&str, User)]) -> Users {
    let mut users = base_users();
    for (name, user) in extra {
        users.accounts.insert((*name).to_owned(), user.clone());
    }
    users
}

/// Whether some line of `text` starts with `name:`.
pub(crate) fn has_entry(text: &str, name: &str) -> bool {
    text.lines().any(|l| l.starts_with(&format!("{name}:")))
}

/// The permission bits of `path`.
pub(crate) fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path)
        .expect("exists")
        .permissions()
        .mode()
        & 0o7777
}

/// Declares `users` for generation `number`, as assembly does, and stores
/// the declared accounts in the generation directory.
pub(crate) fn declare_generation(lock: &Lock, number: u64, users: &Users) -> crate::Declared {
    let declared = crate::declare(lock, users).expect("the accounts are valid");
    let dir = crate::state::gen_dir(lock.root(), number);
    fs::create_dir_all(&dir).expect("can create the generation");
    declared.write(&dir).expect("can write the accounts");
    declared
}

/// Creates a store entry for package `name` whose payload holds `files`,
/// and returns its directory name. Only what rendering reads is written:
/// the manifest and the files.
pub(crate) fn fake_package(root: &Root, name: &str, files: &[(&str, &str)]) -> String {
    use laft_core::Blake3Hash;
    use laft_pkg::{Arch, Entry, Identity, Manifest};

    let mut dirs = std::collections::BTreeSet::new();
    let mut entries = Vec::new();
    for (path, content) in files {
        let mut parent = Path::new(path).parent();
        while let Some(dir) = parent.filter(|d| !d.as_os_str().is_empty()) {
            dirs.insert(dir.to_string_lossy().into_owned());
            parent = dir.parent();
        }
        entries.push(Entry::File {
            path: (*path).to_owned(),
            mode: 0o444,
            size: content.len() as u64,
            hash: Blake3Hash::of(content.as_bytes()),
            owner: None,
            group: None,
        });
    }
    for dir in dirs {
        entries.push(Entry::Dir {
            path: dir,
            mode: 0o755,
            owner: None,
            group: None,
        });
    }
    let identity = Identity {
        name: name.parse().expect("valid name"),
        version: "1.0".parse().expect("valid version"),
        rev: 1,
        variant: laft_core::Variant::Dynamic,
        arch: Arch::Any,
    };
    let manifest =
        Manifest::new(identity.clone(), Blake3Hash::of(b"meta"), entries).expect("valid manifest");
    let dir = manifest
        .store_id()
        .dir_name(&identity.name, &identity.version, identity.variant);
    let entry = root.store().path(&dir);
    fs::create_dir_all(entry.join(".laft")).expect("can create the entry");
    fs::write(entry.join(".laft/manifest.json"), manifest.to_json()).expect("can write");
    for (path, content) in files {
        let file = entry.join(path);
        fs::create_dir_all(file.parent().expect("has a parent")).expect("can create dirs");
        fs::write(file, content).expect("can write");
    }
    dir
}

/// Renders every entry of `tree` as text: a header with the path and mode,
/// owner or link target, followed by the content of files.
pub(crate) fn dump(tree: &crate::EtcTree) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for (path, node) in tree.entries() {
        match node {
            crate::Node::Dir => {
                let _ = writeln!(out, "=== {path}/");
            }
            crate::Node::File {
                bytes,
                mode,
                uid,
                gid,
            } => {
                let text = String::from_utf8_lossy(bytes);
                let _ = write!(out, "=== {path} ({mode:o} {uid}:{gid})\n{text}");
                if !text.ends_with('\n') {
                    out.push_str("<no newline>\n");
                }
            }
            crate::Node::Linked(source) => {
                let name = source.file_name().unwrap_or_default().to_string_lossy();
                let _ = writeln!(out, "=== {path} (from the store: {name})");
            }
            crate::Node::Symlink(target) => {
                let _ = writeln!(out, "=== {path} -> {target}");
            }
        }
    }
    out
}

/// Builds generation `number` from `config` as assembly does: declares the
/// accounts, renders and writes the static tree, stores the declared
/// accounts and writes `gen.json` with the tree's record.
pub(crate) fn build_generation(lock: &Lock, number: u64, config: &crate::EtcConfig) {
    use laft_core::{Blake3Hash, Timestamp};
    use laft_store::{ConfigRecord, Generation, Init};

    let root = lock.root();
    let declared = crate::declare(lock, &config.users).expect("the accounts are valid");
    let tree = crate::render(config, &declared, &root.store(), &[]).expect("renders");
    let dir = crate::state::gen_dir(root, number);
    tree.write(&dir).expect("writes the tree");
    declared.write(&dir).expect("writes the accounts");
    let generation = Generation {
        format: 1,
        generation: number,
        parent: None,
        created: Timestamp::now(),
        message: None,
        laft_version: "0.1.0".into(),
        arch: "x86_64-v3".into(),
        init: Init {
            backend: "runit".into(),
            exec: vec!["/usr/bin/runit".into()],
        },
        config: ConfigRecord {
            hash: Blake3Hash::of(b"{}"),
            files: Vec::new(),
        },
        indexes: Vec::new(),
        packages: Vec::new(),
        initrd_packages: Vec::new(),
        alternatives: BTreeMap::new(),
        views: BTreeMap::new(),
        carried_modules: Vec::new(),
        etc: tree.record(),
    };
    let bytes = serde_json::to_vec(&generation).expect("serializes");
    fs::write(dir.join("gen.json"), bytes).expect("writes gen.json");
}

/// A configuration with the base accounts and `files`, each given as an
/// absolute path and its text.
pub(crate) fn config_with(files: &[(&str, &str)]) -> crate::EtcConfig {
    let mut config = crate::EtcConfig {
        users: base_users(),
        ..crate::EtcConfig::default()
    };
    for (path, text) in files {
        config.files.insert(
            (*path).to_owned(),
            crate::File {
                text: Some((*text).to_owned()),
                ..crate::File::default()
            },
        );
    }
    config
}

/// The target of the symlink `relative` below the root, if it is one.
pub(crate) fn link(env: &Env, relative: &str) -> Option<String> {
    fs::read_link(env.path(relative))
        .ok()
        .map(|t| t.to_string_lossy().into_owned())
}
