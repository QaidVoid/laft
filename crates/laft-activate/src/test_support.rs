//! Roots, accounts and generations shared by the unit tests. Everything
//! happens in a temporary directory.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use laft_store::{Lock, Root};
use laft_test::TempDir;

use crate::{Group, User, Users};

/// A temporary root.
pub(crate) struct Env {
    _dir: TempDir,
    pub(crate) root: Root,
}

impl Env {
    pub(crate) fn new() -> Self {
        let dir = laft_test::tempdir();
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

/// The services the base part of the system configuration declares, which
/// a test of the configuration renders from the base part itself.
pub(crate) fn standard_services() -> BTreeMap<String, laft_svc::Service> {
    serde_json::from_str(include_str!(
        "../../laft-svc/tests/fixtures/standard-services.json"
    ))
    .expect("the standard services fixture deserializes")
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
    let tree =
        crate::render(config, &declared, &root.store(), &[], &BTreeMap::new()).expect("renders");
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

/// A package to build into a real package file.
pub(crate) struct Pkg {
    name: String,
    version: String,
    files: Vec<(String, Vec<u8>)>,
    triggers: Vec<String>,
}

/// A built package file.
pub(crate) struct Built {
    pub(crate) path: PathBuf,
    pub(crate) id: laft_core::StoreId,
    pub(crate) dir: String,
    pub(crate) name: String,
}

impl Pkg {
    pub(crate) fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            files: Vec::new(),
            triggers: Vec::new(),
        }
    }

    /// Adds a file. Files below `usr/bin` and `usr/lib/laft`, and the
    /// program loader, are executable.
    pub(crate) fn file(mut self, path: &str, content: &[u8]) -> Self {
        self.files.push((path.into(), content.into()));
        self
    }

    /// Declares that the payload activates `trigger`.
    pub(crate) fn trigger(mut self, trigger: &str) -> Self {
        self.triggers.push(trigger.into());
        self.triggers.sort();
        self
    }

    /// Writes the package file into `dir`.
    pub(crate) fn build(&self, dir: &Path) -> Built {
        use std::os::unix::fs::PermissionsExt;

        use laft_core::Blake3Hash;
        use laft_pkg::{Arch, Compression, Manifest, Meta, scan, write_package};

        let payload = dir.join(format!("payload-{}-{}", self.name, self.version));
        let _ = fs::remove_dir_all(&payload);
        fs::create_dir_all(&payload).expect("can create the payload");
        for (path, content) in &self.files {
            let full = payload.join(path);
            fs::create_dir_all(full.parent().expect("has a parent")).expect("can create dirs");
            fs::write(&full, content).expect("can write a payload file");
            let executable = path.starts_with("usr/bin/")
                || path.starts_with("usr/lib/laft/")
                || path.contains("/ld-linux");
            let mode = if executable { 0o555 } else { 0o444 };
            fs::set_permissions(&full, fs::Permissions::from_mode(mode)).expect("can chmod");
        }
        let entries: Vec<_> = scan(&payload)
            .expect("the payload scans")
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
            alternatives: vec![],
            triggers: self
                .triggers
                .iter()
                .map(|t| t.parse().expect("valid trigger"))
                .collect(),
            static_deps: vec![],
            abi: Default::default(),
            abi_epoch: 0,
            requested_privileges: vec![],
            installed_size: size,
        };
        let meta_hash = Blake3Hash::of(&meta.to_json().expect("valid metadata"));
        let manifest = Manifest::new(meta.identity(), meta_hash, entries).expect("valid manifest");
        let path = dir.join(manifest.package().file_name());
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
        Built {
            path,
            id,
            dir: id.dir_name(&package.name, &package.version, package.variant),
            name: self.name.clone(),
        }
    }
}

/// A loader cache in the format of glibc 2.32 and later that maps
/// `libc.so.6` to `/usr/lib/libc.so.6`, because the host's loader may only
/// search directories other than `/usr/lib` on its own.
fn loader_cache() -> Vec<u8> {
    let strings = b"libc.so.6\0/usr/lib/libc.so.6\0";
    let header = 48u32;
    let entries = 24u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"glibc-ld.so.cache1.1");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    out.extend_from_slice(&[2, 0, 0, 0]);
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(&0x0303i32.to_le_bytes());
    out.extend_from_slice(&(header + entries).to_le_bytes());
    out.extend_from_slice(&(header + entries + 10).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(strings);
    out
}

