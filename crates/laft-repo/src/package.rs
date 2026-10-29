//! Package files and buildinfo as `laft-repo` reads them.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use laft_core::{Blake3Hash, StoreId};
use laft_index::IndexEntry;
use laft_pkg::{Manifest, Meta, Reader};
use serde::Deserialize;

use crate::error::{Error, Result, io};
use crate::fsx;
use crate::layout::pool_file;

/// A package file with its metadata and manifest, read without unpacking
/// the payload.
#[derive(Debug)]
pub(crate) struct Package {
    /// Where the file is.
    pub(crate) path: PathBuf,
    /// The metadata member.
    pub(crate) meta: Meta,
    /// The payload manifest.
    pub(crate) manifest: Manifest,
    /// The size of the file in bytes.
    pub(crate) size: u64,
    /// The BLAKE3 hash of the file.
    pub(crate) blake3: Blake3Hash,
}

impl Package {
    /// Reads the package file `path`. Its identity comes from the
    /// metadata and manifest inside, never from the file name.
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let (size, blake3) = fsx::hash_file(path)?;
        let file = File::open(path).map_err(io(path))?;
        let reader = Reader::new(BufReader::new(file))
            .map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        Ok(Self {
            path: path.to_path_buf(),
            meta: reader.meta().clone(),
            manifest: reader.manifest().clone(),
            size,
            blake3,
        })
    }

    /// The store id, computed from the manifest.
    pub(crate) fn id(&self) -> StoreId {
        self.manifest.store_id()
    }

    /// The index entry of the package.
    pub(crate) fn entry(&self) -> IndexEntry {
        let id = self.id();
        IndexEntry {
            meta: self.meta.clone(),
            id,
            file: pool_file(id),
            size: self.size,
            blake3: self.blake3,
        }
    }
}

/// The fields of a buildinfo file that publishing depends on. The file
/// itself is copied into the repository byte for byte.
#[derive(Debug, Deserialize)]
pub(crate) struct Buildinfo {
    /// The buildinfo format version, which must be [`Buildinfo::FORMAT`].
    pub(crate) format: u32,
    /// The packages the build produced.
    pub(crate) outputs: Vec<Output>,
    /// The recipe repositories the build used.
    pub(crate) recipe_repos: Vec<RecipeRepo>,
    /// Whether the package came from a bootstrap build, which is never
    /// published to a normal channel. Only bootstrap builds record it.
    #[serde(default)]
    pub(crate) bootstrap: bool,
}

/// One package a build produced.
#[derive(Debug, Deserialize)]
pub(crate) struct Output {
    /// Its store id.
    pub(crate) id: StoreId,
}

/// A recipe repository a build used.
#[derive(Debug, Deserialize)]
pub(crate) struct RecipeRepo {
    /// Whether it had uncommitted changes.
    pub(crate) dirty: bool,
}

impl Buildinfo {
    /// The buildinfo format version this tool reads.
    pub(crate) const FORMAT: u32 = 1;

    /// Parses the buildinfo file `path`, which must have format
    /// [`Self::FORMAT`].
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(io(path))?;
        let info: Self = serde_json::from_slice(&bytes)
            .map_err(|e| Error::new(format!("{}: not a buildinfo file: {e}", path.display())))?;
        if info.format != Self::FORMAT {
            return Err(Error::new(format!(
                "{}: buildinfo format {} is not supported, only {}",
                path.display(),
                info.format,
                Self::FORMAT
            )));
        }
        Ok(info)
    }

    /// Whether the build used a recipe repository with uncommitted changes.
    pub(crate) fn dirty(&self) -> bool {
        self.recipe_repos.iter().any(|r| r.dirty)
    }
}

/// The package files and buildinfo of a set of build outputs.
#[derive(Debug, Default)]
pub(crate) struct Inputs {
    /// The packages, by store id.
    pub(crate) packages: BTreeMap<StoreId, Package>,
    /// The buildinfo file of each output id.
    pub(crate) buildinfo: BTreeMap<StoreId, PathBuf>,
}

impl Inputs {
    /// Reads `paths`: package files, and directories of build outputs whose
    /// `*.lpkg` and `*.buildinfo.json` files are read.
    pub(crate) fn read(paths: &[PathBuf]) -> Result<Self> {
        let mut inputs = Self::default();
        for path in paths {
            if path.is_dir() {
                for name in fsx::list(path)? {
                    let file = path.join(&name);
                    if name.ends_with(".lpkg") {
                        inputs.add_package(&file)?;
                    } else if name.ends_with(".buildinfo.json") {
                        for output in Buildinfo::read(&file)?.outputs {
                            inputs.buildinfo.insert(output.id, file.clone());
                        }
                    }
                }
            } else {
                inputs.add_package(path)?;
            }
        }
        Ok(inputs)
    }

    fn add_package(&mut self, path: &Path) -> Result<()> {
        let package = Package::read(path)?;
        self.packages.insert(package.id(), package);
        Ok(())
    }
}
