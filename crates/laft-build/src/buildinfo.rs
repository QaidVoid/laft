//! The record of one build, written next to its packages.

use std::collections::BTreeMap;
use std::fmt;

use laft_core::{Blake3Hash, Name, Sha256Hash, Soname, StoreId, Timestamp, Variant, Version};
use serde::{Deserialize, Serialize};

use crate::{FileRef, Recipe, VendorKind};

/// The version of the buildinfo format that [`Buildinfo`] reads and
/// writes.
pub const BUILDINFO_FORMAT: u32 = 1;

/// What one build used and produced. One build writes one buildinfo,
/// whatever number of packages it outputs.
///
/// Everything outside [`Buildinfo::host`] is the same for two builds with
/// the same build key and the same build view, so two buildinfo files can
/// be compared with [`Buildinfo::same_build`]. A buildinfo never influences
/// a store id.
///
/// ```
/// use laft_build::{Buildinfo, BuildinfoHost, BuiltPackage};
///
/// let info = Buildinfo {
///     format: laft_build::BUILDINFO_FORMAT,
///     package: BuiltPackage {
///         name: "zlib".parse()?,
///         version: "1.3.1".parse()?,
///         rev: 1,
///         variant: laft_core::Variant::Static,
///     },
///     build_key: laft_core::Blake3Hash::of(b"key"),
///     plan_hash: laft_core::Blake3Hash::of(b"plan"),
///     recipe_repos: vec![],
///     sources: vec![],
///     vendors: vec![],
///     patches: vec![],
///     toolchain_epoch: 1,
///     abi_epoch: 0,
///     build_view: vec![],
///     env: Default::default(),
///     outputs: vec![],
///     abi: Default::default(),
///     host: BuildinfoHost {
///         laft_build: "0.1.0".into(),
///         kernel: "6.17.5".into(),
///         jobs: 16,
///         started: "2026-10-28T10:00:00Z".parse()?,
///         finished: "2026-10-28T10:03:12Z".parse()?,
///     },
///     bootstrap: false,
/// };
/// assert_eq!(info.file_name(), "zlib-1.3.1-r1+static.buildinfo.json");
/// assert_eq!(Buildinfo::from_json(&info.to_json()).ok(), Some(info));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Buildinfo {
    /// [`BUILDINFO_FORMAT`].
    pub format: u32,
    /// What was built.
    pub package: BuiltPackage,
    /// The build key.
    pub build_key: Blake3Hash,
    /// The plan hash of the recipe.
    pub plan_hash: Blake3Hash,
    /// The recipe repositories that contributed, lowest priority first.
    pub recipe_repos: Vec<RepoState>,
    /// The hash of every source, in plan order.
    pub sources: Vec<SourceHash>,
    /// The hash of every vendor tarball.
    pub vendors: Vec<VendorHash>,
    /// The patches, in the order they were applied.
    pub patches: Vec<FileRef>,
    /// The toolchain epoch of the recipe repositories.
    pub toolchain_epoch: u32,
    /// The recipe's `abi_epoch`, which dependents read when they compute
    /// their build keys.
    pub abi_epoch: u32,
    /// Every package of the build view.
    pub build_view: Vec<ViewPackage>,
    /// The environment the phases ran with.
    pub env: BTreeMap<String, String>,
    /// The packages the build produced.
    pub outputs: Vec<Output>,
    /// The ABI digest of every shared library the build produced, by
    /// soname.
    pub abi: BTreeMap<Soname, Blake3Hash>,
    /// Where and when the build ran. It differs between rebuilds.
    pub host: BuildinfoHost,
    /// Whether the build ran in bootstrap mode, whose packages are never
    /// published.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bootstrap: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The package and variant a build produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltPackage {
    /// The package name.
    pub name: Name,
    /// The upstream version.
    pub version: Version,
    /// The Laft revision.
    pub rev: u32,
    /// The variant.
    pub variant: Variant,
}

/// The state of a recipe repository when a build ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoState {
    /// The repository name.
    pub name: Name,
    /// The commit the repository was at.
    pub commit: String,
    /// Whether the repository had uncommitted changes. Packages of such a
    /// build are never published.
    pub dirty: bool,
}

/// The hash of one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceHash {
    /// The source label.
    pub label: String,
    /// The SHA-256 hash of the file or tarball.
    pub sha256: Sha256Hash,
}

/// The hash of one vendor tarball.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorHash {
    /// The vendoring tool.
    pub label: VendorKind,
    /// The SHA-256 hash of the normalized tarball.
    pub sha256: Sha256Hash,
}

