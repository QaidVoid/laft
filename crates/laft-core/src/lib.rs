//! Identifiers and formats shared by every Laft component.
//!
//! Package names, versions, variants, package references, hashes, store
//! identifiers and canonical JSON live here, so that every tool parses and
//! prints them the same way.

#![forbid(unsafe_code)]

mod base32;
mod cjson;
mod error;
mod hash;
mod name;
mod pkgref;
mod store;
mod variant;
mod version;

pub use cjson::{CanonicalJsonError, to_canonical_json};
pub use error::ParseError;
pub use hash::{Blake3Hash, Sha256Hash};
pub use name::Name;
pub use pkgref::PkgRef;
pub use store::{STORE_DIR, StoreId};
pub use variant::Variant;
pub use version::{PackageVersion, Version, VersionPrefix};
