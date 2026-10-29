//! The steps that describe each package and write its file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use laft_core::{Blake3Hash, Dep, PkgRef, Provide, VersionReq};
use laft_pkg::{Abi, Arch, Compression, Entry, Manifest, Meta, StaticDep};

use crate::error::{BuildError, io};
use crate::fsx;
use crate::post::deps::{Detected, has_elf};
use crate::post::elf::ElfFiles;
use crate::post::privileges::Privileges;
use crate::post::split::Part;
use crate::{Recipe, Split};

/// A package that post-processing produced, ready to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Its metadata.
    pub meta: Meta,
    /// Its payload manifest, which names the metadata by hash.
    pub manifest: Manifest,
}

/// What the earlier steps found out about the whole payload, which every
/// package takes its share of.
#[derive(Debug, Clone, Copy)]
pub struct Payload<'a> {
    /// Every path of the payload, as [`laft_pkg::scan`] lists it.
    pub entries: &'a [Entry],
    /// The ELF files.
    pub elves: &'a ElfFiles,
    /// What the files need and offer.
    pub detected: &'a Detected,
    /// Owners and requested privileges.
    pub privileges: &'a Privileges,
    /// The ABI digests, by payload path.
    pub abi: &'a BTreeMap<String, Abi>,
    /// The static libraries linked into the build's programs.
    pub static_deps: &'a [StaticDep],
}

/// A dependency on the package that `r` names.
fn dep(r: &PkgRef) -> Dep {
    let req = r
        .prefix()
        .map_or(VersionReq::Any, |p| VersionReq::Prefix(p.clone()));
    Dep::Pkg(r.name().clone(), req)
}

fn sorted_deps(deps: impl IntoIterator<Item = Dep>) -> Vec<Dep> {
    let map: BTreeMap<String, Dep> = deps.into_iter().map(|d| (d.to_string(), d)).collect();
    map.into_values().collect()
}

/// The metadata and manifest of the package `part` of `recipe`.
///
/// The metadata is a function of the recipe and the payload alone, so two
/// builds with the same output describe their packages with the same
/// bytes. Detected dependencies and provides come from [`Detected`], the
/// declared ones from the recipe or the subpackage. The `dbg` and `dev`
/// subpackages, and every explicit one with `depends_on_main`, depend on
/// the exact version and revision of the main package. The manifest lists
/// the package's paths and their parent directories, each directory with
/// mode `0o755` and every path with the owner `ownership` gives it. Its
/// hash is the store id.
///
/// # Errors
///
/// Fails with `B116` when a declared provide is not a capability, and when
/// the metadata or the manifest breaks a rule of the package format.
pub fn package(
    payload: Payload<'_>,
    recipe: &Recipe,
    part: &Part,
    alternatives: Vec<laft_pkg::Alternative>,
) -> Result<Package, BuildError> {
    let explicit = part
        .suffix
        .as_ref()
        .and_then(|s| recipe.outputs.subpackages.get(s));
    let automatic_main = part
        .suffix
        .as_deref()
        .is_some_and(|s| s == Split::Dbg.as_str() || s == Split::Dev.as_str())
        && explicit.is_none();
    let (detected_deps, detected_provides) = payload.detected.relations(&part.paths);
    let (summary, license, declared) = match (&part.suffix, explicit) {
        (None, _) => (
            recipe.meta.summary.clone(),
            recipe.meta.license.clone(),
            Declared {
                deps: &recipe.outputs.deps,
                provides: &recipe.outputs.provides,
                conflicts: &recipe.outputs.conflicts,
                replaces: &recipe.outputs.replaces,
            },
        ),
        (Some(_), Some(sub)) => (
            sub.summary.clone(),
            sub.license.clone(),
            Declared {
                deps: &sub.deps,
                provides: &sub.provides,
                conflicts: &sub.conflicts,
                replaces: &sub.replaces,
            },
        ),
        (Some(suffix), None) => (
            format!("{} ({suffix})", recipe.meta.summary),
            recipe.meta.license.clone(),
            Declared::default(),
        ),
    };
    let mut depends: Vec<Dep> = detected_deps;
    depends.extend(declared.deps.iter().map(dep));
    if automatic_main || explicit.is_some_and(|s| s.depends_on_main) {
        let exact = format!("{}={}-r{}", recipe.name, recipe.version, recipe.rev);
        depends.push(exact.parse().map_err(|e| {
            BuildError::problem("B116", format!("{exact} is not a dependency: {e}"))
        })?);
    }
    let mut provides: BTreeMap<String, Provide> = detected_provides
        .into_iter()
        .map(|p| (p.to_string(), p))
        .collect();
    for text in declared.provides {
        let provide: Provide = text.parse().map_err(|e| {
            BuildError::problem(
                "B116",
                format!("{} provides {text}, which is invalid: {e}", part.name),
            )
        })?;
        provides.insert(provide.to_string(), provide);
    }
    let mut license = license;
    license.sort();
    license.dedup();

    let entries = entries(payload, part);
    let installed_size = entries
        .iter()
        .map(|e| match e {
            Entry::File { size, .. } => *size,
            _ => 0,
        })
        .sum();
    let meta = Meta {
        format: laft_pkg::FORMAT,
        name: part.name.clone(),
        version: recipe.version.clone(),
        rev: recipe.rev,
        variant: recipe.variant,
        arch: Arch::X86_64V3,
        origin: recipe.name.clone(),
        summary,
        homepage: recipe.meta.homepage.clone(),
        license,
        depends: sorted_deps(depends),
        provides: provides.into_values().collect(),
        conflicts: sorted_deps(declared.conflicts.iter().map(dep)),
        replaces: sorted_deps(declared.replaces.iter().map(dep)),
        alternatives,
        triggers: crate::post::deps::triggers(&part.paths),
        static_deps: if has_elf(payload.elves, &part.paths) {
            payload.static_deps.to_vec()
        } else {
            Vec::new()
        },
        abi: payload
            .abi
            .iter()
            .filter(|(path, _)| part.paths.contains(*path))
            .map(|(p, a)| (p.clone(), a.clone()))
            .collect(),
        abi_epoch: recipe.abi_epoch,
        requested_privileges: payload
            .privileges
            .requested
            .iter()
            .filter(|r| part.paths.contains(&r.path))
            .cloned()
            .collect(),
        installed_size,
    };
    let manifest = Manifest::new(meta.identity(), Blake3Hash::of(&meta.to_json()?), entries)?;
    Ok(Package { meta, manifest })
}

