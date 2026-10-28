use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use laft_core::Blake3Hash;

use crate::decompress::MAX_WINDOW_LOG;
use crate::tar::{self, Header, Kind};
use crate::{Entry, Error, Manifest, Meta};

/// The path of the metadata member.
pub(crate) const META_PATH: &str = ".laft/meta.json";
/// The path of the manifest member.
pub(crate) const MANIFEST_PATH: &str = ".laft/manifest.json";
/// The mode of the metadata and manifest members.
const DOCUMENT_MODE: u32 = 0o444;

/// How a package file is compressed.
///
/// The output is always one zstd frame with the content checksum flag set,
/// and never needs a window above 128 MiB to decompress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compression {
    /// The zstd compression level.
    pub level: i32,
    /// Whether to enable long distance matching with a 128 MiB window, which
    /// shrinks large payloads with repeated content at the cost of memory.
    pub long_distance: bool,
}

impl Compression {
    /// For packages that are published: level 19 with long distance
    /// matching.
    pub const PUBLISH: Self = Self {
        level: 19,
        long_distance: true,
    };

    /// For local development builds: level 3, which is much faster.
    pub const DEVELOPMENT: Self = Self {
        level: 3,
        long_distance: false,
    };
}

/// Writes the package file for `manifest` and `meta` to `out`, reading file
/// content from the payload directory `payload`, and returns `out`.
///
/// The archive is byte-reproducible: it depends only on the metadata, the
/// manifest, the file contents and `compression`. Header fields come from the
/// manifest, never from the payload directory, and every other field is
/// fixed. A file whose content, size, mode, owner and group equal those of an
/// earlier file is stored as a hard link to it.
///
/// # Errors
///
/// Fails when the metadata is invalid or is not the one the manifest names,
/// when a payload file is missing, is not a regular file or differs from its
/// manifest entry in size or content, or when writing fails. The error names
/// the package and the path.
pub fn write_package<W: Write>(
    out: W,
    payload: &Path,
    meta: &Meta,
    manifest: &Manifest,
    compression: Compression,
) -> Result<W, Error> {
    let meta_bytes = meta.to_json()?;
    manifest.check_meta(meta, &meta_bytes)?;
    let package = manifest.package();
    write_archive(out, payload, &meta_bytes, manifest, compression)
        .map_err(|e| e.in_package(package))
}

fn write_archive<W: Write>(
    out: W,
    payload: &Path,
    meta_bytes: &[u8],
    manifest: &Manifest,
    compression: Compression,
) -> Result<W, Error> {
    let mut encoder = zstd::stream::write::Encoder::new(out, compression.level)?;
    encoder.include_checksum(true)?;
    if compression.long_distance {
        encoder.long_distance_matching(true)?;
        encoder.window_log(MAX_WINDOW_LOG)?;
    }
    let mut archive = Archive {
        out: encoder,
        header: Vec::new(),
    };
    archive.document(META_PATH, meta_bytes)?;
    archive.document(MANIFEST_PATH, &manifest.to_json())?;
    let mut written: HashMap<(Blake3Hash, u64, u32, u32, u32), &str> = HashMap::new();
    for entry in manifest.entries() {
        archive
            .entry(payload, entry, &mut written)
            .map_err(|e| e.at(entry.path()))?;
    }
    archive.out.write_all(&[0; 2 * tar::BLOCK])?;
    Ok(archive.out.finish()?)
}

struct Archive<W: Write> {
    out: W,
    header: Vec<u8>,
}

impl<W: Write> Archive<W> {
    fn header(&mut self, header: &Header) -> Result<(), Error> {
        self.header.clear();
        tar::encode(header, &mut self.header);
        self.out.write_all(&self.header)?;
        Ok(())
    }

    fn pad(&mut self, size: u64) -> Result<(), Error> {
        self.out.write_all(&[0; tar::BLOCK][..tar::padding(size)])?;
        Ok(())
    }

    fn document(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        self.header(&Header {
            kind: Kind::File,
            path: path.into(),
            link: Vec::new(),
            mode: DOCUMENT_MODE,
            uid: 0,
            gid: 0,
            size: bytes.len() as u64,
        })?;
        self.out.write_all(bytes)?;
        self.pad(bytes.len() as u64)
    }

    fn entry<'m>(
        &mut self,
        payload: &Path,
        entry: &'m Entry,
        written: &mut HashMap<(Blake3Hash, u64, u32, u32, u32), &'m str>,
    ) -> Result<(), Error> {
        let (uid, gid) = entry.ids()?;
        let path = entry.path();
        let mut header = Header {
            kind: Kind::Dir,
            path: path.into(),
            link: Vec::new(),
            mode: 0,
            uid,
            gid,
            size: 0,
        };
        match entry {
            Entry::Dir { mode, .. } => {
                header.mode = *mode;
                self.header(&header)
            }
            Entry::Symlink { target, .. } => {
                header.kind = Kind::Symlink;
                header.mode = 0o777;
                header.link = target.as_bytes().to_vec();
                self.header(&header)
            }
            Entry::File {
                mode, size, hash, ..
            } => {
                header.mode = *mode;
                if let Some(first) = written.get(&(*hash, *size, *mode, uid, gid)) {
                    header.kind = Kind::HardLink;
                    header.link = first.as_bytes().to_vec();
                    return self.header(&header);
                }
                written.insert((*hash, *size, *mode, uid, gid), path);
                header.kind = Kind::File;
                header.size = *size;
                let source = payload.join(path);
                if !fs::symlink_metadata(&source)?.is_file() {
                    return Err(Error::invalid(
                        "the manifest lists a file, but the payload has something else",
                    ));
                }
                let mut file = File::open(&source)?;
                self.header(&header)?;
                let (copied, actual) = crate::copy_hashed(&mut file, &mut self.out, *size)?;
                if copied < *size || file.read(&mut [0])? > 0 {
                    return Err(Error::invalid(
                        "the payload file changed size after it was scanned",
                    ));
                }
                if actual != *hash {
                    return Err(Error::mismatch("content hash", hash, actual));
                }
                self.pad(*size)
            }
        }
    }
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
