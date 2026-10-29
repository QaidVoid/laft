//! The steps that derive what packages need and offer: dependencies,
//! provides, imports and exports, static dependencies, triggers and
//! alternatives.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use laft_core::{Dep, Name, Provide, Soname, TRIGGERS, Variant};
use laft_elf::{Kind, Library};
use laft_pkg::StaticDep;

use crate::Recipe;
use crate::error::{BuildError, Warning, io};
use crate::fsx::{self, Kind as PathKind};
use crate::glob::{Glob, any_matches};
use crate::post::elf::{ElfFiles, provided, read_elf};
use crate::post::split::Part;
use crate::post::strip::DEBUG_DIR;

/// The interpreters every view has, which need no declared dependency.
const KNOWN_INTERPRETERS: [&str; 4] = [
    "/usr/bin/sh",
    "/usr/bin/bash",
    "/usr/bin/env sh",
    "/usr/bin/env bash",
];

/// What each payload file needs and offers, found by [`detect`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detected {
    /// The sonames each ELF file needs that the payload does not provide
    /// outside `usr/lib/`, by payload path.
    pub needs: BTreeMap<String, BTreeSet<Soname>>,
    /// The capability each file offers, by payload path.
    pub provides: BTreeMap<String, Provide>,
}

impl Detected {
    /// The detected dependencies and provides of a package that holds
    /// `paths`, each sorted by text: every needed soname that the package
    /// does not provide itself becomes `so:<soname>`.
    pub fn relations(&self, paths: &BTreeSet<String>) -> (Vec<Dep>, Vec<Provide>) {
        let provides: BTreeMap<String, Provide> = paths
            .iter()
            .filter_map(|p| self.provides.get(p))
            .map(|p| (p.to_string(), p.clone()))
            .collect();
        let depends: BTreeMap<String, Dep> = paths
            .iter()
            .filter_map(|p| self.needs.get(p))
            .flatten()
            .filter(|s| !provides.contains_key(&format!("so:{s}")))
            .map(|s| (format!("so:{s}"), Dep::So(s.clone())))
            .collect();
        (
            depends.into_values().collect(),
            provides.into_values().collect(),
        )
    }
}

/// Finds what the files of the payload `out` need and offer.
///
/// Every `DT_NEEDED` soname of an ELF file is a need, except one that a
/// shared object of the payload outside `usr/lib/` provides, which the
/// object reaches through its `RUNPATH`. In a `dynamic` build, every shared
/// object directly in `usr/lib/` provides `so:<soname>`, or `so:<file
/// name>` when it has no `DT_SONAME`, because that is the name the loader
/// finds it by. Every `usr/lib/pkgconfig/*.pc` and `usr/share/pkgconfig/*.pc`
/// provides `pc:<module>`, with the version its `Version:` line gives.
/// Files matching `ignore_deps` and debug files are skipped.
///
/// Every `#!` interpreter that the payload does not ship and that is not
/// one every view has is reported with warning `W112`, because nothing
/// detects the package that provides it.
///
/// # Errors
///
/// Fails when the payload cannot be read.
pub fn detect(
    out: &Path,
    elves: &ElfFiles,
    variant: Variant,
    ignore_deps: &[String],
    warnings: &mut Vec<Warning>,
) -> Result<Detected, BuildError> {
    let ignored: Vec<Glob> = ignore_deps.iter().map(|g| Glob::new(g)).collect();
    let private = provided(elves).private;
    let mut detected = Detected::default();
    for (rel, elf) in elves {
        if any_matches(&ignored, rel) || rel.starts_with(&format!("{DEBUG_DIR}/")) {
            continue;
        }
        let needs: BTreeSet<Soname> = elf
            .needed
            .iter()
            .filter_map(|n| match n {
                Library::Soname(s) if !private.contains_key(s.as_str()) => Some(s.clone()),
                _ => None,
            })
            .collect();
        if !needs.is_empty() {
            detected.needs.insert(rel.clone(), needs);
        }
        if variant == Variant::Dynamic
            && elf.kind == Kind::SharedObject
            && fsx::parent(rel) == "usr/lib"
        {
            let soname = match &elf.soname {
                Some(soname) => Some(soname.clone()),
                None => fsx::file_name(rel).parse().ok(),
            };
            if let Some(soname) = soname {
                detected.provides.insert(rel.clone(), Provide::So(soname));
            }
        }
    }
    let entries = fsx::walk(out)?;
    let shipped: BTreeSet<&str> = entries.iter().map(|(r, _)| r.as_str()).collect();
    for (rel, kind) in &entries {
        if *kind != PathKind::File || any_matches(&ignored, rel) {
            continue;
        }
        let dir = fsx::parent(rel);
        if variant == Variant::Dynamic
            && (dir == "usr/lib/pkgconfig" || dir == "usr/share/pkgconfig")
            && let Some(module) = fsx::file_name(rel).strip_suffix(".pc")
            && let Ok(module) = module.parse()
        {
            let version = pc_version(&out.join(rel))?;
            detected
                .provides
                .insert(rel.clone(), Provide::Pc(module, version));
        }
        if let Some(interpreter) = interpreter(&out.join(rel))? {
            let program = match interpreter.strip_prefix("/usr/bin/env ") {
                Some(command) => format!("usr/bin/{command}"),
                None => interpreter.trim_start_matches('/').to_owned(),
            };
            if !KNOWN_INTERPRETERS.contains(&interpreter.as_str())
                && !shipped.contains(program.as_str())
            {
                warnings.push(Warning::new(
                    "W112",
                    format!(
                        "{rel} runs {interpreter}, which is not detected as a dependency: \
                         declare its package in deps"
                    ),
                ));
            }
        }
    }
    Ok(detected)
}

