//! The packages of the test system and the signed repository that serves
//! them.
//!
//! busybox, runit, the kernel and Limine are built from the recipes in
//! `tests/vm/recipes` by `laft-build` in bootstrap mode, with the host's
//! tools. Their sources are pinned there by URL and SHA-256, and
//! `laft-build` verifies every download before it uses it. The self-built
//! system needs only busybox from there, for the installer, and publishes
//! [`SELF_BUILT_PACKAGES`] from the packages the bootstrap made instead.
//! Usable mode adds [`USABLE_PACKAGES`]. The Laft binaries are packaged from
//! the static release build of this workspace.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use laft_core::Blake3Hash;
use laft_pkg::{Alternative, Arch, Compression, Manifest, Meta, Reader, scan, write_package};

use crate::system::{SELF_BUILT_PACKAGES, System, USABLE_PACKAGES};
use crate::{Paths, Result, fail, run};

/// The packages `laft-build` makes, in build order.
const RECIPES: [&str; 4] = ["limine", "runit:static", "busybox:static", "linux"];

/// The Laft programs and where the `laft` package puts them.
const PROGRAMS: [(&str, &str); 4] = [
    ("laft", "usr/bin/laft"),
    ("laft-build", "usr/bin/laft-build"),
    ("laft-enter", "usr/bin/laft-enter"),
    ("laft-boot", "usr/lib/laft/laft-boot"),
];

/// The tools of the cross compiler for the `static` variant, which the
/// host names with a different triple.
const CROSS_TOOLS: [&str; 12] = [
    "gcc", "g++", "cpp", "ar", "as", "ld", "nm", "objcopy", "objdump", "ranlib", "readelf", "strip",
];

/// Builds the packages of `system` and publishes them as a signed
/// repository in `www`. Returns the public key line of the repository.
///
/// # Errors
///
/// Fails when a build, the packaging or the repository tool fails, or when
/// a package of the self-built system is missing.
pub(crate) fn build(paths: &Paths, system: &System) -> Result<String> {
    let bootstrap = bootstrap_root(&paths.cache)?;
    let build_root = paths.cache.join("build-root");
    let recipes: &[&str] = match system {
        System::Minimal => &RECIPES,
        System::SelfBuilt(_) | System::Usable(_) => &["busybox:static"],
    };
    let mut command = Command::new(paths.bin.join("laft-build"));
    command
        .arg("--root")
        .arg(&build_root)
        .arg("--recipes")
        .arg(paths.source.join("recipes"))
        .arg("bootstrap")
        .arg("--target")
        .arg(&bootstrap)
        .args(["--jobs", "4"])
        .args(recipes);
    run(&mut command)?;
    // Runs before the dedicated scenario roots existed carried them left
    // old builds of the scenario packages in the shared build root, where
    // they would enter every index through `published`.
    if let Ok(entries) = paths.build_out().read_dir() {
        let stale = ["greeting-", "libgreet-", "hello-"];
        for item in entries.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if stale.iter().any(|prefix| name.starts_with(prefix)) {
                let _ = fs::remove_file(item.path());
            }
        }
    }
    let published = match system.self_built() {
        Some(dir) => select(dir, &paths.work.join("self-built"), system)?,
        None => paths.build_out(),
    };

    let laft_out = paths.work.join("packages");
    let _ = fs::remove_dir_all(&laft_out);
    laft_package(&paths.bin, &laft_out)?;
    let stubs = paths.work.join("stubs");
    let _ = fs::remove_dir_all(&stubs);
    build_view_stubs(&stubs)?;
    let (defaults, loud) = scenario_builds(paths, &stubs)?;

    let keys = paths.work.join("keys");
    let repo = paths.work.join("repo");
    let www = paths.www();
    for dir in [&keys, &repo, &www] {
        let _ = fs::remove_dir_all(dir);
    }
    fs::create_dir_all(&keys).map_err(|e| fail(format!("{}: {e}", keys.display())))?;
    let key = keys.join("online");
    let repo_tool = || {
        let mut command = Command::new(paths.bin.join("laft-repo"));
        command.env_remove("LAFT_SIGNING_KEY_PASSWORD");
        command
    };
    run(repo_tool()
        .args(["keygen", "--unencrypted", "--out"])
        .arg(&key))?;
    let recipes = paths.source.join("recipes");
    // The plain index serves the installed system. The profile index of
    // the option scenario sits beside it and carries the same snapshot,
    // so both machines can build from it.
    run(repo_tool()
        .args(["index", "--repo"])
        .arg(&repo)
        .args([
            "--name",
            "core",
            "--channel",
            "current",
            "--allow-bootstrap",
            "--key",
        ])
        .arg(key.with_extension("key"))
        .arg("--recipes")
        .arg(&recipes)
        .arg(&published)
        .arg(&laft_out)
        .arg(&stubs)
        .arg(&defaults))?;
    run(repo_tool()
        .args(["index", "--repo"])
        .arg(&repo)
        .args([
            "--name",
            "core",
            "--channel",
            "current",
            "--allow-bootstrap",
            "--key",
        ])
        .arg(key.with_extension("key"))
        .arg("--recipes")
        .arg(&recipes)
        .arg("--profile")
        .arg("loud")
        .arg(&published)
        .arg(&laft_out)
        .arg(&stubs)
        .arg(&loud))?;
    run(repo_tool().arg("publish").arg(&repo).arg(&www))?;
    let public = key.with_extension("pub");
    let text =
        fs::read_to_string(&public).map_err(|e| fail(format!("{}: {e}", public.display())))?;
    text.lines()
        .last()
        .map(str::to_owned)
        .ok_or_else(|| fail(format!("{} has no key line", public.display())))
}

