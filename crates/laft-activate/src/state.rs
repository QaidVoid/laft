//! Paths below a root and the records Laft keeps about `/etc` in
//! `laft/state`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use laft_core::Blake3Hash;
use laft_store::Root;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Result, corrupt, io};
use crate::fsx;

/// The version of every state record this crate writes.
pub(crate) const FORMAT: u32 = 1;

/// The live `/etc` of `root`.
pub(crate) fn etc_dir(root: &Root) -> PathBuf {
    root.path().join("etc")
}

/// The directory of generation `number`.
pub(crate) fn gen_dir(root: &Root, number: u64) -> PathBuf {
    root.path().join("laft/gens").join(number.to_string())
}

/// The file `name` in `laft/state`.
pub(crate) fn state_file(root: &Root, name: &str) -> PathBuf {
    root.path().join("laft/state").join(name)
}

/// A versioned state record.
pub(crate) trait Record: Serialize + DeserializeOwned + Default {
    /// The file name in `laft/state`.
    const NAME: &'static str;

    /// The format version the record was read with.
    fn version(&self) -> u32;

    /// Reads the record, or returns an empty one when the file does not
    /// exist.
    fn load(root: &Root) -> Result<Self> {
        let path = state_file(root, Self::NAME);
        match fs::read(&path) {
            Ok(bytes) => {
                let record: Self = serde_json::from_slice(&bytes).map_err(|e| corrupt(&path, e))?;
                if record.version() != FORMAT {
                    return Err(corrupt(&path, "unsupported format version"));
                }
                Ok(record)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(io(&path)(e)),
        }
    }

    /// Writes the record as canonical JSON, replacing the old one
    /// atomically.
    fn save(&self, root: &Root) -> Result<()> {
        write_json(&state_file(root, Self::NAME), self, 0o644)
    }
}

/// Writes `value` to `path` as canonical JSON, replacing the old file
/// atomically, owned by root.
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T, mode: u32) -> Result<()> {
    let bytes = laft_core::to_canonical_json(value)
        .map_err(|e| corrupt(path, format!("the record is not canonical JSON: {e}")))?;
    fsx::write_atomic(path, &bytes, mode, (0, 0))
}

/// The hashes of the copy-mode files Laft wrote, stored as
/// `etc-copies.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CopiesRecord {
    pub(crate) version: u32,
    /// The hash of the content Laft last wrote, by path relative to `/etc`.
    pub(crate) files: BTreeMap<String, Blake3Hash>,
    /// The hash of content Laft is about to write. It is recorded before
    /// the write, so that a crash between the write and the update of
    /// `files` does not make Laft's own content look changed by someone
    /// else.
    pub(crate) pending: BTreeMap<String, Blake3Hash>,
}

impl Default for CopiesRecord {
    fn default() -> Self {
        Self {
            version: FORMAT,
            files: BTreeMap::new(),
            pending: BTreeMap::new(),
        }
    }
}

impl Record for CopiesRecord {
    const NAME: &'static str = "etc-copies.json";

    fn version(&self) -> u32 {
        self.version
    }
}

impl CopiesRecord {
    /// Records that `hash` was written at `path`.
    pub(crate) fn finish(&mut self, path: &str, hash: Blake3Hash) {
        self.pending.remove(path);
        self.files.insert(path.to_owned(), hash);
    }
}
