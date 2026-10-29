//! The records of finished builds, which plan the next ones.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use laft_core::Blake3Hash;
use serde::{Deserialize, Serialize};

use crate::error::{BuildError, io};
use crate::{BuildRecord, Item, fsx};

/// A successful build as the database keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stored {
    /// What planning needs: the key, its inputs, the output store ids and
    /// the imports.
    pub record: BuildRecord,
    /// The file name of the build's buildinfo in the output directory.
    pub buildinfo: String,
    /// The file names of the packages in the output directory, the main
    /// package first.
    pub files: Vec<String>,
}

/// The build database: `builds/<key>.json` for every successful build and
/// `latest/<name>+<variant>`, a symlink to the newest one of each item.
///
/// Records are canonical JSON and are written with a rename, so a crash
/// leaves the previous state.
#[derive(Debug, Clone)]
pub struct Db {
    dir: PathBuf,
}

impl Db {
    /// The database in the directory `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory of the database.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn key_file(key: &Blake3Hash) -> String {
        let key = key.to_string();
        format!("{}.json", key.trim_start_matches("b3:"))
    }

    fn latest_path(&self, item: &Item) -> PathBuf {
        self.dir
            .join("latest")
            .join(format!("{}+{}", item.name, item.variant))
    }

    /// The newest successful build of `item`, if there is one.
    ///
    /// # Errors
    ///
    /// Fails when the record exists but cannot be read.
    pub fn latest(&self, item: &Item) -> Result<Option<Stored>, BuildError> {
        read(&self.latest_path(item))
    }

    /// The successful build with the build key `key`, if there is one.
    ///
    /// # Errors
    ///
    /// Fails when the record exists but cannot be read.
    pub fn find(&self, key: &Blake3Hash) -> Result<Option<Stored>, BuildError> {
        read(&self.dir.join("builds").join(Self::key_file(key)))
    }

    /// Records a successful build and makes it the newest of its item.
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be written.
    pub fn record(&self, stored: &Stored) -> Result<(), BuildError> {
        let file = Self::key_file(&stored.record.key);
        let path = self.dir.join("builds").join(&file);
        let json = laft_core::to_canonical_json(stored)
            .map_err(|e| BuildError::problem("B023", format!("{}: {e}", path.display())))?;
        fsx::write_atomic(&path, &json, 0o644)?;
        let latest = self.latest_path(&stored.record.item);
        let dir = self.dir.join("latest");
        fs::create_dir_all(&dir).map_err(io(&dir))?;
        let tmp = dir.join(format!(".{file}.{}.tmp", std::process::id()));
        let _ = fs::remove_file(&tmp);
        symlink(format!("../builds/{file}"), &tmp).map_err(io(&tmp))?;
        fsx::publish(&tmp, &latest)
    }
}

fn read(path: &Path) -> Result<Option<Stored>, BuildError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| BuildError::problem("B023", format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path)(e)),
    }
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