/// The version a pkg-config file declares, when it is a valid version.
fn pc_version(path: &Path) -> Result<Option<laft_core::Version>, BuildError> {
    let file = fs::File::open(path).map_err(io(path))?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(io(path))?;
        if let Some(value) = line.strip_prefix("Version:") {
            return Ok(value.trim().parse().ok());
        }
    }
    Ok(None)
}

/// The interpreter of a script that starts with `#!`, with the command
/// that follows `/usr/bin/env`.
fn interpreter(path: &Path) -> Result<Option<String>, BuildError> {
    let mut head = Vec::new();
    let file = fs::File::open(path).map_err(io(path))?;
    BufReader::new(file.take(256))
        .read_until(b'\n', &mut head)
        .map_err(io(path))?;
    let Some(line) = head.strip_prefix(b"#!") else {
        return Ok(None);
    };
    let line = String::from_utf8_lossy(line);
    let mut words = line.split_whitespace();
    let Some(program) = words.next() else {
        return Ok(None);
    };
    Ok(Some(match (program.ends_with("/env"), words.next()) {
        (true, Some(command)) => format!("{program} {command}"),
        _ => program.to_owned(),
    }))
}

/// What the payload's ELF files import from each library they need,
/// written `name@VERSION` or `name`, for the import check of later builds.
///
/// A versioned import belongs to the library its version names. An
/// unversioned import belongs to every needed library that exports the
/// name, looked up in the payload's `usr/lib/` and then in `view`, the
/// build view's `usr` tree.
pub fn imports(out: &Path, elves: &ElfFiles, view: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut cache: BTreeMap<String, Option<BTreeSet<String>>> = BTreeMap::new();
    let mut exported = |soname: &str| -> Option<BTreeSet<String>> {
        cache
            .entry(soname.to_owned())
            .or_insert_with(|| {
                let local = format!("usr/lib/{soname}");
                let elf = fsx::resolve(out, &local)
                    .and_then(|rel| elves.get(&rel).cloned())
                    .or_else(|| read_elf(&view.join("lib").join(soname)).ok().flatten())?;
                Some(elf.exports.iter().map(|e| e.name.clone()).collect())
            })
            .clone()
    };
    let mut imports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (rel, elf) in elves {
        if rel.starts_with(&format!("{DEBUG_DIR}/")) {
            continue;
        }
        let needed: Vec<String> = elf
            .needed
            .iter()
            .filter_map(|n| match n {
                Library::Soname(s) => Some(s.to_string()),
                Library::Path(_) => None,
            })
            .collect();
        for import in &elf.imports {
            match &import.version {
                Some(version) => {
                    if let Library::Soname(library) = &version.library {
                        imports
                            .entry(library.to_string())
                            .or_default()
                            .insert(format!("{}@{}", import.name, version.name));
                    }
                }
                None => {
                    for soname in &needed {
                        if exported(soname).is_some_and(|names| names.contains(&import.name)) {
                            imports
                                .entry(soname.clone())
                                .or_default()
                                .insert(import.name.clone());
                        }
                    }
                }
            }
        }
    }
    imports
}

