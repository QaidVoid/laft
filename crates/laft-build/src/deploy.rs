//! Unpacking packages over a system under construction, so that the builds
//! of a bootstrap stage that makes packages see the packages made before
//! them.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

use laft_pkg::Reader;

use crate::error::{BuildError, io};
use crate::fsx::{self, Kind};

/// Where [`unpack_into`] unpacks a package before it moves the files into
/// place, relative to the destination, so that every move is a rename
/// within one filesystem.
const STAGING: &str = "var/lib/laft-build/unpack";

/// Unpacks the package file `file` over `dest`, a system under
/// construction.
///
/// The payload is unpacked with the checks of [`Reader::unpack`] into a
/// staging directory inside `dest` first. Then every directory that `dest`
/// lacks is made with the package's mode, every existing one is kept as it
/// is, and every file and symbolic link replaces what is at its path with a
/// rename, so a program that runs from `dest` never sees a partial file.
/// Symbolic links in `dest` are never followed, so nothing is written
/// outside it.
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
    let reader = fs::File::open(file)
        .map_err(io(file))
        .and_then(|f| Reader::new(f).map_err(BuildError::from))?;
    reader.unpack(&staging)?;
    for (rel, kind) in fsx::walk(&staging)? {
        let from = staging.join(&rel);
        let to = dest.join(&rel);
        let found = match fs::symlink_metadata(&to) {
            Ok(meta) => Some(meta.file_type()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io(&to)(e)),
        };
        match (kind, found) {
            (Kind::Dir, Some(found)) if found.is_dir() => {}
            (Kind::Dir, None) => {
                let mode = fs::symlink_metadata(&from)
                    .map_err(io(&from))?
                    .permissions()
                    .mode();
                fs::DirBuilder::new()
                    .mode(mode & 0o7777)
                    .create(&to)
                    .map_err(io(&to))?;
            }
            (Kind::Dir, Some(_)) => return Err(conflict(file, &rel, true)),
            (_, Some(found)) if found.is_dir() => {
                return Err(conflict(file, &rel, false));
            }
            _ => fs::rename(&from, &to).map_err(io(&to))?,
        }
    }
    fsx::remove_tree(&staging)
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
