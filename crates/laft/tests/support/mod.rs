//! A temporary Laft root with a signed `file://` repository of test
//! packages, and a runner for the `laft` binary against it.

// Each test binary uses a different part of this module.
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use laft_core::Blake3Hash;
use laft_pkg::{Arch, Compression, Manifest, Meta, scan, write_package};
use laft_test::TempDir;

/// The kernel release of the test kernel.
pub(crate) const RELEASE: &str = "6.1.0-laft1";

/// A `depmod` that lists every module of the release in `modules.dep`,
/// with shell builtins only.
const DEPMOD: &[u8] = b"#!/bin/sh\n\
dir=/usr/lib/modules/$2\n\
for m in $dir/kernel/*/*.ko $dir/kernel/*/*/*.ko; do [ -e \"$m\" ] && echo \"${m#$dir/}:\"; done > $dir/modules.dep\n\
exit 0\n";

/// A package to build.
pub(crate) struct Pkg {
    meta: Meta,
    files: Vec<(String, Vec<u8>)>,
}

/// A package `name` at `version`, revision 1, without files.
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
            summary: format!("The {name} test package"),
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
    }
}

impl Pkg {
    /// Adds a file.
    pub(crate) fn file(mut self, path: &str, content: &[u8]) -> Self {
        self.files.push((path.into(), content.into()));
        self
    }

    /// Sets the variant.
    pub(crate) fn variant(mut self, variant: laft_core::Variant) -> Self {
        self.meta.variant = variant;
        self
    }

    /// Adds a dependency.
    pub(crate) fn depends(mut self, dep: &str) -> Self {
        self.meta
            .depends
            .push(dep.parse().expect("valid dependency"));
        self
    }

    /// Declares that the package activates `trigger`.
    pub(crate) fn trigger(mut self, trigger: &str) -> Self {
        self.meta
            .triggers
            .push(trigger.parse().expect("valid trigger"));
        self
    }

    /// Writes the package file and its buildinfo into `out`.
    pub(crate) fn write(mut self, out: &Path) -> PathBuf {
        fs::create_dir_all(out).expect("writable");
        let payload = laft_test::tempdir();
        for (path, content) in &self.files {
            let full = payload.path().join(path);
            fs::create_dir_all(full.parent().expect("has a parent")).expect("writable");
            fs::write(&full, content).expect("writable");
            let executable = path.starts_with("usr/bin/")
                || path.starts_with("usr/lib/laft/")
                || path.contains("/ld-linux");
            let mode = if executable { 0o555 } else { 0o444 };
            fs::set_permissions(&full, fs::Permissions::from_mode(mode)).expect("chmod");
        }
        set_dir_modes(payload.path());
        self.meta.installed_size = self.files.iter().map(|(_, c)| c.len() as u64).sum();
        let entries = scan(payload.path()).expect("the payload scans");
        let meta_json = self.meta.to_json().expect("valid metadata");
        let manifest = Manifest::new(self.meta.identity(), Blake3Hash::of(&meta_json), entries)
            .expect("valid manifest");
        let identity = self.meta.identity();
        let file = out.join(identity.file_name());
        let mut bytes = Vec::new();
        write_package(
            &mut bytes,
            payload.path(),
            &self.meta,
            &manifest,
            Compression::DEVELOPMENT,
        )
        .expect("the package writes");
        fs::write(&file, &bytes).expect("writable");
        let buildinfo = serde_json::json!({
            "format": 1,
            "package": {"name": self.meta.name, "version": self.meta.version,
                "rev": self.meta.rev, "variant": self.meta.variant},
            "recipe_repos": [{"name": "core", "commit": "4be1", "dirty": false}],
            "outputs": [{"name": self.meta.name, "variant": self.meta.variant,
                "id": manifest.store_id(), "file": identity.file_name()}],
        });
        fs::write(
            out.join(format!("{identity}.buildinfo.json")),
            serde_json::to_vec(&buildinfo).expect("serializes"),
        )
        .expect("writable");
        file
    }
}

