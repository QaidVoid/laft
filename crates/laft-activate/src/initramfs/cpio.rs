//! A deterministic writer of SVR4 "newc" cpio archives, the format the
//! kernel unpacks an initramfs from.
//!
//! Every entry has uid 0, gid 0 and mtime 0, entries are sorted by path in
//! byte order with every parent directory before its children, and inode
//! numbers count up from 1 in that order. The same entries therefore always
//! give the same bytes.

use std::collections::BTreeMap;

/// The magic number of a newc header.
const MAGIC: &str = "070701";

/// The name of the entry that ends an archive.
const TRAILER: &str = "TRAILER!!!";

/// What an archive entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A directory.
    Dir,
    /// A regular file with this content.
    File(Vec<u8>),
    /// A symlink with this target.
    Symlink(String),
    /// A character device with this major and minor number.
    CharDevice(u32, u32),
}

/// A cpio archive being built, by path without a leading `/`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Archive {
    entries: BTreeMap<String, (u32, Kind)>,
}

impl Archive {
    /// Adds `path` with permission bits `mode`, creating missing parent
    /// directories with mode `0o755`. A later entry at the same path
    /// replaces an earlier one.
    pub(crate) fn add(&mut self, path: &str, mode: u32, kind: Kind) {
        let path = path.trim_start_matches('/');
        let mut parent = path;
        while let Some((dir, _)) = parent.rsplit_once('/') {
            self.entries
                .entry(dir.to_owned())
                .or_insert((0o755, Kind::Dir));
            parent = dir;
        }
        self.entries.insert(path.to_owned(), (mode, kind));
    }

    /// Whether the archive has an entry at `path`.
    pub(crate) fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path.trim_start_matches('/'))
    }

    /// Whether the archive has no entries.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The archive's bytes, ending with the trailer entry.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (ino, (path, (mode, kind))) in (1u32..).zip(&self.entries) {
            let (format, nlink, data, rdev): (u32, u32, &[u8], (u32, u32)) = match kind {
                Kind::Dir => (0o040000, 2, &[], (0, 0)),
                Kind::File(bytes) => (0o100000, 1, bytes, (0, 0)),
                Kind::Symlink(target) => (0o120000, 1, target.as_bytes(), (0, 0)),
                Kind::CharDevice(major, minor) => (0o020000, 1, &[], (*major, *minor)),
            };
            header(&mut out, ino, format | mode, nlink, data.len(), rdev, path);
            out.extend_from_slice(data);
            pad(&mut out);
        }
        header(&mut out, 0, 0, 1, 0, (0, 0), TRAILER);
        out
    }
}

/// Appends a header with its name and the padding after the name.
fn header(
    out: &mut Vec<u8>,
    ino: u32,
    mode: u32,
    nlink: u32,
    size: usize,
    rdev: (u32, u32),
    name: &str,
) {
    let fields = [
        ino,
        mode,
        0,
        0,
        nlink,
        0,
        size as u32,
        0,
        0,
        rdev.0,
        rdev.1,
        name.len() as u32 + 1,
        0,
    ];
    out.extend_from_slice(MAGIC.as_bytes());
    for field in fields {
        out.extend_from_slice(format!("{field:08X}").as_bytes());
    }
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    pad(out);
}

/// Pads `out` with zero bytes to a multiple of 4.
fn pad(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

#[cfg(test)]
#[path = "cpio_tests.rs"]
mod tests;
