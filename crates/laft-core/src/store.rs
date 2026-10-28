use std::fmt;
use std::str::FromStr;

use crate::{Blake3Hash, Name, ParseError, Variant, Version, base32};

/// The directory that holds every store entry.
pub const STORE_DIR: &str = "/laft/store";

/// The content address of a store entry.
///
/// A store id is the first 160 bits of the BLAKE3 hash of an entry's canonical
/// payload manifest, written as 32 lowercase base32 characters. Two entries
/// with the same manifest have the same id, whatever produced them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreId([u8; 20]);

impl StoreId {
    /// The id of an entry whose canonical manifest hashes to `manifest_hash`.
    pub fn from_manifest_hash(manifest_hash: &Blake3Hash) -> Self {
        let mut id = [0u8; 20];
        id.copy_from_slice(&manifest_hash.as_bytes()[..20]);
        Self(id)
    }

    /// The name of the entry's directory under [`STORE_DIR`]:
    /// `<id>-<name>-<version>`, plus `+<variant>` for variants other than
    /// dynamic.
    ///
    /// The name carries the upstream version only, not the revision. Only the
    /// id identifies the entry. The rest is for people reading directory
    /// listings, and tools must never parse it back.
    pub fn dir_name(&self, name: &Name, version: &Version, variant: Variant) -> String {
        match variant {
            Variant::Dynamic => format!("{self}-{name}-{version}"),
            _ => format!("{self}-{name}-{version}+{variant}"),
        }
    }
}

impl FromStr for StoreId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        base32::decode(s).map(Self).ok_or_else(|| {
            ParseError::new("store id", s, "expected 32 lowercase base32 characters")
        })
    }
}

impl fmt::Display for StoreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&base32::encode(&self.0))
    }
}

impl fmt::Debug for StoreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
