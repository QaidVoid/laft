//! The step that splits a payload into the main package and its
//! subpackages.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use laft_core::Name;
use laft_elf::Kind;

use crate::error::{BuildError, io};
use crate::fsx::{self, Kind as PathKind};
use crate::glob::{Glob, any_matches};
use crate::post::elf::{ElfFiles, read_elf_bytes};
use crate::{Recipe, Split};

/// One package of a build and the payload paths it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The package name: the recipe's name, or `<name>-<suffix>`.
    pub name: Name,
    /// The subpackage suffix, `None` for the main package.
    pub suffix: Option<String>,
    /// Its files, symlinks and kept empty directories. Their parent
    /// directories are not listed.
    pub paths: BTreeSet<String>,
}

/// The files of each automatic subpackage, except the `usr/lib` rules of
/// `dev`, which depend on the files themselves.
fn automatic(split: Split) -> &'static [&'static str] {
    match split {
        Split::Dbg => &["usr/lib/debug/**"],
        Split::Dev => &[
            "usr/include/**",
            "usr/lib/pkgconfig/**",
            "usr/share/pkgconfig/**",
            "usr/lib/cmake/**",
            "usr/share/aclocal/**",
            "usr/share/gir-1.0/**",
            "usr/share/vala/vapi/**",
            "usr/share/man/man3/**",
        ],
        Split::Doc => &[
            "usr/share/doc/**",
            "usr/share/gtk-doc/**",
            "usr/share/info/**",
            "usr/share/devhelp/**",
            "usr/share/help/**",
        ],
        Split::Locale => &["usr/share/locale/**"],
    }
}

/// Removes empty directories, then assigns every path of the payload
/// `out` to exactly one package of `recipe`.
///
/// Empty directories are removed unless `keep_dirs` matches them. Each
/// remaining path goes to the first of:
///
/// 1. the explicit subpackages, in the bytewise order of their suffixes,
///    whose `files` match it and whose `exclude` do not,
/// 2. the automatic subpackages of `split` in the order `dbg`, `dev`,
///    `doc`, `locale`, unless an explicit subpackage has the same suffix.
///    Besides its globs, `dev` holds `usr/lib/*.a` when archives are kept
///    and every `usr/lib/lib*.so` that is a symlink or a linker script,
///    except the soname links of libraries,
/// 3. the main package.
///
/// License files under `usr/share/licenses/` always stay in the main
/// package. The main package is always emitted, a subpackage only when it
/// holds something or sets `keep_empty`.
///
/// # Errors
///
/// Fails with `B115` when a subpackage name is not a valid package name,
/// and when the payload cannot be read or changed.
pub fn split(out: &Path, recipe: &Recipe, elves: &ElfFiles) -> Result<Vec<Part>, BuildError> {
    let keep: Vec<Glob> = recipe
        .options
        .keep_dirs
        .iter()
        .map(|g| Glob::new(g))
        .collect();
    remove_empty_dirs(out, "", &keep)?;

    let sonames: BTreeSet<String> = elves
        .iter()
        .filter(|(rel, elf)| elf.kind == Kind::SharedObject && fsx::parent(rel) == "usr/lib")
        .filter_map(|(_, elf)| elf.soname.as_ref().map(|s| s.to_string()))
        .collect();
    let entries = fsx::walk(out)?;
    let dirs_with_children: BTreeSet<&str> =
        entries.iter().map(|(rel, _)| fsx::parent(rel)).collect();

    let explicit: Vec<(&String, Vec<Glob>, Vec<Glob>)> = recipe
        .outputs
        .subpackages
        .iter()
        .map(|(suffix, sub)| {
            let globs = |list: &[String]| list.iter().map(|g| Glob::new(g)).collect();
            (suffix, globs(&sub.files), globs(&sub.exclude))
        })
        .collect();
    let auto: Vec<(Split, Vec<Glob>)> = recipe
        .outputs
        .split
        .iter()
        .copied()
        .filter(|s| !recipe.outputs.subpackages.contains_key(s.as_str()))
        .map(|s| (s, automatic(s).iter().map(|g| Glob::new(g)).collect()))
        .collect();

    let mut parts: Vec<Part> = Vec::new();
    part(&mut parts, recipe, None)?;
    for (suffix, _, _) in &explicit {
        if recipe.outputs.subpackages[*suffix].keep_empty {
            part(&mut parts, recipe, Some(suffix))?;
        }
    }

    for (rel, kind) in &entries {
        if *kind == PathKind::Dir && dirs_with_children.contains(rel.as_str()) {
            continue;
        }
        let suffix = if rel.starts_with("usr/share/licenses/") {
            None
        } else if let Some((suffix, _, _)) = explicit
            .iter()
            .find(|(_, files, exclude)| any_matches(files, rel) && !any_matches(exclude, rel))
        {
            Some(suffix.as_str())
        } else {
            let mut found = None;
            for (split, globs) in &auto {
                let extra = *split == Split::Dev && dev_library(out, rel, *kind, recipe, &sonames)?;
                if any_matches(globs, rel) || extra {
                    found = Some(split.as_str());
                    break;
                }
            }
            found
        };
        let index = part(&mut parts, recipe, suffix)?;
        parts[index].paths.insert(rel.clone());
    }
    Ok(parts)
}

