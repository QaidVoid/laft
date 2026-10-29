//! The steps that check and normalize where a payload puts its files.

use std::fs;
use std::path::Path;

use laft_core::{TRIGGERS, Variant};

use crate::SYSROOT;
use crate::error::{BuildError, Warning, io};
use crate::fsx::{self, Kind};
use crate::post::read_elf;

/// P1: fails with `B101` when the payload `out` holds no file or symlink,
/// unless the package is `empty`.
///
/// # Errors
///
/// Fails with `B101`, or when the payload cannot be read.
pub fn check_output(out: &Path, empty: bool) -> Result<(), BuildError> {
    if empty {
        return Ok(());
    }
    let any = fsx::walk(out)?
        .iter()
        .any(|(_, kind)| matches!(kind, Kind::File | Kind::Symlink));
    if any {
        Ok(())
    } else {
        Err(BuildError::problem("B101", "install produced no files"))
    }
}

/// P2: moves everything into the `usr` layout and deletes files no package
/// ships.
///
/// Top-level `bin`, `sbin`, `lib` and `lib64` move into `usr/`, then
/// `usr/sbin` into `usr/bin` and `usr/lib64` into `usr/lib`, merging
/// directories. A symlink in place of one of these directories is the
/// merged layout itself and is removed. Then `usr/share/info/dir`, every
/// `*.la` file and every `perllocal.pod` and `.packlist` file are deleted.
///
/// # Errors
///
/// Fails with `B102` when a file would land on an existing one, and when
/// the payload cannot be changed.
pub fn normalize_paths(out: &Path) -> Result<(), BuildError> {
    for dir in ["bin", "sbin", "lib", "lib64"] {
        merge(out, dir, &format!("usr/{dir}"))?;
    }
    merge(out, "usr/sbin", "usr/bin")?;
    merge(out, "usr/lib64", "usr/lib")?;
    for (rel, kind) in fsx::walk(out)? {
        let name = fsx::file_name(&rel);
        let unwanted = rel == "usr/share/info/dir"
            || name.ends_with(".la")
            || name == "perllocal.pod"
            || name == ".packlist";
        if unwanted && kind != Kind::Dir {
            let path = out.join(&rel);
            fs::remove_file(&path).map_err(io(&path))?;
        }
    }
    Ok(())
}

/// Moves the contents of the payload directory `from` into `to`.
fn merge(out: &Path, from: &str, to: &str) -> Result<(), BuildError> {
    let source = out.join(from);
    let Ok(metadata) = fs::symlink_metadata(&source) else {
        return Ok(());
    };
    if metadata.is_symlink() {
        return fs::remove_file(&source).map_err(io(&source));
    }
    if !metadata.is_dir() {
        return Err(BuildError::problem(
            "B102",
            format!("{from} is not a directory, so it cannot move to {to}"),
        ));
    }
    let target = out.join(to);
    if fs::symlink_metadata(&target).is_ok_and(|m| m.is_symlink()) {
        fs::remove_file(&target).map_err(io(&target))?;
    }
    fs::create_dir_all(&target).map_err(io(&target))?;
    let mut names: Vec<String> = fs::read_dir(&source)
        .map_err(io(&source))?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()
        .map_err(io(&source))?;
    names.sort();
    for name in names {
        let (a, b) = (format!("{from}/{name}"), format!("{to}/{name}"));
        let existing = fs::symlink_metadata(out.join(&b)).ok();
        let moving = fs::symlink_metadata(out.join(&a)).map_err(io(&out.join(&a)))?;
        match existing {
            None => fs::rename(out.join(&a), out.join(&b)).map_err(io(&out.join(&b)))?,
            Some(m) if m.is_dir() && moving.is_dir() => merge(out, &a, &b)?,
            Some(_) => {
                return Err(BuildError::problem(
                    "B102",
                    format!("{a} and {b} both exist"),
                ));
            }
        }
    }
    fs::remove_dir(&source).map_err(io(&source))
}

