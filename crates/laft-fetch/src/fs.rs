//! Crash-safe file helpers for the cache.

use std::fs::{self, DirBuilder, File};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::error::io;

/// The mode of cache files. The cache directories are private, so the
/// files need not be.
pub(crate) const FILE_MODE: u32 = 0o644;

/// The temporary name `path` is written under before it is renamed into
/// place.
pub(crate) fn part_path(path: &Path) -> PathBuf {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

/// Creates `dir` and its missing parents with mode 0700. Several callers may
/// race to create the same directory.
pub(crate) fn create_dir(dir: &Path) -> Result<(), Error> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io(dir))
}

/// Renames the synced file `part` to `path` and syncs the directory.
pub(crate) fn publish(part: &Path, path: &Path) -> Result<(), Error> {
    fs::rename(part, path).map_err(io(path))?;
    let dir = parent(path);
    File::open(dir).and_then(|d| d.sync_all()).map_err(io(dir))
}

/// Removes a file that may not exist.
pub(crate) fn remove(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io(path)(e)),
        _ => Ok(()),
    }
}

fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}
