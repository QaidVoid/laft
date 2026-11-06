//! Filesystem helpers shared by the store, assembly, transactions and GC.

use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::Path;

use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, Timespec, Timestamps};
use rustix::io::Errno;

use crate::error::{Result, corrupt, io};

/// The mode of every directory in a store entry, view or generation.
pub(crate) const SEALED_DIR: u32 = 0o555;

/// Reads at most `max` bytes of the regular file `path`. The file is
/// opened without following a symlink at `path` and without blocking on a
/// FIFO, so a swapped path can neither point the read outside the tree,
/// hang the caller, nor exhaust its memory. A symlink, a non-regular file,
/// or a file larger than `max` bytes is refused.
///
/// # Errors
///
/// Fails when the file cannot be opened or read, with
/// [`io::ErrorKind::InvalidInput`] when `path` is a symlink or not a
/// regular file, and with [`io::ErrorKind::InvalidData`] when it is larger
/// than `max`.
pub fn read_regular(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let file = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|errno| match errno {
        Errno::LOOP => refused(io::ErrorKind::InvalidInput, "is a symlink"),
        errno => io::Error::from(errno),
    })?;
    let stat = rustix::fs::fstat(&file).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(refused(
            io::ErrorKind::InvalidInput,
            "is not a regular file",
        ));
    }
    let size = u64::try_from(stat.st_size).unwrap_or(u64::MAX);
    if size > max {
        return Err(refused(
            io::ErrorKind::InvalidData,
            format!("is {size} bytes, over the limit of {max} bytes"),
        ));
    }
    let mut bytes = Vec::new();
    File::from(file)
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map(|_| bytes)
}

fn refused(kind: io::ErrorKind, reason: impl fmt::Display) -> io::Error {
    io::Error::new(kind, reason.to_string())
}

/// Writes `bytes` to `path` so that a crash leaves either the old file or
/// the new one: a temporary file in the same directory is written, synced
/// and renamed into place, then the directory is synced.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let dir = parent(path);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{}.tmp", name.unwrap_or_default()));
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .map_err(io(&tmp))?;
    file.write_all(bytes).map_err(io(&tmp))?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(io(&tmp))?;
    file.sync_all().map_err(io(&tmp))?;
    fs::rename(&tmp, path).map_err(io(path))?;
    sync_dir(dir)
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

/// Creates directory `path` with exactly `mode`.
pub(crate) fn create_dir(path: &Path, mode: u32) -> Result<()> {
    DirBuilder::new()
        .mode(mode)
        .create(path)
        .map_err(io(path))?;
    set_mode(path, mode)
}

/// Creates directory `path` with `mode` unless it exists.
pub(crate) fn ensure_dir(path: &Path, mode: u32) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_dir(path, mode),
        Err(e) => Err(io(path)(e)),
    }
}

/// Sets the permission bits of `path`, following no symlink because it is
/// only called on directories and files.
pub(crate) fn set_mode(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(io(path))
}

/// Seals a tree before it is published: every directory below `root` gets
/// mode `0o555`, deepest first. With `epoch`, every file, symlink and
/// directory including `root` also gets the modification time 1 second after
/// the epoch.
///
/// `root` keeps its mode, because renaming a directory into another parent
/// needs write permission on it to update its `..` entry. Callers set it
/// after the rename.
pub(crate) fn seal(root: &Path, epoch: bool) -> Result<()> {
    for item in fs::read_dir(root).map_err(io(root))? {
        let path = item.map_err(io(root))?.path();
        let kind = fs::symlink_metadata(&path).map_err(io(&path))?.file_type();
        if kind.is_dir() {
            seal(&path, epoch)?;
            set_mode(&path, SEALED_DIR)?;
        } else if epoch {
            set_epoch_mtime(&path)?;
        }
    }
    if epoch {
        set_epoch_mtime(root)?;
    }
    Ok(())
}

