use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use laft_core::Name;

use crate::error::{Result, corrupt, io};
use crate::{Generation, Store, ViewManifest, fsx};

/// The root of a Laft system, under which every path this crate touches
/// lies.
///
/// The root is `/` on a running system. An installer points it at the
/// target's mount point, and tests at a temporary directory. The store,
/// generations, views and state live in `<root>/laft`, and the runtime
/// symlinks that name the active and booted generations in
/// `<root>/run/laft`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    path: PathBuf,
}

impl Default for Root {
    /// The root of the running system, `/`.
    fn default() -> Self {
        Self::new("/")
    }
}

impl Root {
    /// The root at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The package store.
    pub fn store(&self) -> Store {
        Store::new(self.laft().join("store"))
    }

    /// The numbers of the generation directories, sorted. A directory is
    /// listed whether or not it holds a valid `gen.json`.
    ///
    /// # Errors
    ///
    /// Fails when the generations directory cannot be read.
    pub fn generations(&self) -> Result<Vec<u64>> {
        fsx::numbered(&self.gens_dir())
    }

    /// Reads the manifest of generation `number`.
    ///
    /// # Errors
    ///
    /// Fails when the generation has no readable, valid `gen.json`, which
    /// means it is incomplete and must never be booted or activated.
    pub fn generation(&self, number: u64) -> Result<Generation> {
        let path = self.gen_dir(number).join("gen.json");
        let generation: Generation = read_json(&path)?;
        if generation.format != crate::generation::FORMAT || generation.generation != number {
            return Err(corrupt(&path, "unsupported format or wrong generation"));
        }
        Ok(generation)
    }

    /// The generation that `laft/gens/default` points at, the one the
    /// machine boots by default.
    ///
    /// # Errors
    ///
    /// Fails when the link exists but cannot be read as a generation number.
    pub fn default_generation(&self) -> Result<Option<u64>> {
        let link = self.gens_dir().join("default");
        match fs::symlink_metadata(&link) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(&link)(e)),
            Ok(_) => fsx::link_number(&link)
                .map(Some)
                .ok_or_else(|| corrupt(&link, "the link does not name a generation")),
        }
    }

    /// The versions of view `name`, sorted.
    ///
    /// # Errors
    ///
    /// Fails when the view's directory exists but cannot be read.
    pub fn view_versions(&self, name: &Name) -> Result<Vec<u64>> {
        fsx::numbered(&self.views_dir().join(name.as_str()))
    }

    /// Reads the manifest of version `version` of view `name`.
    ///
    /// # Errors
    ///
    /// Fails when the version has no readable, valid `view.json`.
    pub fn view(&self, name: &Name, version: u64) -> Result<ViewManifest> {
        let path = self.view_dir(name, version).join("view.json");
        let view: ViewManifest = read_json(&path)?;
        if view.version != crate::view::FORMAT || view.view_gen != version || &view.name != name {
            return Err(corrupt(&path, "unsupported format or wrong view"));
        }
        Ok(view)
    }

    pub(crate) fn laft(&self) -> PathBuf {
        self.path.join("laft")
    }

    pub(crate) fn gens_dir(&self) -> PathBuf {
        self.laft().join("gens")
    }

    pub(crate) fn gen_dir(&self, number: u64) -> PathBuf {
        self.gens_dir().join(number.to_string())
    }

    pub(crate) fn views_dir(&self) -> PathBuf {
        self.laft().join("views")
    }

    pub(crate) fn view_dir(&self, name: &Name, version: u64) -> PathBuf {
        self.views_dir()
            .join(name.as_str())
            .join(version.to_string())
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.laft().join("state")
    }

    pub(crate) fn journal_dir(&self) -> PathBuf {
        self.state_dir().join("journal")
    }

    pub(crate) fn pins_dir(&self) -> PathBuf {
        self.state_dir().join("pins")
    }

    pub(crate) fn tmp_dir(&self) -> PathBuf {
        self.laft().join("tmp")
    }

    /// Creates the directories of `laft/` that are missing.
    pub(crate) fn init(&self) -> Result<()> {
        let laft = self.laft();
        for (dir, mode) in [
            (laft.clone(), 0o755),
            (laft.join("store"), 0o755),
            (self.gens_dir(), 0o755),
            (self.views_dir(), 0o755),
            (self.state_dir(), 0o700),
            (self.journal_dir(), 0o700),
            (self.pins_dir(), 0o700),
            (self.tmp_dir(), 0o700),
        ] {
            fsx::ensure_dir(&dir, mode)?;
        }
        Ok(())
    }
}

/// Reads a JSON document that Laft wrote.
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(io(path))?;
    serde_json::from_slice(&bytes).map_err(|e| corrupt(path, e))
}

/// Serializes a record as canonical JSON.
pub(crate) fn to_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    laft_core::to_canonical_json(value)
        .map_err(|e| crate::Error::Invalid(format!("cannot record a value: {e}")))
}
