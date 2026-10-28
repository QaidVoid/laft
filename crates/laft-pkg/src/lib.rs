//! Laft package files (`.lpkg`) and the payload manifests inside them.
//!
//! A package file is a pax tar archive compressed with zstd. Its first member
//! is the package metadata (`.laft/meta.json`), its second the payload
//! manifest (`.laft/manifest.json`), and the payload follows in manifest
//! order. Both documents are canonical JSON (RFC 8785). The BLAKE3 hash of the
//! manifest is the package's store id.
//!
//! The builder turns a payload directory into a package file:
//!
//! 1. [`scan`] lists the payload as manifest entries.
//! 2. It fills in a [`Meta`] and builds a [`Manifest`] from its identity, the
//!    hash of its canonical bytes and the entries.
//! 3. [`write_package`] writes a byte-reproducible archive.
//!
//! The store and the client read package files with [`Reader`], which checks
//! every member against the manifest while unpacking.
//!
//! ```
//! use std::fs;
//! use std::os::unix::fs::PermissionsExt;
//!
//! use laft_core::Blake3Hash;
//! use laft_pkg::{Arch, Compression, Manifest, Meta, Reader, scan, write_package};
//!
//! let payload = tempfile::tempdir()?;
//! fs::create_dir_all(payload.path().join("usr/bin"))?;
//! let program = payload.path().join("usr/bin/hello");
//! fs::write(&program, b"#!/bin/sh\necho hello\n")?;
//! fs::set_permissions(&program, fs::Permissions::from_mode(0o555))?;
//!
//! let entries = scan(payload.path())?;
//! let meta = Meta {
//!     format: laft_pkg::FORMAT,
//!     name: "hello".parse()?,
//!     version: "1.0".parse()?,
//!     rev: 1,
//!     variant: Default::default(),
//!     arch: Arch::Any,
//!     origin: "hello".parse()?,
//!     summary: "Print a greeting".into(),
//!     homepage: None,
//!     license: vec!["MIT".into()],
//!     depends: vec!["so:libc.so.6".parse()?],
//!     provides: vec![],
//!     conflicts: vec![],
//!     replaces: vec![],
//!     alternatives: vec![],
//!     triggers: vec![],
//!     static_deps: vec![],
//!     abi: Default::default(),
//!     abi_epoch: 0,
//!     requested_privileges: vec![],
//!     installed_size: 21,
//! };
//! let manifest = Manifest::new(meta.identity(), Blake3Hash::of(&meta.to_json()?), entries)?;
//! let file = write_package(Vec::new(), payload.path(), &meta, &manifest, Compression::DEVELOPMENT)?;
//!
//! let reader = Reader::new(file.as_slice())?;
//! assert_eq!(reader.manifest().store_id(), manifest.store_id());
//! let unpacked = tempfile::tempdir()?;
//! reader.unpack(unpacked.path())?;
//! assert_eq!(fs::read(unpacked.path().join("usr/bin/hello"))?, b"#!/bin/sh\necho hello\n");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

use laft_core::Blake3Hash;

mod decompress;
mod error;
mod manifest;
mod meta;
mod path;
mod read;
mod scan;
mod tar;
#[cfg(test)]
mod test_support;
mod write;

pub use error::{Error, ErrorKind};
pub use manifest::{Arch, Entry, FORMAT, Identity, Manifest};
pub use meta::{Abi, Alternative, Meta, PrivRequest, StaticDep};
pub use read::Reader;
pub use scan::scan;
pub use write::{Compression, write_package};

/// Copies up to `size` bytes from `src` to `dst` and hashes them. Returns the
/// number of bytes copied, which is less than `size` only when `src` ends
/// early, and their hash.
fn copy_hashed(
    src: &mut impl Read,
    dst: &mut impl Write,
    size: u64,
) -> io::Result<(u64, Blake3Hash)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0; 64 * 1024];
    let mut copied = 0;
    while copied < size {
        let want = usize::try_from(size - copied).map_or(buf.len(), |left| left.min(buf.len()));
        let n = match src.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n])?;
        copied += n as u64;
    }
    Ok((
        copied,
        Blake3Hash::from_bytes(*hasher.finalize().as_bytes()),
    ))
}