/// P3: fails with `B103` for anything outside `usr/` and `etc/` or inside
/// `usr/local/`. In a `dynamic` build, static archives directly in
/// `usr/lib/` are deleted with warning `W110` unless `keep_archives`.
///
/// # Errors
///
/// Fails with `B103`, and when the payload cannot be read or changed.
pub fn check_paths(
    out: &Path,
    variant: Variant,
    keep_archives: bool,
    warnings: &mut Vec<Warning>,
) -> Result<(), BuildError> {
    let entries = fsx::walk(out)?;
    let allowed = |rel: &str| {
        matches!(rel, "usr" | "etc")
            || (rel.starts_with("usr/") || rel.starts_with("etc/"))
                && !(rel == "usr/local" || rel.starts_with("usr/local/"))
    };
    let forbidden: Vec<&(String, Kind)> = entries.iter().filter(|(r, _)| !allowed(r)).collect();
    let shown = forbidden
        .iter()
        .find(|(_, kind)| *kind != Kind::Dir)
        .or(forbidden.first());
    if let Some((rel, _)) = shown {
        return Err(BuildError::problem(
            "B103",
            format!("forbidden path {rel} (packages may only install under usr/ and etc/)"),
        ));
    }
    if variant == Variant::Dynamic && !keep_archives {
        let archives: Vec<&String> = entries
            .iter()
            .filter(|(rel, kind)| {
                *kind != Kind::Dir && fsx::parent(rel) == "usr/lib" && rel.ends_with(".a")
            })
            .map(|(rel, _)| rel)
            .collect();
        for rel in &archives {
            let path = out.join(rel);
            fs::remove_file(&path).map_err(io(&path))?;
        }
        if !archives.is_empty() {
            warnings.push(Warning::new(
                "W110",
                format!(
                    "removed static archives {} (set keep_archives to ship them)",
                    join(&archives)
                ),
            ));
        }
    }
    Ok(())
}

/// P4: arranges the payload of a `static` build, which installs into the
/// musl sysroot.
///
/// Programs in the sysroot's `usr/bin` move to `usr/bin`, its manual
/// pages, licenses and shell completions move to `usr/share`, and its
/// documentation, info pages and translations are deleted. The payload
/// must then hold no shared object (`B104`) and no program in `usr/bin`
/// that loads one (`B105`).
///
/// # Errors
///
/// Fails with `B102`, `B104` or `B105`, and when the payload cannot be
/// read or changed.
pub fn static_layout(out: &Path) -> Result<(), BuildError> {
    let sysroot = SYSROOT.trim_start_matches('/');
    merge(out, &format!("{sysroot}/usr/bin"), "usr/bin")?;
    for dir in ["man", "licenses", "bash-completion", "zsh", "fish"] {
        merge(
            out,
            &format!("{sysroot}/usr/share/{dir}"),
            &format!("usr/share/{dir}"),
        )?;
    }
    for dir in ["doc", "info", "locale"] {
        fsx::remove_tree(&out.join(format!("{sysroot}/usr/share/{dir}")))?;
    }
    for (rel, kind) in fsx::walk(out)? {
        if kind != Kind::File {
            continue;
        }
        let Some(elf) = read_elf(&out.join(&rel)).ok().flatten() else {
            continue;
        };
        if elf.kind == laft_elf::Kind::SharedObject {
            return Err(BuildError::problem(
                "B104",
                format!("{rel} is a shared object, but static builds ship none"),
            ));
        }
        if fsx::parent(&rel) == "usr/bin" && (elf.interpreter.is_some() || !elf.needed.is_empty()) {
            return Err(BuildError::problem(
                "B105",
                format!("{rel} is dynamically linked, but static builds ship static programs"),
            ));
        }
    }
    Ok(())
}

/// P4b: deletes every file that a trigger of the trigger table generates,
/// because every view generates its own, with warning `W113`.
///
/// # Errors
///
/// Fails when the payload cannot be read or changed.
pub fn strip_trigger_outputs(out: &Path, warnings: &mut Vec<Warning>) -> Result<(), BuildError> {
    let mut removed = Vec::new();
    for (rel, kind) in fsx::walk(out)? {
        if kind != Kind::Dir && TRIGGERS.iter().any(|t| t.generates(&rel)) {
            let path = out.join(&rel);
            fs::remove_file(&path).map_err(io(&path))?;
            removed.push(rel);
        }
    }
    if !removed.is_empty() {
        let removed: Vec<&String> = removed.iter().collect();
        warnings.push(Warning::new(
            "W113",
            format!(
                "removed files that views generate themselves: {}",
                join(&removed)
            ),
        ));
    }
    Ok(())
}

fn join(paths: &[&String]) -> String {
    paths
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