/// The index of the part with `suffix` in `parts`, which is added when
/// missing.
fn part(parts: &mut Vec<Part>, recipe: &Recipe, suffix: Option<&str>) -> Result<usize, BuildError> {
    if let Some(i) = parts.iter().position(|p| p.suffix.as_deref() == suffix) {
        return Ok(i);
    }
    let name = match suffix {
        None => recipe.name.clone(),
        Some(suffix) => format!("{}-{suffix}", recipe.name).parse().map_err(|e| {
            BuildError::problem(
                "B115",
                format!(
                    "subpackage {suffix} of {} has no valid name: {e}",
                    recipe.name
                ),
            )
        })?,
    };
    parts.push(Part {
        name,
        suffix: suffix.map(str::to_owned),
        paths: BTreeSet::new(),
    });
    Ok(parts.len() - 1)
}

/// Whether the `dev` subpackage takes the payload path `rel` by its
/// `usr/lib` rules.
fn dev_library(
    out: &Path,
    rel: &str,
    kind: PathKind,
    recipe: &Recipe,
    sonames: &BTreeSet<String>,
) -> Result<bool, BuildError> {
    if fsx::parent(rel) != "usr/lib" {
        return Ok(false);
    }
    let name = fsx::file_name(rel);
    if name.ends_with(".a") {
        return Ok(recipe.options.keep_archives);
    }
    if !(name.starts_with("lib") && name.ends_with(".so")) || sonames.contains(name) {
        return Ok(false);
    }
    Ok(match kind {
        PathKind::Symlink => true,
        PathKind::File => read_elf_bytes(&out.join(rel))?.is_none(),
        _ => false,
    })
}

/// Removes the empty directories below `dir` of the payload `out`, deepest
/// first, unless `keep` matches them. Returns whether `dir` is empty
/// afterwards.
fn remove_empty_dirs(out: &Path, dir: &str, keep: &[Glob]) -> Result<bool, BuildError> {
    let path = out.join(dir);
    let mut empty = true;
    for item in fs::read_dir(&path).map_err(io(&path))? {
        let item = item.map_err(io(&path))?;
        let name = item.file_name().to_string_lossy().into_owned();
        let rel = if dir.is_empty() {
            name
        } else {
            format!("{dir}/{name}")
        };
        if item.file_type().map_err(io(&path))?.is_dir()
            && remove_empty_dirs(out, &rel, keep)?
            && !any_matches(keep, &rel)
        {
            let child = out.join(&rel);
            fs::remove_dir(&child).map_err(io(&child))?;
        } else {
            empty = false;
        }
    }
    Ok(empty)
}

#[cfg(test)]
#[path = "split_tests.rs"]
mod tests;
