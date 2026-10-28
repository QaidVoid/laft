//! Signed Laft repository indexes and the client trust model.
//!
//! A repository publishes, for each channel and architecture, an [`Index`]
//! of its packages as zstd-compressed canonical JSON, signed with minisign.
//! The trusted comment of the signature ([`IndexComment`]) names the
//! repository, channel, architecture and generation, so a client rejects a
//! stale or substituted index before decompressing it.
//!
//! Keys form two levels. Root keys are configured on the client and sign
//! only `keys.json` ([`Keys`]), which lists the online keys that sign
//! indexes, their validity windows and revoked keys. A repository without
//! `keys.json` runs in direct mode and signs indexes with a root key.
//!
//! [`IndexState`] keeps what a client accepted, one record per repository
//! for `keys.json` and one per channel and architecture for indexes, and
//! refuses anything that would go back: a lower index generation, a lower
//! `keys.json` version, a `keys.json` that disappears, a revoked key or an
//! expired index. This crate reads downloaded bytes and does no networking.
//!
//! A publisher fills in an [`Index`] or [`Keys`], encodes it with
//! [`Index::encode`] or [`Keys::to_json`], and signs the bytes with minisign
//! using the trusted comment from [`Index::comment`] or [`Keys::comment`].
//!
//! ```no_run
//! use laft_core::Timestamp;
//! use laft_index::{IndexState, KeysFetch, Repo};
//!
//! # fn fetch(_: &str) -> Option<Vec<u8>> { None }
//! let state = IndexState::new("/laft/state/index");
//! let repo = Repo {
//!     name: "core".parse()?,
//!     channel: "current".parse()?,
//!     arch: laft_pkg::Arch::X86_64V3,
//!     key: "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3".parse()?,
//!     keys: vec![],
//! };
//! let keys = fetch("keys.json").zip(fetch("keys.json.minisig"));
//! let trust = match &keys {
//!     Some((json, signature)) => state.accept_keys(&repo, KeysFetch::Found { json, signature })?,
//!     None => state.accept_keys(&repo, KeysFetch::NotFound)?,
//! };
//! let index = fetch("channels/current/x86_64-v3/index.json.zst").unwrap_or_default();
//! let signature = fetch("channels/current/x86_64-v3/index.json.zst.minisig").unwrap_or_default();
//! let accepted = state.accept_index(&repo, &trust, &index, &signature, Timestamp::now(), false)?;
//! println!("generation {}", accepted.record.generation);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

mod comment;
mod error;
mod index;
mod key;
mod keys;
mod state;
#[cfg(test)]
mod test_support;

pub use comment::{IndexComment, KeysComment};
pub use error::{Error, ErrorKind};
pub use index::{
    Defaults, Index, IndexEntry, MAX_INDEX_SIZE, MAX_VALIDITY_SECS, Yank, next_generation,
};
pub use key::{KeyId, PublicKey, Signature};
pub use keys::{Keys, OnlineKey};
pub use state::{AcceptedIndex, IndexRecord, IndexState, KeysFetch, KeysRecord, Repo, Trust};

/// The largest integer that canonical JSON represents exactly.
const MAX_SAFE_INTEGER: u64 = 1 << 53;
