//! Crash-safe file operations on repository trees.
//!
//! Every file is written to a temporary file in its directory, synced and
//! renamed into place, and the directory is synced after the rename. A
//! crash therefore leaves either the old file or the new one, never a
//! partial file under the final name.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use laft_core::Blake3Hash;

use crate::error::{Error, Result, io};

/// The mode of every file `laft-repo` writes except secret keys.
pub(crate) const FILE_MODE: u32 = 0o644;

/// Writes `bytes` to `path` crash-safely with mode `mode`, creating the
/// missing parent directories.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    write_with(path, mode, |file| file.write_all(bytes))
}

/// Copies the file `from` to `to` crash-safely, creating the missing
/// parent directories of `to`.
pub(crate) fn copy_atomic(from: &Path, to: &Path) -> Result<()> {
    let mut source = File::open(from).map_err(io(from))?;
    write_with(to, FILE_MODE, |file| io::copy(&mut source, file).map(drop))
}

fn write_with(
    path: &Path,
    mode: u32,
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<()> {
    let dir = parent(path);
    ensure_dir(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{}.tmp", name.unwrap_or_default()));
    remove(&tmp)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .map_err(io(&tmp))?;
    let written = fill(&mut file)
        .and_then(|()| file.set_permissions(fs::Permissions::from_mode(mode)))
        .and_then(|()| file.sync_all());
    if let Err(e) = written {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(io(&tmp)(e));
    }
    fs::rename(&tmp, path).map_err(io(path))?;
    sync_dir(dir)
}

/// Creates `dir` and its missing parents, syncing each parent after a
/// directory is created in it, so that the new directories survive a
/// crash.
pub(crate) fn ensure_dir(dir: &Path) -> Result<()> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    let parent = parent(dir);
    ensure_dir(parent)?;
    match fs::create_dir(dir) {
        Ok(()) => sync_dir(parent),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        Err(e) => Err(io(dir)(e)),
    }
}

/// Removes the file `path`, syncing its directory. Returns whether it
/// existed.
pub(crate) fn remove(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => {
            sync_dir(parent(path))?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io(path)(e)),
    }
}

/// Syncs the directory `dir`, which makes renames and removals in it
/// durable.
pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir).and_then(|d| d.sync_all()).map_err(io(dir))
}

/// Reads `path`, or returns `None` when it does not exist.
pub(crate) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path)(e)),
    }
}

/// The size and BLAKE3 hash of the file `path`, read in a streaming
/// fashion so that large package files never sit in memory.
pub(crate) fn hash_file(path: &Path) -> Result<(u64, Blake3Hash)> {
    let mut file = File::open(path).map_err(io(path))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0; 256 * 1024];
    let mut size = 0;
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io(path)(e)),
        };
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((size, Blake3Hash::from_bytes(*hasher.finalize().as_bytes())))
}

/// The names of the entries of `dir`, sorted, or nothing when it does not
/// exist. Names that are not UTF-8 are skipped, because no repository file
/// has one.
pub(crate) fn list(dir: &Path) -> Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(dir)(e)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io(dir))?;
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// The size of the file `path`, or `None` when it does not exist.
pub(crate) fn size(path: &Path) -> Result<Option<u64>> {
    match fs::metadata(path) {
        Ok(m) if m.is_file() => Ok(Some(m.len())),
        Ok(_) => Err(Error::new(format!(
            "{} is not a regular file",
            path.display()
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path)(e)),
    }
}

fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

#[cfg(test)]
#[path = "fsx_tests.rs"]
mod tests;