/// A package of the build view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewPackage {
    /// The package name.
    pub name: Name,
    /// The upstream version.
    pub version: Version,
    /// The Laft revision.
    pub rev: u32,
    /// The variant.
    pub variant: Variant,
    /// The store id.
    pub id: StoreId,
}

/// A package the build produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// The package name, of the main package or a subpackage.
    pub name: Name,
    /// The variant.
    pub variant: Variant,
    /// The store id.
    pub id: StoreId,
    /// The name of the package file.
    pub file: String,
}

/// Where and when a build ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildinfoHost {
    /// The version of `laft-build`.
    pub laft_build: String,
    /// The release of the kernel the build ran on.
    pub kernel: String,
    /// The number of parallel jobs.
    pub jobs: u32,
    /// When the build started.
    pub started: Timestamp,
    /// When the build finished.
    pub finished: Timestamp,
}

/// Why a buildinfo could not be read.
#[derive(Debug)]
pub enum BuildinfoError {
    /// The document is not a buildinfo of any known format.
    Json(serde_json::Error),
    /// The document has a format this version does not read.
    Format(u32),
}

impl fmt::Display for BuildinfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "invalid buildinfo: {e}"),
            Self::Format(n) => write!(
                f,
                "buildinfo format {n} is not supported, only format {BUILDINFO_FORMAT}"
            ),
        }
    }
}

impl std::error::Error for BuildinfoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            Self::Format(_) => None,
        }
    }
}

impl Buildinfo {
    /// The parts of a buildinfo that come from the recipe alone, with
    /// everything the build itself decides left empty: no repository
    /// states, build view, environment, outputs or ABI digests, and a host
    /// to fill in. Sources are listed as [`Buildinfo::sources`] describes.
    pub fn for_recipe(recipe: &Recipe, build_key: Blake3Hash, host: BuildinfoHost) -> Self {
        Self {
            format: BUILDINFO_FORMAT,
            package: BuiltPackage {
                name: recipe.name.clone(),
                version: recipe.version.clone(),
                rev: recipe.rev,
                variant: recipe.variant,
            },
            build_key,
            plan_hash: recipe.plan_hash(),
            recipe_repos: Vec::new(),
            sources: recipe
                .sources
                .iter()
                .map(|s| SourceHash {
                    label: s.label.clone(),
                    sha256: *s.sha256(),
                })
                .collect(),
            vendors: recipe
                .vendors
                .iter()
                .map(|v| VendorHash {
                    label: v.kind,
                    sha256: v.sha256,
                })
                .collect(),
            patches: recipe.patches.clone(),
            toolchain_epoch: recipe.toolchain_epoch,
            abi_epoch: recipe.abi_epoch,
            build_view: Vec::new(),
            env: BTreeMap::new(),
            outputs: Vec::new(),
            abi: BTreeMap::new(),
            host,
            bootstrap: false,
        }
    }

    /// The buildinfo as canonical JSON.
    ///
    /// # Panics
    ///
    /// Never in practice: a buildinfo holds no number that canonical JSON
    /// refuses.
    pub fn to_json(&self) -> Vec<u8> {
        laft_core::to_canonical_json(self).expect("a buildinfo serializes to canonical JSON")
    }

    /// Reads a buildinfo.
    ///
    /// # Errors
    ///
    /// Fails with [`BuildinfoError::Format`] for a format other than
    /// [`BUILDINFO_FORMAT`], and with [`BuildinfoError::Json`] for anything
    /// that is not a buildinfo.
    pub fn from_json(bytes: &[u8]) -> Result<Self, BuildinfoError> {
        #[derive(Deserialize)]
        struct Format {
            format: u32,
        }
        let format: Format = serde_json::from_slice(bytes).map_err(BuildinfoError::Json)?;
        if format.format != BUILDINFO_FORMAT {
            return Err(BuildinfoError::Format(format.format));
        }
        serde_json::from_slice(bytes).map_err(BuildinfoError::Json)
    }

    /// The file name, `<name>-<version>-r<rev>.buildinfo.json`, with
    /// `+<variant>` after the revision for variants other than `dynamic`.
    pub fn file_name(&self) -> String {
        let p = &self.package;
        match p.variant {
            Variant::Dynamic => format!("{}-{}-r{}.buildinfo.json", p.name, p.version, p.rev),
            variant => format!(
                "{}-{}-r{}+{variant}.buildinfo.json",
                p.name, p.version, p.rev
            ),
        }
    }

    /// Whether two buildinfos record the same build: everything but
    /// [`Buildinfo::host`] is equal.
    pub fn same_build(&self, other: &Self) -> bool {
        Self {
            host: other.host.clone(),
            ..self.clone()
        } == *other
    }
}

#[cfg(test)]
#[path = "buildinfo_tests.rs"]
mod tests;
