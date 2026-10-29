//! Filesystem helpers shared by the builder.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{Mode, OFlags, ResolveFlags};

use crate::error::{BuildError, io};

/// Writes `bytes` to `path` so that a crash leaves either the old file or
/// the complete new one: a temporary file in the same directory is written,
/// synced and renamed into place, then the directory is synced.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), BuildError> {
    let dir = dir_of(path);
    fs::create_dir_all(dir).map_err(io(dir))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(io(path))
}

/// Moves the finished file `tmp` to `path` like [`write_atomic`]: syncs it,
/// renames it into place and syncs the directory.
pub(crate) fn publish(tmp: &Path, path: &Path) -> Result<(), BuildError> {
    let dir = dir_of(path);
    File::open(tmp)
        .and_then(|f| f.sync_all())
        .and_then(|()| fs::rename(tmp, path))
        .and_then(|()| File::open(dir)?.sync_all())
        .map_err(io(path))
}

/// The directory that holds `path`. A bare file name, whose parent is
/// empty, is in the working directory.
pub(crate) fn dir_of(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

/// Flushes every write to the filesystem that holds `path`.
pub(crate) fn sync_fs(path: &Path) -> Result<(), BuildError> {
    let dir = File::open(path).map_err(io(path))?;
    rustix::fs::syncfs(&dir).map_err(|e| io(path)(e.into()))
}

/// Removes the tree at `path`, first giving its owner write access to every
/// directory in it. A missing tree is fine.
pub(crate) fn remove_tree(path: &Path) -> Result<(), BuildError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io(path)(e)),
        Ok(m) if !m.is_dir() => return fs::remove_file(path).map_err(io(path)),
        Ok(_) => {}
    }
    writable_dirs(path)?;
    fs::remove_dir_all(path).map_err(io(path))
}

fn writable_dirs(dir: &Path) -> Result<(), BuildError> {
    let mode = fs::symlink_metadata(dir)
        .map_err(io(dir))?
        .permissions()
        .mode();
    if mode & 0o700 != 0o700 {
        fs::set_permissions(dir, fs::Permissions::from_mode(mode | 0o700)).map_err(io(dir))?;
    }
    for item in fs::read_dir(dir).map_err(io(dir))? {
        let item = item.map_err(io(dir))?;
        if item.file_type().map_err(io(dir))?.is_dir() {
            writable_dirs(&item.path())?;
        }
    }
    Ok(())
}

/// The kind of one payload path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Dir,
    File,
    Symlink,
    /// A device, FIFO or socket.
    Other,
}

/// Every path below `root`, relative to it and sorted bytewise, with its
/// kind. Symlinks are not followed.
pub(crate) fn walk(root: &Path) -> Result<Vec<(String, Kind)>, BuildError> {
    let mut out = Vec::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        for item in fs::read_dir(&dir).map_err(io(&dir))? {
            let item = item.map_err(io(&dir))?;
            let name = item.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = item.file_type().map_err(io(&item.path()))?;
            let kind = if kind.is_dir() {
                pending.push((item.path(), rel.clone()));
                Kind::Dir
            } else if kind.is_file() {
                Kind::File
            } else if kind.is_symlink() {
                Kind::Symlink
            } else {
                Kind::Other
            };
            out.push((rel, kind));
        }
    }
    out.sort();
    Ok(out)
}

/// Follows the symlinks of the payload path `rel` inside the payload
/// `root`, where absolute targets are taken relative to `root`, and returns
/// the payload path it ends at. Returns `None` when it leaves the payload,
/// loops or ends nowhere.
pub(crate) fn resolve(root: &Path, rel: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut todo: Vec<String> = rel.split('/').rev().map(str::to_owned).collect();
    let mut hops = 0;
    while let Some(part) = todo.pop() {
        match part.as_str() {
            "" | "." => continue,
            ".." => {
                parts.pop()?;
                continue;
            }
            _ => {}
        }
        parts.push(part);
        let here = root.join(parts.join("/"));
        let metadata = fs::symlink_metadata(&here).ok()?;
        if metadata.is_symlink() {
            hops += 1;
            if hops > 40 {
                return None;
            }
            let target = fs::read_link(&here).ok()?;
            let target = target.to_str()?.to_owned();
            parts.pop();
            if target.starts_with('/') {
                parts.clear();
            }
            todo.extend(target.split('/').rev().map(str::to_owned));
        }
    }
    Some(parts.join("/"))
}

/// Normalizes an absolute path lexically: removes `.` and empty
/// components and applies `..`, which never goes above `/`.
pub(crate) fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str().unwrap_or_default()),
            Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    format!("/{}", parts.join("/"))
}

/// The directory part of the payload path `rel`, `""` at the top.
pub(crate) fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The last component of the payload path `rel`.
pub(crate) fn file_name(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, name)| name)
}

/// Lowercase hexadecimal.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Sets the permission bits of `path`.
pub(crate) fn set_mode(path: &Path, mode: u32) -> Result<(), BuildError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(io(path))
}

/// Replaces the file `path` with `bytes`, keeping its permission bits. The
/// file may be read-only, as long as its directory is writable.
pub(crate) fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), BuildError> {
    let mode = fs::metadata(path).map_err(io(path))?.permissions().mode() & 0o7777;
    let tmp = PathBuf::from(format!("{}.laft-tmp", path.display()));
    fs::write(&tmp, bytes)
        .and_then(|()| fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)))
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(io(path))
}

/// Whether `rel` is a plain relative path: not empty, not absolute, and
/// without empty, `.` or `..` components.
pub(crate) fn is_plain_relative(rel: &str) -> bool {
    rel.split('/')
        .all(|c| !c.is_empty() && c != "." && c != "..")
}

/// Opens the regular file `rel` below `root` for reading. `rel` must be a
/// plain relative path, and no symbolic link on the way may lead out of
/// `root`, so a name a recipe or a build controls never reaches a host
/// file.
pub(crate) fn open_beneath(root: &Path, rel: &str) -> io::Result<File> {
    if !is_plain_relative(rel) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{rel} is not a plain relative path"),
        ));
    }
    let dir = rustix::fs::open(
        root,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    // Nonblocking, so that opening a FIFO returns at once and is refused
    // below instead of waiting for a writer.
    let fd = rustix::fs::openat2(
        &dir,
        rel,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS,
    )?;
    let file = File::from(fd);
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{rel} is not a regular file"),
        ));
    }
    Ok(file)
}

/// Copies the tree `from` to `to`, with symlinks copied as symlinks.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<(), BuildError> {
    fs::create_dir_all(to).map_err(io(to))?;
    for (rel, kind) in walk(from)? {
        let (src, dst) = (from.join(&rel), to.join(&rel));
        match kind {
            Kind::Dir => fs::create_dir_all(&dst).map_err(io(&dst))?,
            Kind::File => {
                fs::copy(&src, &dst).map_err(io(&dst))?;
            }
            Kind::Symlink => {
                let target = fs::read_link(&src).map_err(io(&src))?;
                std::os::unix::fs::symlink(target, &dst).map_err(io(&dst))?;
            }
            Kind::Other => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "fsx_tests.rs"]
mod tests;