/// Links the package files of [`SELF_BUILT_PACKAGES`] and, in usable mode,
/// [`USABLE_PACKAGES`] from `dir` into a new directory `to`, with every
/// buildinfo file of `dir`, and returns `to`. A file on another filesystem
/// is copied instead.
fn select(dir: &Path, to: &Path, system: &System) -> Result<PathBuf> {
    let _ = fs::remove_dir_all(to);
    fs::create_dir_all(to).map_err(|e| fail(format!("{}: {e}", to.display())))?;
    let files = package_files(dir)?;
    let mut chosen = Vec::new();
    let extra = if matches!(system, System::Usable(_)) {
        USABLE_PACKAGES
    } else {
        &[]
    };
    for name in SELF_BUILT_PACKAGES.iter().chain(extra) {
        chosen.push(one(dir, &files, name)?.0.clone());
    }
    for item in fs::read_dir(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))? {
        let path = item
            .map_err(|e| fail(format!("{}: {e}", dir.display())))?
            .path();
        if path.to_string_lossy().ends_with(".buildinfo.json") {
            chosen.push(path);
        }
    }
    for from in chosen {
        let target = to.join(from.file_name().expect("a listed file has a name"));
        fs::hard_link(&from, &target)
            .or_else(|e| match e.kind() {
                ErrorKind::CrossesDevices => fs::copy(&from, &target).map(drop),
                _ => Err(e),
            })
            .map_err(|e| fail(format!("{} to {}: {e}", from.display(), target.display())))?;
    }
    Ok(to.to_path_buf())
}

/// Every package file in `dir` with its metadata, by package name.
fn package_files(dir: &Path) -> Result<BTreeMap<String, Vec<(PathBuf, Meta)>>> {
    let mut found: BTreeMap<String, Vec<(PathBuf, Meta)>> = BTreeMap::new();
    for item in fs::read_dir(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))? {
        let path = item
            .map_err(|e| fail(format!("{}: {e}", dir.display())))?
            .path();
        if path.extension().is_none_or(|e| e != "lpkg") {
            continue;
        }
        let meta = fs::File::open(&path)
            .map_err(|e| e.to_string())
            .and_then(|f| Reader::new(f).map_err(|e| e.to_string()))
            .map(|r| r.meta().clone())
            .map_err(|e| fail(format!("{}: {e}", path.display())))?;
        found
            .entry(meta.name.to_string())
            .or_default()
            .push((path, meta));
    }
    Ok(found)
}

