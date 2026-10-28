use std::collections::BTreeSet;

use laft_core::{Name, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{Error, KeyId, KeysComment, MAX_SAFE_INTEGER, PublicKey};

/// The online signing keys of a repository, published as `keys.json` and
/// signed by a root key.
///
/// Root keys are configured on the client and kept offline. They sign only
/// this document. The online keys it lists sign indexes, each within its
/// validity window. A key in `revoked` is never accepted again, and the
/// increasing `version` stops an old `keys.json` from reviving it.
///
/// The document is canonical JSON (RFC 8785). [`Self::to_json`] produces
/// the published bytes and [`Self::comment`] the trusted comment to sign
/// them with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    /// The format version, always [`Self::FORMAT`].
    pub format: u32,
    /// The repository name.
    pub repo: Name,
    /// Strictly increasing on every change. At most 2^53.
    pub version: u64,
    /// The online keys, each with a distinct id.
    pub keys: Vec<OnlineKey>,
    /// Ids of keys that are never accepted, even when listed in `keys` or
    /// in an older `keys.json`.
    pub revoked: Vec<KeyId>,
    /// The last known good index generation, set when a key is revoked.
    ///
    /// A client whose accepted index was signed by a revoked key lowers its
    /// accepted generation to this value, so that an index with an inflated
    /// generation signed by a leaked key cannot freeze it.
    pub generation_floor: Option<u64>,
}

/// An online signing key and the window in which its signatures are valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnlineKey {
    /// The key id, which must match `public`.
    pub id: KeyId,
    /// The public key.
    pub public: PublicKey,
    /// The earliest index signing time the key is valid for.
    pub not_before: Timestamp,
    /// The latest index signing time the key is valid for.
    pub not_after: Timestamp,
}

impl Keys {
    /// The `keys.json` format version this crate reads and writes.
    pub const FORMAT: u32 = 1;

    /// The trusted comment to sign the document with.
    pub fn comment(&self) -> KeysComment {
        KeysComment {
            repo: self.repo.clone(),
            version: self.version,
        }
    }

    /// The listed key with id `id`, revoked or not.
    pub fn key(&self, id: KeyId) -> Option<&OnlineKey> {
        self.keys.iter().find(|k| k.id == id)
    }

    /// Whether `id` is revoked.
    pub fn is_revoked(&self, id: KeyId) -> bool {
        self.revoked.contains(&id)
    }

    /// Checks every rule of the format.
    ///
    /// # Errors
    ///
    /// Fails, naming the rule, when one is broken.
    pub fn validate(&self) -> Result<(), Error> {
        let err = |reason: String| Error::invalid("keys.json", reason);
        if self.format != Self::FORMAT {
            return Err(err(format!(
                "format {} is not supported, only {} is",
                self.format,
                Self::FORMAT
            )));
        }
        if self.version > MAX_SAFE_INTEGER
            || self.generation_floor.is_some_and(|g| g > MAX_SAFE_INTEGER)
        {
            return Err(err(
                "`version` and `generation_floor` are at most 2^53".into()
            ));
        }
        let mut ids = BTreeSet::new();
        for key in &self.keys {
            if key.id != key.public.id() {
                return Err(err(format!(
                    "key {} has a public key with id {}",
                    key.id,
                    key.public.id()
                )));
            }
            if key.not_before > key.not_after {
                return Err(err(format!(
                    "key {} becomes valid after it stops being valid",
                    key.id
                )));
            }
            if !ids.insert(key.id) {
                return Err(err(format!("key {} is listed twice", key.id)));
            }
        }
        let mut revoked = BTreeSet::new();
        if let Some(id) = self.revoked.iter().find(|id| !revoked.insert(**id)) {
            return Err(err(format!("key {id} is revoked twice")));
        }
        Ok(())
    }

    /// The document as canonical JSON (RFC 8785).
    ///
    /// # Errors
    ///
    /// Fails when the document is not valid.
    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        Ok(laft_core::to_canonical_json(self)
            .expect("a valid keys.json has no integer above 2^53, so it is canonical JSON"))
    }

    /// Parses a document and validates it.
    ///
    /// # Errors
    ///
    /// Fails when `bytes` are not the JSON of a valid `keys.json`.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let keys: Self =
            serde_json::from_slice(bytes).map_err(|e| Error::invalid("keys.json", e))?;
        keys.validate()?;
        Ok(keys)
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
