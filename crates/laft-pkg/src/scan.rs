use std::fs::{self, File};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use laft_core::Blake3Hash;

use crate::{Entry, Error};

/// Lists every path under the payload directory `root` as manifest entries,
/// sorted by path.
///
/// Files are hashed with BLAKE3. Modes are recorded as found, limited to the
/// permission bits, so the caller clears write, setuid and setgid bits on disk
/// first. Owners and groups are left as root, because the ids of a build
/// directory say nothing about the accounts a package needs, and the caller
/// sets them on the entries. `root` itself is not an entry.
///
/// The result is not checked against the manifest rules. [`crate::Manifest::new`]
/// does that.
///
/// # Errors
///
/// Fails when the tree cannot be read, contains a device, FIFO or socket, or
/// has a path or symlink target that is not valid UTF-8. The error names the
/// path.
pub fn scan(root: &Path) -> Result<Vec<Entry>, Error> {
    let mut entries = Vec::new();
    let mut pending: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let at = |e: io::Error| Error::from(e).at(&prefix);
        for item in fs::read_dir(&dir).map_err(at)? {
            let item = item.map_err(at)?;
            let name = item.file_name().into_string().map_err(|name| {
                Error::invalid("the path is not valid UTF-8")
                    .at(&format!("{prefix}/{}", name.to_string_lossy()))
            })?;
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let full = item.path();
            let metadata = item.metadata().map_err(|e| Error::from(e).at(&path))?;
            let mode = metadata.permissions().mode() & 0o7777;
            let kind = metadata.file_type();
            if kind.is_dir() {
                entries.push(Entry::Dir {
                    path: path.clone(),
                    mode,
                    owner: None,
                    group: None,
                });
                pending.push((full, path));
            } else if kind.is_file() {
                let (size, hash) = hash_file(&full).map_err(|e| Error::from(e).at(&path))?;
                entries.push(Entry::File {
                    path,
                    mode,
                    size,
                    hash,
                    owner: None,
                    group: None,
                });
            } else if kind.is_symlink() {
                let target = fs::read_link(&full)
                    .map_err(|e| Error::from(e).at(&path))?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| {
                        Error::invalid("the symlink target is not valid UTF-8").at(&path)
                    })?;
                entries.push(Entry::Symlink { path, target });
            } else {
                return Err(
                    Error::invalid("devices, FIFOs and sockets cannot be packaged").at(&path),
                );
            }
        }
    }
    entries.sort_by(|a, b| a.path().cmp(b.path()));
    Ok(entries)
}

fn hash_file(path: &Path) -> io::Result<(u64, Blake3Hash)> {
    let mut hasher = blake3::Hasher::new();
    let size = io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok((size, Blake3Hash::from_bytes(*hasher.finalize().as_bytes())))
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