/// The one package file named `name` of `files`, which `dir` holds.
fn one<'a>(
    dir: &Path,
    files: &'a BTreeMap<String, Vec<(PathBuf, Meta)>>,
    name: &str,
) -> Result<(&'a PathBuf, &'a Meta)> {
    match files.get(name).map(Vec::as_slice) {
        Some([(file, meta)]) => Ok((file, meta)),
        found => Err(fail(format!(
            "{} has {} packages named {name}, not one",
            dir.display(),
            found.map_or(0, <[_]>::len)
        ))),
    }
}

/// The one package file in `dir` whose package is named `name`.
///
/// # Errors
///
/// Fails when `dir` cannot be read or holds no or several such files.
pub(crate) fn package_file(dir: &Path, name: &str) -> Result<PathBuf> {
    let files = package_files(dir)?;
    one(dir, &files, name).map(|(file, _)| file.clone())
}

/// The stub packages of the build view that local builds in the machine
/// resolve against: every base package of `laft-build` as an empty
/// package, except dash and coreutils, which carry the host's shell and
/// the standalone tools the recipes' phases run. A build in the machine
/// seeds them from this repository instead of building them.
///
/// The payload files are host files, and the machine runs them inside
/// its build views: it is the same architecture, and its kernel is newer
/// than the oldest one the host libraries support.
pub(crate) fn build_view_stubs(out: &Path) -> Result<()> {
    let files = view_files()?;
    for name in laft_build::view::BUILD_BASE {
        let summary = match *name {
            "dash" => "The shell of the build views of the machine",
            "coreutils" => "The tools of the build views of the machine",
            "glibc" => "The libraries of the build views of the machine",
            _ => "A stub of the build view of the option scenario",
        };
        let own: Vec<_> = files
            .iter()
            .filter(|(path, _)| match *name {
                "dash" => path.as_str() == "usr/bin/sh",
                "coreutils" => {
                    matches!(
                        path.as_str(),
                        "usr/bin/cp" | "usr/bin/mkdir" | "usr/bin/printf"
                    )
                }
                "glibc" => path.starts_with("usr/lib/"),
                _ => false,
            })
            .map(|(path, (bytes, mode))| (path.clone(), bytes.clone(), *mode))
            .collect();
        stub_package(out, name, summary, &own)?;
    }
    Ok(())
}

/// The payload of the machine's build views, as one map: the host's
/// `/bin/sh` and the tools the recipes' phases run, each with the shared
/// libraries they need. `build_view_stubs` splits the map between the
/// dash, coreutils and glibc stubs, so that no path belongs to two of
/// them. Every file keeps the executable bit of its host file, and the
/// recipes that run in a view set `LD_LIBRARY_PATH` to `/usr/lib`, so the
/// loader finds the libraries without `ld.so.cache`.
fn view_files() -> Result<BTreeMap<String, (Vec<u8>, u32)>> {
    let mut files: BTreeMap<String, (Vec<u8>, u32)> = BTreeMap::new();
    let host =
        fs::canonicalize("/bin/sh").map_err(|e| fail(format!("the host has no /bin/sh: {e}")))?;
    let sh = fs::read(&host).map_err(|e| fail(format!("{}: {e}", host.display())))?;
    files.insert("usr/bin/sh".to_owned(), (sh, host_mode(&host)?));
    library_closure(&host, &mut files)?;
    for tool in ["cp", "mkdir", "printf"] {
        let host = Path::new("/usr/bin").join(tool);
        let bytes = fs::read(&host).map_err(|e| fail(format!("{}: {e}", host.display())))?;
        let mode = host_mode(&host)?;
        files.insert(format!("usr/bin/{tool}"), (bytes, mode));
        library_closure(&host, &mut files)?;
    }
    Ok(files)
}