/// What the payload's libraries export, by the soname they provide,
/// written like [`imports`]: `name@VERSION` for a versioned symbol, and
/// the bare name for an unversioned one or one at its default version.
pub fn exports(elves: &ElfFiles) -> BTreeMap<String, BTreeSet<String>> {
    let mut exports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (rel, elf) in elves {
        let (Some(soname), Kind::SharedObject) = (&elf.soname, elf.kind) else {
            continue;
        };
        if fsx::parent(rel) != "usr/lib" {
            continue;
        }
        let names = exports.entry(soname.to_string()).or_default();
        for export in &elf.exports {
            match &export.version {
                Some(version) => {
                    names.insert(format!("{}@{}", export.name, version.name));
                    if !version.hidden {
                        names.insert(export.name.clone());
                    }
                }
                None => {
                    names.insert(export.name.clone());
                }
            }
        }
    }
    exports
}

/// The static library packages linked into a `static` build, sorted
/// by name, from `view`, the `static` packages that came into the build
/// view from its link dependencies. A `dynamic` build has none.
pub fn static_deps(variant: Variant, view: &[StaticDep]) -> Vec<StaticDep> {
    if variant != Variant::Static {
        return Vec::new();
    }
    let mut deps = view.to_vec();
    deps.sort_by(|a, b| a.name.cmp(&b.name));
    deps.dedup_by(|a, b| a.name == b.name);
    deps
}

/// The triggers that the payload paths `paths` activate, sorted by
/// name.
pub fn triggers<'a>(paths: impl IntoIterator<Item = &'a String>) -> Vec<Name> {
    let paths: Vec<&String> = paths.into_iter().collect();
    let names: BTreeSet<Name> = TRIGGERS
        .iter()
        .filter(|t| paths.iter().any(|p| t.activated_by(p)))
        .filter_map(|t| t.name.parse().ok())
        .collect();
    names.into_iter().collect()
}

/// The alternative groups of every package of the build, by package
/// name, with the priority the recipe gives each. Every path a group lists
/// must be in the package that the group names, the main package unless it
/// names a subpackage.
///
/// # Errors
///
/// Fails with `R040` for a path the package does not contain.
pub fn alternatives(
    recipe: &Recipe,
    parts: &[Part],
) -> Result<BTreeMap<Name, Vec<laft_pkg::Alternative>>, BuildError> {
    let mut groups: BTreeMap<Name, Vec<laft_pkg::Alternative>> = BTreeMap::new();
    for (group, alternative) in &recipe.outputs.alternatives {
        let part = parts
            .iter()
            .find(|p| p.suffix == alternative.subpackage)
            .ok_or_else(|| {
                BuildError::problem(
                    "R040",
                    format!(
                        "alternative {group} names subpackage {}, which the build did not emit",
                        alternative.subpackage.as_deref().unwrap_or("main")
                    ),
                )
            })?;
        if let Some(missing) = alternative.paths.iter().find(|p| !part.paths.contains(*p)) {
            return Err(BuildError::problem(
                "R040",
                format!(
                    "alternative {group} lists {missing}, which package {} does not contain",
                    part.name
                ),
            ));
        }
        let group: Name = group.parse().map_err(|e| {
            BuildError::problem("R040", format!("alternative group {group} is invalid: {e}"))
        })?;
        let mut paths = alternative.paths.clone();
        paths.sort();
        paths.dedup();
        groups
            .entry(part.name.clone())
            .or_default()
            .push(laft_pkg::Alternative {
                group,
                paths,
                priority: alternative.priority,
            });
    }
    Ok(groups)
}

/// Whether one of `paths` is an ELF file.
pub(crate) fn has_elf(elves: &ElfFiles, paths: &BTreeSet<String>) -> bool {
    paths.iter().any(|p| elves.contains_key(p))
}

#[cfg(test)]
#[path = "deps_tests.rs"]
mod tests;
