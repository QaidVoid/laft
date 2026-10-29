//! The file layout of a repository directory.
//!
//! ```text
//! <root>/
//! ├── keys.json, keys.json.minisig
//! ├── pool/<id>.lpkg
//! ├── buildinfo/<id>.json
//! └── channels/<channel>/<arch>/
//!     ├── index.json.zst, index.json.zst.minisig
//!     └── snapshots/<generation>.json.zst, <generation>.json.zst.minisig
//! ```

use std::path::{Path, PathBuf};

use laft_core::{Name, StoreId};
use laft_index::Index;
use laft_pkg::Arch;

use crate::error::{Error, Result};
use crate::fsx;

/// The name of a channel's entry point.
pub(crate) const INDEX: &str = "index.json.zst";
/// The suffix of a signature file.
pub(crate) const SIG: &str = ".minisig";
/// The name of the online key list.
pub(crate) const KEYS: &str = "keys.json";

/// A repository directory.
#[derive(Debug, Clone)]
pub(crate) struct Repository {
    root: PathBuf,
}

/// One channel and architecture of a repository.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Channel {
    /// The channel name, for example `current`.
    pub(crate) name: Name,
    /// The architecture.
    pub(crate) arch: Arch,
}

impl Channel {
    /// The directory of the channel relative to the repository root.
    pub(crate) fn dir(&self) -> String {
        format!("channels/{}/{}", self.name, self.arch)
    }

    /// The entry point relative to the repository root.
    pub(crate) fn index(&self) -> String {
        format!("{}/{INDEX}", self.dir())
    }

    /// The snapshot of `generation` relative to the repository root.
    pub(crate) fn snapshot(&self, generation: u64) -> String {
        format!("{}/snapshots/{generation}.json.zst", self.dir())
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.name, self.arch)
    }
}

impl Repository {
    /// The repository rooted at `root`, which need not exist yet.
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The path of `relative` below the root.
    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The package file of `id`.
    pub(crate) fn pool(&self, id: StoreId) -> PathBuf {
        self.path(&pool_file(id))
    }

    /// The buildinfo of `id`.
    pub(crate) fn buildinfo(&self, id: StoreId) -> PathBuf {
        self.path(&format!("buildinfo/{id}.json"))
    }

    /// Reads and validates the entry point of `channel`, or returns `None`
    /// when the channel has none yet.
    ///
    /// The signature is not checked. The repository directory is the
    /// publisher's own working tree.
    pub(crate) fn read_index(&self, channel: &Channel) -> Result<Option<Index>> {
        let path = self.path(&channel.index());
        let Some(bytes) = fsx::read_optional(&path)? else {
            return Ok(None);
        };
        Index::decode(&bytes)
            .map(Some)
            .map_err(|e| Error::new(format!("{}: {e}", path.display())))
    }

    /// The generations of the snapshots of `channel`, ascending.
    pub(crate) fn snapshots(&self, channel: &Channel) -> Result<Vec<u64>> {
        let dir = self.path(&format!("{}/snapshots", channel.dir()));
        let mut generations: Vec<u64> = fsx::list(&dir)?
            .iter()
            .filter_map(|name| name.strip_suffix(".json.zst"))
            .filter_map(|n| n.parse().ok().filter(|g: &u64| g.to_string() == n))
            .collect();
        generations.sort_unstable();
        Ok(generations)
    }

    /// Every channel and architecture that has an entry point, sorted.
    pub(crate) fn channels(&self) -> Result<Vec<Channel>> {
        let mut channels = Vec::new();
        for name in fsx::list(&self.path("channels"))? {
            let Ok(name) = name.parse::<Name>() else {
                continue;
            };
            for arch in fsx::list(&self.path(&format!("channels/{name}")))? {
                let Ok(arch) = arch.parse::<Arch>() else {
                    continue;
                };
                let channel = Channel {
                    name: name.clone(),
                    arch,
                };
                if self.path(&channel.index()).is_file() {
                    channels.push(channel);
                }
            }
        }
        Ok(channels)
    }

    /// The indexes of every other channel of the same architecture as
    /// `channel`.
    pub(crate) fn other_indexes(&self, channel: &Channel) -> Result<Vec<Index>> {
        let mut indexes = Vec::new();
        for other in self.channels()? {
            if other.arch == channel.arch && other.name != channel.name {
                indexes.extend(self.read_index(&other)?);
            }
        }
        Ok(indexes)
    }

    /// The root directory.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// The path of the package file of `id` relative to the repository root,
/// which is also the `file` of its index entry.
pub(crate) fn pool_file(id: StoreId) -> String {
    format!("pool/{id}.lpkg")
}

/// The signature file of the file `relative`.
pub(crate) fn sig(relative: &str) -> String {
    format!("{relative}{SIG}")
}
