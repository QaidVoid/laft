//! Crash safe file writes: a temporary file in the same directory is
//! written and synced, then moved into place, and the directory is synced.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The temporary file next to `path`, named after its whole file name so
/// that two files of one directory never share it.
fn temporary(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.laft-tmp"))
}

/// Writes `bytes` to the temporary file of `path` with the permissions
/// `mode` and syncs it.
fn write_temporary(
    path: &Path,
    bytes: &[u8],
    mode: Option<fs::Permissions>,
) -> io::Result<PathBuf> {
    let tmp = temporary(path);
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        if let Some(mode) = mode {
            file.set_permissions(mode)?;
        }
        file.sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(tmp)
}

fn sync_parent(path: &Path) -> io::Result<()> {
    fs::File::open(path.parent().unwrap_or(Path::new("/")))?.sync_all()
}

/// Replaces the file `path` with `bytes`, keeping its permissions, so that
/// a crash leaves either the old or the new content.
///
/// # Errors
///
/// Fails when the file cannot be read, written or renamed. The temporary
/// file is removed then.
pub(crate) fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mode = fs::metadata(path)?.permissions();
    let tmp = write_temporary(path, bytes, Some(mode))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    sync_parent(path)
}

/// Creates the file `path` with `bytes` and its parent directories. It
/// appears complete or not at all, and an existing file is never replaced.
///
/// # Errors
///
/// Fails with [`io::ErrorKind::AlreadyExists`] when `path` exists, and
/// when it cannot be written.
pub(crate) fn create_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = write_temporary(path, bytes, None)?;
    // A hard link fails when the name exists, unlike a rename.
    let linked = fs::hard_link(&tmp, path);
    let _ = fs::remove_file(&tmp);
    linked?;
    sync_parent(path)
}

#[cfg(test)]
#[path = "fsx_tests.rs"]
mod tests;
