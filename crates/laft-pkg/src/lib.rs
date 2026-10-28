//! Laft package metadata and the payload manifests of package files.
//!
//! A package's metadata ([`Meta`]) and payload manifest ([`Manifest`]) are
//! stored as canonical JSON (RFC 8785). The BLAKE3 hash of the manifest is
//! the package's store id.
//!
//! The builder turns a payload directory into a manifest:
//!
//! 1. [`scan`] lists the payload as manifest entries.
//! 2. It fills in a [`Meta`] and builds a [`Manifest`] from its identity, the
//!    hash of its canonical bytes and the entries.

#![forbid(unsafe_code)]

mod error;
mod manifest;
mod meta;
mod path;
mod scan;
#[cfg(test)]
mod test_support;

pub use error::{Error, ErrorKind};
pub use manifest::{Arch, Entry, FORMAT, Identity, Manifest};
pub use meta::{Abi, Alternative, Meta, PrivRequest, StaticDep};
pub use scan::scan;