/// The alternative groups a stub joins: `sh` and `coreutils` share their
/// files with the busybox of the repository.
fn stub_alternatives(name: &str) -> Result<Vec<Alternative>> {
    let paths = |paths: &[&str]| paths.iter().map(|p| (*p).to_owned()).collect();
    match name {
        "dash" => Ok(vec![Alternative {
            group: parse("sh")?,
            paths: paths(&["usr/bin/sh"]),
            priority: 10,
        }]),
        "coreutils" => Ok(vec![Alternative {
            group: parse("coreutils")?,
            paths: paths(&["usr/bin/cp", "usr/bin/mkdir", "usr/bin/printf"]),
            priority: 10,
        }]),
        _ => Ok(Vec::new()),
    }
}

/// Builds the packages of the option scenario twice into fresh build
/// roots, whose stores the stubs seed, and returns their output
/// directories: the declared defaults first, then the `loud` option set
/// of the repository profile. The default set gives the machine's
/// resolution something to differ from: an optioned configuration then
/// refuses with S011, `--build-missing` builds the optioned set from the
/// recipe snapshot, and the default configuration installs the published
/// packages without a build. The loud set is what the profiled index
/// may publish: a profile must not disagree with the builds it indexes.
fn scenario_builds(paths: &Paths, stubs: &Path) -> Result<(PathBuf, PathBuf)> {
    let mut files: Vec<PathBuf> = fs::read_dir(stubs)
        .map_err(|e| fail(format!("{}: {e}", stubs.display())))?
        .flatten()
        .map(|item| item.path())
        .filter(|path| path.extension().is_some_and(|e| e == "lpkg"))
        .collect();
    files.sort();
    let build = |root: &Path, set: &[&str]| {
        // The store entries carry read-only directories, which plain
        // removal cannot unlink.
        let _ = Command::new("chmod").args(["-R", "u+w"]).arg(root).status();
        let _ = fs::remove_dir_all(root);
        let command = |arguments: &[&str]| {
            let mut command = Command::new(paths.bin.join("laft-build"));
            command
                .arg("--root")
                .arg(root)
                .arg("--recipes")
                .arg(paths.source.join("recipes"))
                .args(set)
                .args(arguments);
            command
        };
        run(command(&["seed"]).args(&files))?;
        run(command(&["build"]).args(["greeting", "libgreet", "hello"]))
    };
    let defaults = paths.work.join("defaults-root");
    build(&defaults, &[])?;
    let loud = paths.work.join("loud-root");
    build(
        &loud,
        &[
            "--set",
            "greeting.mood=loud",
            "--set",
            "greeting.twice=true",
            "--set",
            "libgreet.shout=true",
        ],
    )?;
    Ok((defaults.join("out"), loud.join("out")))
}

