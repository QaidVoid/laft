//! Identifiers and formats shared by every Laft component.
//!
//! Package names, versions, variants, package references, dependencies,
//! hashes, store identifiers, timestamps, the static system accounts and
//! canonical JSON live here, so that every tool parses and prints them the
//! same way.

#![forbid(unsafe_code)]

mod account;
mod base32;
mod cjson;
mod dep;
mod error;
mod hash;
mod name;
mod pkgref;
mod store;
mod text_serde;
mod time;
mod variant;
mod version;

pub use account::{system_gid, system_uid};
pub use cjson::{CanonicalJsonError, to_canonical_json};
pub use dep::{Constraint, Constraints, Dep, Op, PcName, Provide, Soname, VersionReq};
pub use error::ParseError;
pub use hash::{Blake3Hash, Sha256Hash};
pub use name::Name;
pub use pkgref::PkgRef;
pub use store::{STORE_DIR, StoreId};
pub use time::Timestamp;
pub use variant::Variant;
pub use version::{PackageVersion, Version, VersionPrefix};