fn set_dir_modes(dir: &Path) {
    for entry in fs::read_dir(dir).expect("readable") {
        let path = entry.expect("readable").path();
        if path.is_dir() {
            set_dir_modes(&path);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
}

/// A minimal relocatable ELF object with a `.modinfo` section, as a kernel
/// module is.
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

/// A loader cache in the format of glibc 2.32 and later that maps
/// `libc.so.6` to `/usr/lib/libc.so.6`.
fn loader_cache() -> Vec<u8> {
    let strings = b"libc.so.6\0/usr/lib/libc.so.6\0";
    let (header, entries) = (48u32, 24u32);
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

/// The shell package: the host's `dash` with its C library and loader, or
/// `None` when the host lacks them.
fn shell() -> Option<Pkg> {
    let mut package = pkg("shell", "1").file("etc/ld.so.cache", &loader_cache());
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
    Some(package)
}

/// The kernel package for release `release`.
pub(crate) fn linux(version: &str, release: &str) -> Pkg {
    pkg("linux", version)
        .file(&format!("usr/lib/modules/{release}/vmlinuz"), release.as_bytes())
        .file(
            &format!("usr/lib/modules/{release}/kernel/drivers/virtio_blk.ko"),
            &module_elf(b"license=GPL\0"),
        )
        .file(
            &format!("usr/lib/modules/{release}/modules.builtin"),
            b"kernel/fs/ext4/ext4.ko\nkernel/fs/vfat.ko\nkernel/fs/nls/nls_cp437.ko\nkernel/fs/nls/nls_iso8859-1.ko\n",
        )
        .trigger("depmod")
}

/// Why this environment cannot run the tests that assemble generations,
/// or `None` when it can. Assembly runs the `depmod` trigger, a shell
/// script, in a sandbox that needs user namespaces.
pub(crate) fn unavailable() -> Option<String> {
    static REASON: OnceLock<Option<String>> = OnceLock::new();
    REASON
        .get_or_init(|| {
            if shell().is_none() {
                return Some("the host has no dash and C library to build a shell from".into());
            }
            let status = Command::new("unshare")
                .args(["--user", "--map-root-user", "--mount", "true"])
                .stderr(Stdio::null())
                .status();
            match status {
                Ok(s) if s.success() => None,
                _ => Some("unprivileged user and mount namespaces are unavailable".into()),
            }
        })
        .clone()
}

/// The `laft-repo` binary, built on demand when the workspace build did
/// not build it next to `laft`.
fn laft_repo() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let laft = PathBuf::from(env!("CARGO_BIN_EXE_laft"));
        let path = laft.with_file_name("laft-repo");
        if !path.exists() {
            let status = Command::new(env!("CARGO"))
                .args(["build", "-p", "laft-repo"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .status()
                .expect("cargo runs");
            assert!(status.success(), "laft-repo builds");
        }
        path
    })
    .clone()
}

/// Runs `laft-repo` and fails the test unless it succeeds.
pub(crate) fn repo_tool(args: &[&str]) -> String {
    let output = Command::new(laft_repo())
        .args(args)
        .env_remove("LAFT_SIGNING_KEY_PASSWORD")
        .stdin(Stdio::null())
        .output()
        .expect("laft-repo runs");
    assert!(
        output.status.success(),
        "laft-repo {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8")
}

/// A [`Fixture`] with the base packages and the given ones, or a return
/// from the test after printing why the environment cannot assemble.
#[macro_export]
macro_rules! fixture {
    ($($package:expr),* $(,)?) => {
        match support::Fixture::new(vec![$($package),*]) {
            Ok(fixture) => fixture,
            Err(reason) => {
                println!("skipping: {reason}");
                return;
            }
        }
    };
}

/// A temporary directory with a repository, a published copy of it and a
/// Laft root.
pub(crate) struct Fixture {
    dir: TempDir,
    key: PathBuf,
}

impl Fixture {
    /// A fixture whose repository holds the base packages and `extra`, or
    /// why the environment cannot assemble generations.
    pub(crate) fn new(extra: Vec<Pkg>) -> Result<Self, String> {
        if let Some(reason) = unavailable() {
            return Err(reason);
        }
        let dir = laft_test::tempdir();
        let prefix = dir.path().join("keys/online");
        fs::create_dir_all(dir.path().join("keys")).expect("writable");
        repo_tool(&["keygen", "--unencrypted", "--out", s(&prefix)]);
        let mut packages = vec![
            shell().expect("checked"),
            pkg("kmod", "33").file("usr/bin/depmod", DEPMOD),
            pkg("laft", "0.1")
                .file("usr/lib/laft/laft-boot", b"laft-boot binary")
                .file("usr/bin/laft", b"laft binary"),
            pkg("busybox", "1.37").file("usr/bin/busybox", b"busybox"),
            pkg("busybox", "1.37")
                .variant(laft_core::Variant::Static)
                .file("usr/bin/busybox", b"static busybox"),
            linux("6.1.0", RELEASE),
        ];
        packages.extend(extra);
        let fixture = Self {
            dir,
            key: prefix.with_extension("key"),
        };
        fixture.publish(packages);
        fs::create_dir_all(fixture.root().join("boot")).expect("writable");
        fs::create_dir_all(fixture.root().join("etc/laft")).expect("writable");
        Ok(fixture)
    }

    /// The temporary directory.
    pub(crate) fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// The Laft root.
    pub(crate) fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    /// The `file://` URL of the published repository.
    pub(crate) fn url(&self) -> String {
        format!("file://{}", self.dir.path().join("www").display())
    }

    /// The public key line of the repository's signing key.
    pub(crate) fn public_key(&self) -> String {
        let text = fs::read_to_string(self.key.with_extension("pub")).expect("keygen wrote it");
        text.lines().last().expect("a key line").to_owned()
    }

    /// Builds `packages` and adds them to the repository as a new index
    /// generation, then publishes it. Index generations are numbered by
    /// the second they are signed in, so this waits for the next second
    /// after an earlier publication.
    pub(crate) fn publish(&self, packages: Vec<Pkg>) {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let out = self.dir.path().join(format!(
            "out-{}",
            COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        for package in packages {
            package.write(&out);
        }
        let repo = self.dir.path().join("repo");
        if repo.exists() {
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        repo_tool(&[
            "index",
            "--repo",
            s(&repo),
            "--name",
            "core",
            "--channel",
            "current",
            "--key",
            s(&self.key),
            s(&out),
        ]);
        repo_tool(&["publish", s(&repo), s(&self.dir.path().join("www"))]);
    }

    /// Writes the system configuration with `body` inside `system { }`,
    /// after the repository, the filesystems and the base packages.
    pub(crate) fn configure(&self, packages: &str, body: &str) {
        let text = format!(
            "system {{\n  hostname = \"forge\"\n  packages = [shell, kmod, laft, busybox{packages}]\n  \
             repo core {{ url = \"{}\", key = \"{}\" }}\n  \
             fs \"/\" {{ device = \"UUID=3f1c2a8e\", fstype = ext4, fsck = false }}\n  \
             fs \"/boot\" {{ device = \"UUID=1A2B-3C4D\", fstype = vfat }}\n  \
             boot {{ microcode = off }}\n{body}\n}}\n",
            self.url(),
            self.public_key()
        );
        fs::write(self.root().join("etc/laft/system.stav"), text).expect("writable");
    }

    /// Runs `laft --root <root> <args>`.
    pub(crate) fn laft(&self, args: &[&str]) -> Run {
        let root = self.root();
        let mut all = vec!["--root", s(&root)];
        all.extend_from_slice(args);
        run(&all)
    }

    /// The text of `/etc/<path>` as the system would see it: a managed
    /// symlink into `/etc/static` is followed into the default generation.
    pub(crate) fn etc(&self, path: &str) -> String {
        let live = self.root().join("etc").join(path);
        match fs::read_link(&live) {
            Ok(target) if target.starts_with("/etc/static") => {
                let default = fs::read_link(self.root().join("laft/gens/default"))
                    .expect("a default generation");
                let rel = target.strip_prefix("/etc/static").expect("checked");
                fs::read_to_string(
                    self.root()
                        .join("laft/gens")
                        .join(default)
                        .join("etc")
                        .join(rel),
                )
                .expect("readable")
            }
            _ => fs::read_to_string(live).expect("readable"),
        }
    }

    /// The text of `path` below the root.
    pub(crate) fn read(&self, path: &str) -> String {
        fs::read_to_string(self.root().join(path)).expect("readable")
    }
}

/// A finished run of `laft`.
pub(crate) struct Run {
    /// The exit status.
    pub(crate) code: i32,
    /// Standard output.
    pub(crate) stdout: String,
    /// Standard error.
    pub(crate) stderr: String,
}

impl Run {
    /// Fails the test unless the status is `code`.
    #[track_caller]
    pub(crate) fn expect(self, code: i32) -> Self {
        assert_eq!(
            self.code, code,
            "unexpected status {}\n--- stdout\n{}\n--- stderr\n{}",
            self.code, self.stdout, self.stderr
        );
        self
    }

    /// Standard output parsed as one JSON document.
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout is not one JSON document: {e}\n{}\n{}",
                self.stdout, self.stderr
            )
        })
    }

    /// Standard output parsed as JSON lines.
    pub(crate) fn events(&self) -> Vec<serde_json::Value> {
        self.stdout
            .lines()
            .map(|l| serde_json::from_str(l).expect("each line is JSON"))
            .collect()
    }
}

/// Runs `laft <args>` without a terminal and without color.
pub(crate) fn run(args: &[&str]) -> Run {
    let output: Output = Command::new(env!("CARGO_BIN_EXE_laft"))
        .args(args)
        .env("NO_COLOR", "1")
        .env("LANG", "C.UTF-8")
        .env_remove("LAFT_CONFIG")
        .env_remove("LAFT_COLOR")
        .stdin(Stdio::null())
        .output()
        .expect("laft runs");
    Run {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).expect("UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("UTF-8"),
    }
}

/// `path` as a string.
pub(crate) fn s(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}
