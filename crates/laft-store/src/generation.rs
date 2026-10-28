use std::collections::BTreeMap;

use laft_core::{Blake3Hash, Name, StoreId};
use serde::{Deserialize, Serialize};

use crate::{PackageInput, PackageRecord};

/// The version of the `gen.json` format.
pub(crate) const FORMAT: u32 = 1;

/// What a generation is assembled from, apart from what the transaction
/// knows itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationInput {
    /// The resolved packages of the system view.
    pub packages: Vec<PackageInput>,
    /// The chosen member of alternatives groups in the system view, by
    /// group.
    pub alternatives: BTreeMap<Name, Name>,
    /// The declared views and the version of each that belongs to this
    /// generation. Every version must exist.
    pub views: BTreeMap<Name, u64>,
    /// The message given with the command, if any.
    pub message: Option<String>,
    /// The version of Laft that builds the generation.
    pub laft_version: String,
    /// The architecture, for example `x86_64-v3`.
    pub arch: String,
    /// The init system the generation boots with.
    pub init: Init,
    /// The evaluated configuration value, stored as `config.json`.
    pub config: Vec<u8>,
    /// Every configuration source file read during evaluation, sorted by
    /// path.
    pub config_files: Vec<ConfigFile>,
    /// The repository indexes the packages were resolved from.
    pub indexes: Vec<IndexRecord>,
}

/// The manifest of a generation, stored as `gen.json`.
///
/// It is written last during assembly, so a generation directory without a
/// valid `gen.json` is incomplete and is never booted or activated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    /// The format version, 1.
    pub format: u32,
    /// The generation number.
    pub generation: u64,
    /// The active system generation when this one was built.
    pub parent: Option<u64>,
    /// When the generation was assembled, in RFC 3339 UTC.
    pub created: String,
    /// The message given with the command, if any.
    pub message: Option<String>,
    /// The version of Laft that built it.
    pub laft_version: String,
    /// The architecture.
    pub arch: String,
    /// The init system it boots with.
    pub init: Init,
    /// The configuration it was built from.
    pub config: ConfigRecord,
    /// The repository indexes the packages were resolved from.
    pub indexes: Vec<IndexRecord>,
    /// The packages of the system view, sorted by name.
    pub packages: Vec<PackageRecord>,
    /// The packages of the initrd, which belong to no view.
    pub initrd_packages: Vec<PackageRecord>,
    /// The effective alternatives choices in the system view, by group.
    pub alternatives: BTreeMap<Name, Name>,
    /// The version of each declared view, by view name.
    pub views: BTreeMap<Name, u64>,
    /// Kernel modules of a running kernel carried into the system view.
    pub carried_modules: Vec<CarriedModule>,
    /// The paths in `/etc` the generation manages.
    pub etc: EtcRecord,
}

/// The init system of a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Init {
    /// The service backend, for example `runit`.
    pub backend: String,
    /// The command line of PID 1, for example `["/usr/bin/runit"]`.
    pub exec: Vec<String>,
}

/// The configuration a generation was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRecord {
    /// The BLAKE3 hash of `config.json`.
    pub hash: Blake3Hash,
    /// Every configuration source file read during evaluation.
    pub files: Vec<ConfigFile>,
}

/// A configuration source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// Its absolute path.
    pub path: String,
    /// The BLAKE3 hash of its content.
    pub hash: Blake3Hash,
}

/// A repository index a generation was resolved from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexRecord {
    /// The repository name.
    pub repo: String,
    /// The channel, for example `current`.
    pub channel: String,
    /// The index generation number.
    pub generation: u64,
}

/// Kernel modules carried into a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarriedModule {
    /// The kernel release the modules belong to.
    pub kernel: String,
    /// The store id of the package that ships them.
    pub id: StoreId,
}

/// The paths in `/etc` a generation manages, relative to `/etc`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtcRecord {
    /// Paths that are symlinks into the static tree, sorted.
    pub managed: Vec<String>,
    /// Paths that are copied, sorted.
    pub copy: Vec<String>,
}