fn set_epoch_mtime(path: &Path) -> Result<()> {
    let epoch = Timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    let times = Timestamps {
        last_access: epoch,
        last_modification: epoch,
    };
    rustix::fs::utimensat(CWD, path, &times, AtFlags::SYMLINK_NOFOLLOW).map_err(io(path))
}

/// Removes a tree whose directories may be read-only. Missing trees are
/// fine.
pub(crate) fn remove_tree(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io(path)(e)),
    };
    if !metadata.is_dir() {
        return fs::remove_file(path).map_err(io(path));
    }
    make_writable(path)?;
    fs::remove_dir_all(path).map_err(io(path))
}

fn make_writable(dir: &Path) -> Result<()> {
    set_mode(dir, 0o700)?;
    for item in fs::read_dir(dir).map_err(io(dir))? {
        let item = item.map_err(io(dir))?;
        if item.file_type().map_err(io(dir))?.is_dir() {
            make_writable(&item.path())?;
        }
    }
    Ok(())
}

/// Removes everything inside `dir`.
pub(crate) fn clear_dir(dir: &Path) -> Result<()> {
    for item in fs::read_dir(dir).map_err(io(dir))? {
        remove_tree(&item.map_err(io(dir))?.path())?;
    }
    Ok(())
}

/// Deletes the published tree `path` so that readers see it either whole or
/// gone: it is renamed to `trash` (in the staging directory of the same
/// filesystem) first, then removed. A symlink at `path` is removed without
/// touching its target. Missing trees are fine.
pub(crate) fn retire(path: &Path, trash: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io(path)(e)),
    };
    if metadata.is_dir() {
        // A cross-directory rename of a directory needs write permission on it.
        set_mode(path, 0o700)?;
    }
    remove_tree(trash)?;
    fs::rename(path, trash).map_err(io(path))?;
    sync_dir(parent(path))?;
    remove_tree(trash)
}

/// Points the symlink `link` at `target` with one rename, creating the new
/// link at `tmp` first.
pub(crate) fn replace_symlink(link: &Path, target: &str, tmp: &Path) -> Result<()> {
    let _ = fs::remove_file(tmp);
    symlink(target, tmp).map_err(io(tmp))?;
    fs::rename(tmp, link).map_err(io(link))?;
    sync_dir(parent(link))
}

/// Flushes every write to the filesystem that holds `path`.
pub(crate) fn sync_fs(path: &Path) -> Result<()> {
    let dir = File::open(path).map_err(io(path))?;
    rustix::fs::syncfs(&dir).map_err(io(path))
}

/// The number at the end of the target of symlink `link`, such as `42` or
/// `mounts/42`, or `None` when `link` does not exist.
///
/// Fails when `link` exists but cannot be read as a symlink, or when its
/// target does not end in a number.
pub(crate) fn link_number(link: &Path) -> Result<Option<u64>> {
    let target = match fs::read_link(link) {
        Ok(target) => target,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(link)(e)),
    };
    target
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(canonical_number)
        .map(Some)
        .ok_or_else(|| corrupt(link, "the link does not name a number"))
}

/// The names of the numbered subdirectories of `dir`, sorted.
pub(crate) fn numbered(dir: &Path) -> Result<Vec<u64>> {
    let mut numbers = Vec::new();
    let items = match fs::read_dir(dir) {
        Ok(items) => items,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(numbers),
        Err(e) => return Err(io(dir)(e)),
    };
    for item in items {
        let item = item.map_err(io(dir))?;
        let name = item.file_name();
        let Some(number) = name.to_str().and_then(canonical_number) else {
            continue;
        };
        if item.file_type().map_err(io(dir))?.is_dir() {
            numbers.push(number);
        }
    }
    numbers.sort_unstable();
    Ok(numbers)
}

/// Parses a positive decimal number without leading zeros.
fn canonical_number(s: &str) -> Option<u64> {
    let n: u64 = s.parse().ok()?;
    (n > 0 && n.to_string() == s).then_some(n)
}

/// The directory that holds `path`. Only `/` has no parent, and it is its
/// own.
fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}