/// Writes the empty package `name`, with `files` in the payload, into
/// `out`. Each file keeps the mode it carries, so executable files stay
/// executable inside a build view.
fn stub_package(
    out: &Path,
    name: &str,
    summary: &str,
    files: &[(String, Vec<u8>, u32)],
) -> Result<()> {
    let payload = out.join("payload");
    let _ = fs::remove_dir_all(&payload);
    fs::create_dir_all(&payload).map_err(|e| fail(format!("{}: {e}", payload.display())))?;
    let mut size = 0u64;
    for (path, bytes, mode) in files {
        let full = payload.join(path);
        let parent = full.parent().expect("a payload path has a parent");
        fs::create_dir_all(parent).map_err(|e| fail(format!("{}: {e}", parent.display())))?;
        fs::write(&full, bytes).map_err(|e| fail(format!("{}: {e}", full.display())))?;
        fs::set_permissions(&full, fs::Permissions::from_mode(*mode))
            .map_err(|e| fail(format!("{}: {e}", full.display())))?;
        size += bytes.len() as u64;
    }
    let entries = scan(&payload).map_err(|e| fail(format!("scanning the payload: {e}")))?;
    let meta = Meta {
        format: laft_pkg::FORMAT,
        name: parse(name)?,
        version: parse("1.0")?,
        rev: 1,
        variant: laft_core::Variant::Dynamic,
        arch: Arch::X86_64V3,
        origin: parse(name)?,
        summary: summary.to_owned(),
        homepage: None,
        license: vec!["MIT-0".into()],
        depends: vec![],
        provides: vec![],
        conflicts: vec![],
        replaces: vec![],
        alternatives: stub_alternatives(name)?,
        triggers: vec![],
        static_deps: vec![],
        abi: Default::default(),
        options: BTreeMap::new(),
        abi_epoch: 0,
        requested_privileges: vec![],
        installed_size: size,
    };
    let meta_json = meta
        .to_json()
        .map_err(|e| fail(format!("the stub metadata: {e}")))?;
    let manifest = Manifest::new(meta.identity(), Blake3Hash::of(&meta_json), entries)
        .map_err(|e| fail(format!("the stub manifest: {e}")))?;
    let identity = meta.identity();
    let mut bytes = Vec::new();
    write_package(
        &mut bytes,
        &payload,
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .map_err(|e| fail(format!("writing the stub {name}: {e}")))?;
    let file = out.join(identity.file_name());
    fs::write(&file, &bytes).map_err(|e| fail(format!("{}: {e}", file.display())))?;
    let buildinfo = serde_json::json!({
        "format": 1,
        "package": {"name": meta.name, "version": meta.version, "rev": meta.rev,
            "variant": meta.variant},
        "recipe_repos": [{"name": "vmtest", "commit": "0", "dirty": true}],
        "outputs": [{"name": meta.name, "variant": meta.variant,
            "id": manifest.store_id(), "file": identity.file_name()}],
    });
    let info = out.join(format!("{identity}.buildinfo.json"));
    fs::write(&info, buildinfo.to_string())
        .map_err(|e| fail(format!("{}: {e}", info.display())))?;
    fs::remove_dir_all(&payload).map_err(|e| fail(format!("{}: {e}", payload.display())))
}

/// Reads the ELF dependencies of `file`, transitively, into `files`, at
/// the payload paths the build views use: the interpreter and every
/// needed library under `usr/lib`, searched in the library directories
/// of the host.
fn library_closure(file: &Path, files: &mut BTreeMap<String, (Vec<u8>, u32)>) -> Result<()> {
    const LIBRARY_DIRS: [&str; 6] = [
        "/lib64",
        "/usr/lib64",
        "/lib",
        "/usr/lib",
        "/lib/x86_64-linux-gnu",
        "/usr/lib/x86_64-linux-gnu",
    ];
    let mut queue = vec![file.to_path_buf()];
    let mut seen = BTreeSet::new();
    while let Some(file) = queue.pop() {
        let bytes = fs::read(&file).map_err(|e| fail(format!("{}: {e}", file.display())))?;
        let Ok(Some(elf)) = laft_elf::inspect(&bytes) else {
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
            let bytes = fs::read(&dep).map_err(|e| fail(format!("{}: {e}", dep.display())))?;
            let host = dep.to_str().expect("a UTF-8 library path");
            let host = host.strip_prefix('/').expect("an absolute library path");
            let under = host.strip_prefix("usr/").unwrap_or(host);
            let rel = match under.split_once('/') {
                Some(("lib" | "lib64", tail)) => format!("usr/lib/{tail}"),
                _ => format!("usr/{under}"),
            };
            files.insert(rel, (bytes, host_mode(&dep)?));
            queue.push(dep);
        }
    }
    Ok(())
}

/// The mode to keep for a copy of `file`: executable when the host file
/// has an execute bit, read-only otherwise.
fn host_mode(file: &Path) -> Result<u32> {
    let mode = fs::metadata(file)
        .map_err(|e| fail(format!("{}: {e}", file.display())))?
        .permissions()
        .mode();
    if mode & 0o111 != 0 {
        Ok(0o555)
    } else {
        Ok(0o444)
    }
}

/// A bootstrap system whose `usr/bin` is the host's, with the cross tools
/// for the `static` variant under the triple `laft-build` uses.
fn bootstrap_root(cache: &Path) -> Result<PathBuf> {
    let root = cache.join("bootstrap-root");
    let bin = root.join("usr/bin");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&bin).map_err(|e| fail(format!("{}: {e}", bin.display())))?;
    let host = fs::read_dir("/usr/bin").map_err(|e| fail(format!("/usr/bin: {e}")))?;
    for item in host.flatten() {
        symlink(item.path(), bin.join(item.file_name()))
            .map_err(|e| fail(format!("{}: {e}", bin.display())))?;
    }
    for tool in CROSS_TOOLS {
        let host = PathBuf::from(format!("/usr/bin/x86_64-linux-musl-{tool}"));
        if !host.exists() {
            return Err(fail(format!(
                "{} is missing, and the static packages need it",
                host.display()
            )));
        }
        let link = bin.join(format!("x86_64-laft-linux-musl-{tool}"));
        let _ = fs::remove_file(&link);
        symlink(&host, &link).map_err(|e| fail(format!("{}: {e}", link.display())))?;
    }
    Ok(root)
}

