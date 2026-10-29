//! The packages of the test system and the signed repository that serves
//! them.
//!
//! busybox, runit, the kernel and Limine are built from the recipes in
//! `tests/vm/recipes` by `laft-build` in bootstrap mode, with the host's
//! tools. Their sources are pinned there by URL and SHA-256, and
//! `laft-build` verifies every download before it uses it. The Laft
//! binaries are packaged from the static release build of this workspace.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use laft_core::Blake3Hash;
use laft_pkg::{Arch, Compression, Manifest, Meta, scan, write_package};

use crate::{Paths, Result, fail, run};

/// The packages `laft-build` makes, in build order.
const RECIPES: [&str; 4] = ["limine", "runit:static", "busybox:static", "linux"];

/// The Laft programs and where the `laft` package puts them.
const PROGRAMS: [(&str, &str); 3] = [
    ("laft", "usr/bin/laft"),
    ("laft-enter", "usr/bin/laft-enter"),
    ("laft-boot", "usr/lib/laft/laft-boot"),
];

/// The tools of the cross compiler for the `static` variant, which the
/// host names with a different triple.
const CROSS_TOOLS: [&str; 12] = [
    "gcc", "g++", "cpp", "ar", "as", "ld", "nm", "objcopy", "objdump", "ranlib", "readelf", "strip",
];

/// Builds every package and publishes them as a signed repository in
/// `www`. Returns the public key line of the repository.
///
/// # Errors
///
/// Fails when a build, the packaging or the repository tool fails.
pub(crate) fn build(paths: &Paths) -> Result<String> {
    let bootstrap = bootstrap_root(&paths.cache)?;
    let build_root = paths.cache.join("build-root");
    let mut command = Command::new(paths.bin.join("laft-build"));
    command
        .arg("--root")
        .arg(&build_root)
        .arg("--recipes")
        .arg(paths.source.join("recipes"))
        .arg("bootstrap")
        .arg("--root")
        .arg(&bootstrap)
        .args(["--jobs", "4"])
        .args(RECIPES);
    run(&mut command)?;

    let laft_out = paths.work.join("packages");
    let _ = fs::remove_dir_all(&laft_out);
    laft_package(&paths.bin, &laft_out)?;

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
        .arg(build_root.join("out"))
        .arg(&laft_out))?;
    run(repo_tool().arg("publish").arg(&repo).arg(&www))?;
    let public = key.with_extension("pub");
    let text =
        fs::read_to_string(&public).map_err(|e| fail(format!("{}: {e}", public.display())))?;
    text.lines()
        .last()
        .map(str::to_owned)
        .ok_or_else(|| fail(format!("{} has no key line", public.display())))
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
