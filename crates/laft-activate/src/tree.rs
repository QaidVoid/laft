//! Walking and resolving paths inside a tree under construction, without
//! ever leaving it through a symlink.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result, io};

/// Every entry below `tree/rel`, as paths relative to `tree`, parents
/// before children and siblings in byte order. Symlinks are listed and not
/// followed. A `tree/rel` that is missing, is not a directory or has a
/// symlink among its components has no entries.
///
/// # Errors
///
/// Fails when `rel` is not a relative path of normal components, or when a
/// directory cannot be read.
pub(crate) fn walk(tree: &Path, rel: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if real_dir(tree, rel)? {
        walk_into(tree, rel, &mut out)?;
    }
    Ok(out)
}

/// Whether `tree/rel` is a directory reached without a symlink.
///
/// # Errors
///
/// Fails when `rel` is not a relative path of normal components.
fn real_dir(tree: &Path, rel: &str) -> Result<bool> {
    if rel
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Error::Invalid(format!(
            "{rel:?} is not a relative path inside a tree"
        )));
    }
    let mut end = 0;
    for part in rel.split('/') {
        end += part.len();
        match lstat(tree, &rel[..end])? {
            Some(metadata) if metadata.is_dir() => {}
            _ => return Ok(false),
        }
        end += 1;
    }
    Ok(true)
}

fn walk_into(tree: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
    let dir = tree.join(rel);
    let items = match fs::read_dir(&dir) {
        Ok(items) => items,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(());
        }
        Err(e) => return Err(io(&dir)(e)),
    };
    let mut names = Vec::new();
    for item in items {
        let item = item.map_err(io(&dir))?;
        let is_dir = item.file_type().map_err(io(&dir))?.is_dir();
        names.push((item.file_name().to_string_lossy().into_owned(), is_dir));
    }
    names.sort();
    for (name, is_dir) in names {
        let child = format!("{rel}/{name}");
        out.push(child.clone());
        if is_dir {
            walk_into(tree, &child, out)?;
        }
    }
    Ok(())
}

/// The names of the directories directly in `tree/rel`, sorted. Symlinks
/// to directories are left out, and a `tree/rel` reached through a symlink
/// has none.
///
/// # Errors
///
/// Fails when `rel` is not a relative path of normal components, or when
/// the directory cannot be read.
pub(crate) fn subdirs(tree: &Path, rel: &str) -> Result<Vec<String>> {
    if !real_dir(tree, rel)? {
        return Ok(Vec::new());
    }
    let dir = tree.join(rel);
    let items = match fs::read_dir(&dir) {
        Ok(items) => items,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(e) => return Err(io(&dir)(e)),
    };
    let mut names = Vec::new();
    for item in items {
        let item = item.map_err(io(&dir))?;
        if item.file_type().map_err(io(&dir))?.is_dir() {
            names.push(item.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The metadata of `tree/rel` without following a final symlink, or `None`
/// when it does not exist.
pub(crate) fn lstat(tree: &Path, rel: &str) -> Result<Option<fs::Metadata>> {
    let path = tree.join(rel);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(io(&path)(e)),
    }
}

/// Resolves `rel`, a path relative to `tree`, following symlinks the way
/// they resolve when `tree` is the root: absolute targets start at `tree`
/// and `..` never climbs above it. Returns the host path of the regular
/// file it names, or `None` when it names nothing or something else.
pub(crate) fn resolve_file(tree: &Path, rel: &str) -> Result<Option<PathBuf>> {
    let mut pending: Vec<String> = rel.split('/').rev().map(str::to_owned).collect();
    let mut resolved: Vec<String> = Vec::new();
    let mut hops = 0;
    while let Some(part) = pending.pop() {
        match part.as_str() {
            "" | "." => continue,
            ".." => {
                resolved.pop();
                continue;
            }
            _ => {}
        }
        resolved.push(part);
        let current = resolved.join("/");
        let Some(metadata) = lstat(tree, &current)? else {
            return Ok(None);
        };
        if metadata.file_type().is_symlink() {
            hops += 1;
            if hops > 40 {
                return Ok(None);
            }
            let link = tree.join(&current);
            let target = fs::read_link(&link).map_err(io(&link))?;
            resolved.pop();
            if target.has_root() {
                resolved.clear();
            }
            for component in target.components().rev() {
                if let Component::Normal(name) = component {
                    pending.push(name.to_string_lossy().into_owned());
                } else if component == Component::ParentDir {
                    pending.push("..".to_owned());
                }
            }
        } else if (pending.is_empty() && !metadata.is_file())
            || (!pending.is_empty() && !metadata.is_dir())
        {
            return Ok(None);
        }
    }
    if resolved.is_empty() {
        return Ok(None);
    }
    Ok(Some(tree.join(resolved.join("/"))))
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