/// The relations a recipe or subpackage declares.
#[derive(Default)]
struct Declared<'a> {
    deps: &'a [PkgRef],
    provides: &'a [String],
    conflicts: &'a [PkgRef],
    replaces: &'a [PkgRef],
}

/// The manifest entries of `part`: its paths and their parent directories.
fn entries(payload: Payload<'_>, part: &Part) -> Vec<Entry> {
    let mut dirs: BTreeSet<&str> = BTreeSet::new();
    for path in &part.paths {
        let mut dir = fsx::parent(path);
        while !dir.is_empty() && dirs.insert(dir) {
            dir = fsx::parent(dir);
        }
    }
    payload
        .entries
        .iter()
        .filter(|e| part.paths.contains(e.path()) || dirs.contains(e.path()))
        .map(|entry| {
            let (owner, group) = payload.privileges.owner(entry.path());
            match entry.clone() {
                Entry::Dir { path, .. } => Entry::Dir {
                    path,
                    mode: 0o755,
                    owner,
                    group,
                },
                Entry::File {
                    path,
                    mode,
                    size,
                    hash,
                    ..
                } => Entry::File {
                    path,
                    mode,
                    size,
                    hash,
                    owner,
                    group,
                },
                symlink => symlink,
            }
        })
        .collect()
}

/// Writes every package to the directory `dir`, reading payload files
/// from `out`, and returns the paths of the package files. Each file is
/// named as the package format specifies and appears whole, with one
/// rename.
///
/// # Errors
///
/// Fails when a package cannot be written.
pub fn write_packages(
    out: &Path,
    packages: &[Package],
    dir: &Path,
    compression: Compression,
) -> Result<Vec<PathBuf>, BuildError> {
    fs::create_dir_all(dir).map_err(io(dir))?;
    let mut written = Vec::new();
    for package in packages {
        let name = package.manifest.package().file_name();
        let path = dir.join(&name);
        let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        let result = File::create(&tmp).map_err(io(&tmp)).and_then(|file| {
            let mut writer = BufWriter::new(file);
            laft_pkg::write_package(
                &mut writer,
                out,
                &package.meta,
                &package.manifest,
                compression,
            )?;
            writer.into_inner().map_err(|e| io(&tmp)(e.into_error()))?;
            fsx::publish(&tmp, &path)
        });
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;
