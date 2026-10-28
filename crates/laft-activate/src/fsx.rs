//! Crash safe file operations on the live system.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, fchown};
use std::path::{Path, PathBuf};

use crate::error::{Result, io};

/// Who owns a file Laft writes: a uid and a gid.
pub(crate) type Owner = (u32, u32);

/// Whether the process runs as root, so that it can give files away.
///
/// An unprivileged caller, such as a test working in a temporary root,
/// cannot change owners, so its files keep the caller as owner.
pub(crate) fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

/// Writes `bytes` to the new file `tmp` with `mode` and `owner`, and syncs
/// it. A file left at `tmp` by an earlier attempt is replaced.
pub(crate) fn stage(tmp: &Path, bytes: &[u8], mode: u32, owner: Owner) -> Result<()> {
    remove_if_present(tmp)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)
        .map_err(io(tmp))?;
    file.write_all(bytes).map_err(io(tmp))?;
    if is_root() {
        fchown(&file, Some(owner.0), Some(owner.1)).map_err(io(tmp))?;
    }
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(io(tmp))?;
    file.sync_all().map_err(io(tmp))
}

/// Renames the staged file `tmp` to `path` and syncs the directory, so the
/// replacement is durable.
pub(crate) fn commit(tmp: &Path, path: &Path) -> Result<()> {
    fs::rename(tmp, path).map_err(io(path))?;
    sync_dir(parent(path))
}

/// Replaces `path` with `bytes` so that a crash leaves either the old file
/// or the new one: a temporary file in the same directory is written,
/// synced and renamed into place, then the directory is synced.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32, owner: Owner) -> Result<()> {
    let tmp = sibling(path, ".laft-tmp");
    stage(&tmp, bytes, mode, owner)?;
    commit(&tmp, path)
}

/// Creates the new file `path` with `bytes` and exactly `mode`, without
/// syncing. For files inside a tree that is published by a rename later.
pub(crate) fn write_new(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(io(path))?;
    file.write_all(bytes).map_err(io(path))?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(io(path))
}

/// Syncs a directory, so that renames and links inside it are durable.
pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir).and_then(|d| d.sync_all()).map_err(io(dir))
}

/// `path` with `suffix` appended to its file name, for example
/// `passwd+` or `hosts.laft-orig`.
pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(suffix);
    path.with_file_name(name)
}

/// Removes the file or symlink `path` unless it is absent.
pub(crate) fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io(path)(e)),
        _ => Ok(()),
    }
}

/// The metadata of `path` without following a symlink, or `None` when it
/// does not exist, including when a directory above it is not one.
pub(crate) fn lstat(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(io(path)(e)),
    }
}

/// The directory that holds `path`. Only `/` has no parent, and it is its
/// own.
pub(crate) fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}

/// `absolute`, an absolute path inside the system, as a path below `root`.
pub(crate) fn under(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}
