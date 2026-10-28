//! `laft-repo keys`: maintains `keys.json`, the list of online signing
//! keys that a root key signs.

use laft_core::{Name, Timestamp};
use laft_index::{KeyId, Keys, OnlineKey, PublicKey};

use crate::error::{Error, Result};
use crate::fsx::{self, FILE_MODE};
use crate::layout::{KEYS, Repository, sig};
use crate::sign::Signer;

/// A change to `keys.json`.
pub(crate) enum Change {
    /// Lists an online key, valid for indexes signed in the window.
    Add {
        /// The public key.
        public: PublicKey,
        /// The earliest signing time it is valid for.
        not_before: Timestamp,
        /// The latest signing time it is valid for.
        not_after: Timestamp,
    },
    /// Stops listing a key, as the last step of a planned rotation.
    Remove(KeyId),
    /// Revokes a key for good, removing it from the list.
    Revoke {
        /// The key.
        id: KeyId,
        /// The last known good index generation, which lowers the
        /// accepted generation of clients that trusted an index signed by
        /// the key.
        generation_floor: Option<u64>,
    },
}

/// Applies `change` to the repository's `keys.json`, creating it for
/// repository `name` when it does not exist, increases its version and
/// writes it, signed with the root key `signer` when one is given.
///
/// An unsigned `keys.json` loses its old signature before it is replaced.
/// `laft-repo sign --keys` signs it later.
pub(crate) fn run(
    repo: &Repository,
    name: Option<&Name>,
    change: Change,
    signer: Option<&Signer>,
) -> Result<Keys> {
    let path = repo.path(KEYS);
    let mut keys = match fsx::read_optional(&path)? {
        Some(bytes) => {
            let keys = Keys::from_json(&bytes)
                .map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
            if let Some(name) = name
                && *name != keys.repo
            {
                return Err(Error::new(format!(
                    "{KEYS} belongs to repository {}, not {name}",
                    keys.repo
                )));
            }
            keys
        }
        None => Keys {
            format: Keys::FORMAT,
            repo: name.cloned().ok_or_else(|| {
                Error::new(format!("there is no {KEYS} yet, so --name is required"))
            })?,
            version: 0,
            keys: Vec::new(),
            revoked: Vec::new(),
            generation_floor: None,
        },
    };
    match change {
        Change::Add {
            public,
            not_before,
            not_after,
        } => {
            let id = public.id();
            if keys.is_revoked(id) {
                return Err(Error::new(format!("key {id} is revoked")));
            }
            keys.keys.retain(|k| k.id != id);
            keys.keys.push(OnlineKey {
                id,
                public,
                not_before,
                not_after,
            });
        }
        Change::Remove(id) => {
            if keys.key(id).is_none() {
                return Err(Error::new(format!("key {id} is not listed")));
            }
            keys.keys.retain(|k| k.id != id);
        }
        Change::Revoke {
            id,
            generation_floor,
        } => {
            if keys.is_revoked(id) {
                return Err(Error::new(format!("key {id} is already revoked")));
            }
            keys.keys.retain(|k| k.id != id);
            keys.revoked.push(id);
            keys.revoked.sort();
            if generation_floor.is_some() {
                keys.generation_floor = generation_floor;
            }
        }
    }
    keys.keys.sort_by_key(|k| k.id);
    keys.version += 1;
    let bytes = keys.to_json()?;
    let signature = signer
        .map(|signer| root_signature(&keys, &bytes, signer))
        .transpose()?;
    if signature.is_none() {
        fsx::remove(&repo.path(&sig(KEYS)))?;
    }
    fsx::write_atomic(&path, &bytes, FILE_MODE)?;
    if let Some(signature) = signature {
        fsx::write_atomic(&repo.path(&sig(KEYS)), &signature, FILE_MODE)?;
    }
    Ok(keys)
}

/// Signs the repository's `keys.json` with the root key `signer`.
pub(crate) fn sign(repo: &Repository, signer: &Signer) -> Result<Keys> {
    let path = repo.path(KEYS);
    let bytes = fsx::read_optional(&path)?
        .ok_or_else(|| Error::new(format!("{} does not exist", path.display())))?;
    let keys =
        Keys::from_json(&bytes).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
    let signature = root_signature(&keys, &bytes, signer)?;
    fsx::write_atomic(&repo.path(&sig(KEYS)), &signature, FILE_MODE)?;
    Ok(keys)
}

/// Signs `bytes`, the document `keys`, refusing a key that the document
/// lists or revokes, because only a root key signs it.
fn root_signature(keys: &Keys, bytes: &[u8], signer: &Signer) -> Result<Vec<u8>> {
    let id = signer.id();
    if keys.key(id).is_some() || keys.is_revoked(id) {
        return Err(Error::new(format!(
            "key {id} is an online key of {KEYS}, but only a root key signs it"
        )));
    }
    signer.sign(bytes, &keys.comment().to_string())
}