/// The host files that make up a minimal shell: `dash` with the C library
/// and loader it needs, plus a loader cache, as a package `shell` with
/// `extra` added. `None` when the host lacks them.
pub(crate) fn shell_package(extra: &[(&str, &[u8])]) -> Option<Pkg> {
    let mut package = Pkg::new("shell", "1").file("etc/ld.so.cache", &loader_cache());
    for (host, inside) in [
        ("/usr/bin/dash", "usr/bin/sh"),
        (
            "/lib64/ld-linux-x86-64.so.2",
            "usr/lib/ld-linux-x86-64.so.2",
        ),
        ("/usr/lib/libc.so.6", "usr/lib/libc.so.6"),
    ] {
        package = package.file(inside, &fs::read(host).ok()?);
    }
    for (path, content) in extra {
        package = package.file(path, content);
    }
    Some(package)
}

/// Why this environment cannot run a shell from a minimal tree in the
/// fixed program sandbox, or `None` when it can. Checked once.
pub(crate) fn sandbox_unavailable() -> Option<String> {
    use std::sync::OnceLock;

    static REASON: OnceLock<Option<String>> = OnceLock::new();
    REASON
        .get_or_init(|| {
            let Some(shell) = shell_package(&[]) else {
                return Some("the host has no dash and C library to build a shell from".into());
            };
            let dir = laft_test::tempdir();
            for (path, content) in &shell.files {
                let full = dir.path().join("tree").join(path);
                fs::create_dir_all(full.parent().expect("has a parent")).expect("can create dirs");
                laft_test::write_program(&full, content);
            }
            fs::create_dir_all(dir.path().join("tree/etc")).expect("can create etc");
            let sandbox = laft_sandbox::FixedSandbox::new(
                dir.path().join("tree"),
                dir.path().join("tree/etc"),
            );
            match sandbox
                .command("/usr/bin/sh")
                .args(["-c", "exit 0"])
                .output()
            {
                Ok(output) if output.status.success() => None,
                Ok(output) => Some(format!(
                    "the host's shell does not run from a minimal tree: {}",
                    String::from_utf8_lossy(&output.stderr)
                )),
                Err(error) => Some(format!("the fixed program sandbox is unavailable: {error}")),
            }
        })
        .clone()
}

/// A minimal ELF relocatable object, as a kernel module is, whose
/// `.modinfo` section holds `modinfo`.
pub(crate) fn module_elf(modinfo: &[u8]) -> Vec<u8> {
    let names = b"\0.modinfo\0.shstrtab\0";
    let modinfo_at = 64u64;
    let names_at = modinfo_at + modinfo.len() as u64;
    let headers_at = (names_at + names.len() as u64).div_ceil(8) * 8;
    let mut out = Vec::new();
    out.extend_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0]);
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&62u16.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&headers_at.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for half in [64u16, 0, 0, 64, 3, 2] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    out.extend_from_slice(modinfo);
    out.extend_from_slice(names);
    out.resize(headers_at as usize, 0);
    let section = |out: &mut Vec<u8>, name: u32, kind: u32, flags: u64, at: u64, size: u64| {
        out.extend_from_slice(&name.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        for word in [flags, 0, at, size] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
    };
    out.extend_from_slice(&[0; 64]);
    section(&mut out, 1, 1, 2, modinfo_at, modinfo.len() as u64);
    section(&mut out, 10, 3, 0, names_at, names.len() as u64);
    out
}

/// `bytes` compressed with zstd.
pub(crate) fn zstd(bytes: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(bytes, 3).expect("compresses")
}

/// Every entry of the newc cpio archives concatenated in `bytes`, by path:
/// the mode and the data.
pub(crate) fn read_cpio(bytes: &[u8]) -> BTreeMap<String, (u32, Vec<u8>)> {
    let field = |at: usize, index: usize| {
        let start = at + 6 + index * 8;
        u32::from_str_radix(
            std::str::from_utf8(&bytes[start..start + 8]).expect("ASCII"),
            16,
        )
        .expect("hex")
    };
    let mut entries = BTreeMap::new();
    let mut at = 0;
    while at < bytes.len() {
        assert_eq!(&bytes[at..at + 6], b"070701", "a newc header at {at}");
        let size = field(at, 6) as usize;
        let name_len = field(at, 11) as usize;
        let name = std::str::from_utf8(&bytes[at + 110..at + 110 + name_len - 1])
            .expect("UTF-8 name")
            .to_owned();
        let data_at = (at + 110 + name_len).div_ceil(4) * 4;
        let data = bytes[data_at..data_at + size].to_vec();
        let mode = field(at, 1);
        at = (data_at + size).div_ceil(4) * 4;
        if name != "TRAILER!!!" {
            entries.insert(name, (mode, data));
        }
    }
    entries
}
