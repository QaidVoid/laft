//! Unpacking packages over a system under construction, so that the builds
//! of a bootstrap stage that makes packages see the packages made before
//! them.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
use std::path::Path;

use laft_pkg::{Entry, Reader};

use crate::error::{BuildError, io};
use crate::fsx;

/// Where [`unpack_into`] unpacks a package before it moves the files into
/// place, relative to the destination, so that every move is a rename
/// within one filesystem.
const STAGING: &str = "var/lib/laft-build/unpack";

/// Unpacks the package file `file` over `dest`, a system under
/// construction.
///
/// The payload is unpacked with the checks of [`Reader::unpack`] into a
/// staging directory inside `dest` first. Then every directory that `dest`
/// lacks is made with the mode, owner and group the package declares,
/// every existing one is kept as it is, and every file and symbolic link
/// replaces what is at its path with a rename, so a program that runs from
/// `dest` never sees a partial file. Symbolic links in `dest` are never
/// followed, so nothing is written outside it. The staging directory is
/// removed whether or not this succeeds.
///
/// Every file is on disk before it is renamed into place, and every rename
/// and new directory is on disk before this returns, so that a record
/// written afterwards never outlives a payload that a crash lost. Each of
/// the two flushes is one `syncfs(2)` of the filesystem of `dest`, which
/// covers every path of the package at once.
///
/// # Errors
///
/// Fails with `B043` when a payload path is a directory in the package and
/// something else in `dest`, or the other way round, which leaves the paths
/// before it in place. Fails with [`BuildError::Package`] when the package
/// does not match its manifest, and with [`BuildError::Io`] when reading or
/// writing fails.
pub(crate) fn unpack_into(file: &Path, dest: &Path) -> Result<(), BuildError> {
    let staging = dest.join(STAGING);
    fsx::remove_tree(&staging)?;
    fs::create_dir_all(&staging).map_err(io(&staging))?;
    let placed = place(file, &staging, dest);
    let removed = fsx::remove_tree(&staging);
    placed.and(removed)
}

/// Unpacks `file` into `staging` and moves its payload into `dest`.
fn place(file: &Path, staging: &Path, dest: &Path) -> Result<(), BuildError> {
    let reader = fs::File::open(file)
        .map_err(io(file))
        .and_then(|f| Reader::new(f).map_err(BuildError::from))?;
    let entries = reader.manifest().entries().to_vec();
    reader.unpack(staging)?;
    fsx::sync_fs(staging)?;
    let mut made = Vec::new();
    for entry in &entries {
        let rel = entry.path();
        let from = staging.join(rel);
        let to = dest.join(rel);
        let found = match fs::symlink_metadata(&to) {
            Ok(meta) => Some(meta.file_type()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io(&to)(e)),
        };
        match (entry, found) {
            (Entry::Dir { .. }, Some(found)) if found.is_dir() => {}
            (Entry::Dir { mode, .. }, None) => {
                fs::create_dir(&to).map_err(io(&to))?;
                made.push((from, to, *mode));
            }
            (Entry::Dir { .. }, Some(_)) => return Err(conflict(file, rel, true)),
            (_, Some(found)) if found.is_dir() => {
                return Err(conflict(file, rel, false));
            }
            _ => fs::rename(&from, &to).map_err(io(&to))?,
        }
    }
    // The modes come last, because a declared mode may keep the owner from
    // moving the directory's contents in.
    for (from, to, mode) in made {
        let staged = fs::symlink_metadata(&from).map_err(io(&from))?;
        let ours = fs::symlink_metadata(&to).map_err(io(&to))?;
        if (staged.uid(), staged.gid()) != (ours.uid(), ours.gid()) {
            chown(&to, Some(staged.uid()), Some(staged.gid())).map_err(io(&to))?;
        }
        fs::set_permissions(&to, fs::Permissions::from_mode(mode)).map_err(io(&to))?;
    }
    fsx::sync_fs(dest)
}

fn conflict(file: &Path, rel: &str, directory: bool) -> BuildError {
    let (ours, theirs) = if directory {
        ("a directory", "something else")
    } else {
        ("not a directory", "a directory")
    };
    BuildError::problem(
        "B043",
        format!(
            "{rel} is {ours} in {}, but the destination has {theirs} there",
            file.display()
        ),
    )
}

#[cfg(test)]
#[path = "deploy_tests.rs"]
mod tests;