/// Writes the `laft` package of the programs in `bin`, and its buildinfo,
/// into `out`.
fn laft_package(bin: &Path, out: &Path) -> Result<()> {
    let payload = out.join("payload");
    let mut size = 0;
    for (program, inside) in PROGRAMS {
        let target = payload.join(inside);
        let parent = target.parent().expect("a payload path has a parent");
        fs::create_dir_all(parent).map_err(|e| fail(format!("{}: {e}", parent.display())))?;
        let from = bin.join(program);
        size += fs::copy(&from, &target).map_err(|e| {
            fail(format!(
                "{}: {e}; build the static binaries first",
                from.display()
            ))
        })?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o555))
            .map_err(|e| fail(format!("{}: {e}", target.display())))?;
    }
    for dir in ["usr/bin", "usr/lib/laft", "usr/lib", "usr"] {
        let path = payload.join(dir);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    }
    let meta = Meta {
        format: laft_pkg::FORMAT,
        name: parse("laft")?,
        version: parse(env!("CARGO_PKG_VERSION"))?,
        rev: 1,
        variant: laft_core::Variant::Dynamic,
        arch: Arch::X86_64V3,
        origin: parse("laft")?,
        summary: "The Laft system tools".into(),
        homepage: None,
        license: vec!["Apache-2.0".into(), "MIT".into()],
        depends: vec![],
        provides: vec![],
        conflicts: vec![],
        replaces: vec![],
        alternatives: vec![],
        triggers: vec![],
        static_deps: vec![],
        abi: Default::default(),
        options: BTreeMap::new(),
        abi_epoch: 0,
        requested_privileges: vec![],
        installed_size: size,
    };
    let entries = scan(&payload).map_err(|e| fail(format!("scanning the payload: {e}")))?;
    let meta_json = meta
        .to_json()
        .map_err(|e| fail(format!("the laft metadata: {e}")))?;
    let manifest = Manifest::new(meta.identity(), Blake3Hash::of(&meta_json), entries)
        .map_err(|e| fail(format!("the laft manifest: {e}")))?;
    let identity = meta.identity();
    let mut bytes = Vec::new();
    write_package(
        &mut bytes,
        &payload,
        &meta,
        &manifest,
        Compression::DEVELOPMENT,
    )
    .map_err(|e| fail(format!("writing the laft package: {e}")))?;
    let file = out.join(identity.file_name());
    fs::write(&file, &bytes).map_err(|e| fail(format!("{}: {e}", file.display())))?;
    let buildinfo = serde_json::json!({
        "format": 1,
        "package": {"name": meta.name, "version": meta.version, "rev": meta.rev,
            "variant": meta.variant},
        "recipe_repos": [{"name": "vmtest", "commit": "0", "dirty": true}],
        "outputs": [{"name": meta.name, "variant": meta.variant,
            "id": manifest.store_id(), "file": identity.file_name()}],
    });
    let info = out.join(format!("{identity}.buildinfo.json"));
    fs::write(&info, buildinfo.to_string())
        .map_err(|e| fail(format!("{}: {e}", info.display())))?;
    fs::remove_dir_all(&payload).map_err(|e| fail(format!("{}: {e}", payload.display())))
}

fn parse<T: std::str::FromStr>(text: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    text.parse()
        .map_err(|e| fail(format!("{text:?} does not parse: {e}")))
}
