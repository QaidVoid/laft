//! The evaluated, rendered description of one build.

use std::collections::BTreeMap;

use laft_core::{Blake3Hash, Name, PkgRef, Sha256Hash, Variant, Version};
use serde::{Deserialize, Serialize};

use crate::env;

/// One package and variant, fully evaluated: every default applied, every
/// overlay merged, the build style rendered into phase scripts and the
/// complete environment computed. It is everything a build needs from the
/// recipe and nothing that depends on where the recipes are checked out.
///
/// Its canonical JSON, without the operational fields, the toolchain
/// epoch and the hashes of sources, vendor tarballs and files, is the plan
/// that [`Recipe::plan_hash`] hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipe {
    /// The package name.
    pub name: Name,
    /// The upstream version.
    pub version: Version,
    /// The Laft revision, at least 1.
    pub rev: u32,
    /// The variant this build produces.
    pub variant: Variant,
    /// The recipe repositories that contributed to the package, lowest
    /// priority first. The first one defines the package.
    pub repos: Vec<Name>,
    /// The metadata of the main package.
    pub meta: Metadata,
    /// The sources, in label order.
    pub sources: Vec<Source>,
    /// The vendored dependencies, `cargo` before `go`.
    pub vendors: Vec<Vendor>,
    /// The patches, in the order they are applied.
    pub patches: Vec<FileRef>,
    /// The `-p` value for `patch`.
    pub patch_strip: u32,
    /// The other files the recipe names with path literals, such as a
    /// kernel configuration.
    pub files: Vec<FileRef>,
    /// Tools that run during the build.
    pub build_deps: Vec<PkgRef>,
    /// Libraries the build links against, adjusted to the variant: every
    /// entry of a `static` build selects the `static` variant.
    pub link_deps: Vec<PkgRef>,
    /// Packages present only when the check phase runs.
    pub check_deps: Vec<PkgRef>,
    /// The rendered phase scripts.
    pub phases: Phases,
    /// The environment of every phase, except `JOBS` and `MAKEFLAGS`, which
    /// depend on the builder. [`Recipe::environment`] adds them.
    pub env: BTreeMap<String, String>,
    /// Switches of the build and of post-processing.
    pub options: BuildOptions,
    /// How the output is split into packages and what they declare.
    pub outputs: Outputs,
    /// A script run in a view with only the main package and its runtime
    /// dependencies.
    pub smoke: Option<String>,
    /// The ABI generation of the package's libraries.
    pub abi_epoch: u32,
    /// The sum of the `toolchain_epoch` of every contributing repository,
    /// which grows whenever one of them is incremented.
    pub toolchain_epoch: u32,
    /// Fields that never influence a build.
    pub operations: Operations,
}

impl Recipe {
    /// The plan hash: BLAKE3 of the canonical JSON of the recipe without
    /// [`Recipe::operations`], without [`Recipe::toolchain_epoch`] and
    /// without the content hashes of sources, vendor tarballs, patches and
    /// files.
    ///
    /// The epoch and the content hashes are left out because the build
    /// key covers them separately. A changed hash is therefore a source
    /// change and a changed epoch a toolchain change, never a plan change,
    /// while an edit to anything else the recipe says is a plan change.
    ///
    /// # Panics
    ///
    /// Never in practice: a recipe always serializes to canonical JSON.
    pub fn plan_hash(&self) -> Blake3Hash {
        let mut value = serde_json::to_value(self).expect("a recipe serializes to JSON");
        if let Some(object) = value.as_object_mut() {
            object.remove("operations");
            object.remove("toolchain_epoch");
            for (list, key) in [
                ("vendors", "sha256"),
                ("patches", "blake3"),
                ("files", "blake3"),
            ] {
                for entry in entries(object.get_mut(list)) {
                    entry.remove(key);
                }
            }
            for source in entries(object.get_mut("sources")) {
                let fetch = source.get_mut("fetch").and_then(|f| f.as_object_mut());
                for kind in fetch.into_iter().flat_map(|f| f.values_mut()) {
                    if let Some(kind) = kind.as_object_mut() {
                        kind.remove("sha256");
                    }
                }
            }
        }
        let json = laft_core::to_canonical_json(&value)
            .expect("a recipe holds no number that canonical JSON refuses");
        Blake3Hash::of(&json)
    }

    /// The SHA-256 hashes the recipe declares, in plan order: every source
    /// that declares one, then every vendor tarball.
    pub fn source_hashes(&self) -> Vec<Sha256Hash> {
        let sources = self.sources.iter().filter_map(|s| s.sha256().copied());
        sources
            .chain(self.vendors.iter().map(|v| v.sha256))
            .collect()
    }

    /// The BLAKE3 hashes of every patch, in order, then of every other
    /// file the recipe names.
    pub fn file_hashes(&self) -> Vec<Blake3Hash> {
        let files = self.patches.iter().chain(&self.files);
        files.map(|f| f.blake3).collect()
    }

    /// The complete environment of every phase for a builder that runs
    /// `jobs` jobs in parallel: [`Recipe::env`] with `JOBS` and
    /// `MAKEFLAGS`. A recipe that does not build in parallel gets one job.
    pub fn environment(&self, jobs: u32) -> BTreeMap<String, String> {
        let jobs = if self.options.parallel {
            jobs.max(1)
        } else {
            1
        };
        let mut env = self.env.clone();
        env.insert(env::JOBS.to_owned(), jobs.to_string());
        env.insert(env::MAKEFLAGS.to_owned(), format!("-j{jobs}"));
        env
    }
}

/// The objects in the JSON array `value`.
fn entries(
    value: Option<&mut serde_json::Value>,
) -> impl Iterator<Item = &mut serde_json::Map<String, serde_json::Value>> {
    value
        .and_then(|v| v.as_array_mut())
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_object_mut())
}

/// Metadata of the main package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    /// One line of at most 80 characters, without a final period.
    pub summary: String,
    /// A longer description.
    pub description: Option<String>,
    /// The project's `https://` web site.
    pub homepage: Option<String>,
    /// Every SPDX license or exception that applies, sorted.
    pub license: Vec<String>,
    /// License files relative to `$SRC`, or `None` for every top level
    /// file whose name starts with `COPYING`, `LICENSE`, `LICENCE`,
    /// `NOTICE` or `COPYRIGHT`.
    pub license_files: Option<Vec<String>>,
}

/// A source of the package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The label, `main` for the primary source.
    pub label: String,
    /// Where the source comes from.
    pub fetch: Fetch,
    /// The directory below `$SRC` that it is placed in, `""` for `$SRC`
    /// itself. It is relative and has no `..` component.
    pub dest: String,
    /// The leading path components dropped when extracting. `None` drops
    /// one exactly when the archive holds a single directory at its top.
    pub strip_components: Option<u32>,
}

impl Source {
    /// The SHA-256 hash the recipe declares, which is always present for a
    /// URL source.
    pub fn sha256(&self) -> Option<&Sha256Hash> {
        match &self.fetch {
            Fetch::Url { sha256, .. } => Some(sha256),
            Fetch::Git { sha256, .. } => sha256.as_ref(),
        }
    }
}

/// Where a source comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fetch {
    /// A file downloaded from the first URL that works.
    Url {
        /// The `https://` locations, tried in order.
        urls: Vec<String>,
        /// The hash the file must have.
        sha256: Sha256Hash,
        /// The file name: the recipe's `file`, or the last path segment of
        /// the first URL.
        file: String,
        /// Whether the file is an archive that is extracted. Otherwise it
        /// is copied to `$SRC/<dest>/<file>`.
        extract: bool,
    },
    /// A normalized tarball of one commit of a git repository.
    Git {
        /// The `https://` clone URL.
        url: String,
        /// The full 40 character commit hash.
        commit: String,
        /// Whether submodules are included at their recorded commits.
        submodules: bool,
        /// The hash the tarball must have, when the recipe declares one.
        sha256: Option<Sha256Hash>,
    },
}

/// Which tool vendors dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VendorKind {
    /// `cargo vendor`, extracted to `/build/vendor/cargo`.
    Cargo,
    /// `go mod vendor`, extracted to `/build/vendor/go`.
    Go,
}

impl VendorKind {
    /// The name, `cargo` or `go`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Go => "go",
        }
    }
}

/// Vendored dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vendor {
    /// The vendoring tool.
    pub kind: VendorKind,
    /// The hash of the normalized vendor tarball.
    pub sha256: Sha256Hash,
    /// The directory below `$SRC` that holds `Cargo.lock` or `go.mod`.
    pub dir: String,
    /// The label of the source the lock file comes from.
    pub source: String,
}

/// A file a recipe names with a path literal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    /// The absolute path of the file inside the build sandbox, below
    /// `/build/recipes/<repo>/<name>/` or `/build/common/<repo>/`.
    pub path: String,
    /// The hash of its content when the recipe was evaluated.
    pub blake3: Blake3Hash,
}

/// The rendered phase scripts. Each is a complete `bash` script, or `None`
/// when the phase has nothing to run.
///
/// The style's script and the recipe's hooks are folded in: every script
/// starts with `set -euo pipefail`, `umask 022` and a `cd` into
/// `$SRC/<subdir>`, then runs the `pre_` hook, the phase script and the
/// `post_` hook in one shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phases {
    /// Source fixups such as `autoreconf`.
    pub prepare: Option<String>,
    /// Configuration of the build system.
    pub configure: Option<String>,
    /// The build.
    pub build: Option<String>,
    /// The test suite. It runs only when [`BuildOptions::check`] is set.
    pub check: Option<String>,
    /// Installation into `$DESTDIR`.
    pub install: Option<String>,
    /// The recipe's additions to the installed tree.
    pub post_install: Option<String>,
}

impl Phases {
    /// The phases in the order they run, with their names.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, Option<&str>)> {
        [
            ("prepare", &self.prepare),
            ("configure", &self.configure),
            ("build", &self.build),
            ("check", &self.check),
            ("install", &self.install),
            ("post_install", &self.post_install),
        ]
        .into_iter()
        .map(|(name, script)| (name, script.as_deref()))
    }
}

/// Switches of the build and of post-processing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildOptions {
    /// Run the check phase.
    pub check: bool,
    /// Build with more than one job.
    pub parallel: bool,
    /// Strip ELF files.
    pub strip: bool,
    /// Split debug info into the `-dbg` subpackage.
    pub debug_split: bool,
    /// Link static executables with `-static-pie`.
    pub static_pie: bool,
    /// Keep `usr/lib/*.a` in a `dynamic` build.
    pub keep_archives: bool,
    /// Link time optimization, already part of the flags.
    pub lto: bool,
    /// A meta package without payload.
    pub empty: bool,
    /// Empty directories kept in the payload.
    pub keep_dirs: Vec<String>,
}

/// An automatic subpackage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// `-dbg`: `usr/lib/debug/**`.
    Dbg,
    /// `-dev`: headers, pkg-config and cmake files, and linker names.
    Dev,
    /// `-doc`: documentation and info pages.
    Doc,
    /// `-locale`: translation catalogs.
    Locale,
}

impl Split {
    /// The suffix of the subpackage name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dbg => "dbg",
            Self::Dev => "dev",
            Self::Doc => "doc",
            Self::Locale => "locale",
        }
    }
}

/// How the output is split into packages and what they declare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outputs {
    /// The automatic subpackages, in the order files are assigned to them.
    /// A `static` build has at most `dbg`.
    pub split: Vec<Split>,
    /// The explicit subpackages, by suffix.
    pub subpackages: BTreeMap<String, Subpackage>,
    /// Alternative groups the package belongs to, by group name.
    pub alternatives: BTreeMap<String, Alternative>,
    /// Runtime dependencies of the main package in addition to the
    /// detected ones.
    pub deps: Vec<PkgRef>,
    /// Extra `virtual:` capabilities of the main package.
    pub provides: Vec<String>,
    /// Packages that cannot be in the same view as the main package.
    pub conflicts: Vec<PkgRef>,
    /// Package names the main package supersedes.
    pub replaces: Vec<PkgRef>,
    /// Globs of payload paths excluded from dependency detection.
    pub ignore_deps: Vec<String>,
    /// Payload paths that request a setuid or setgid bit.
    pub setuid: Vec<String>,
    /// Payload paths with their requested file capabilities.
    pub caps: BTreeMap<String, String>,
    /// Payload globs with their `user:group` owner.
    pub ownership: BTreeMap<String, String>,
}

/// An explicit subpackage, named `<name>-<suffix>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subpackage {
    /// The summary, by default the package's summary followed by the
    /// suffix in parentheses.
    pub summary: String,
    /// Globs of the payload paths it contains.
    pub files: Vec<String>,
    /// Globs removed from the match.
    pub exclude: Vec<String>,
    /// Its runtime dependencies.
    pub deps: Vec<PkgRef>,
    /// Its capabilities.
    pub provides: Vec<String>,
    /// Its conflicts.
    pub conflicts: Vec<PkgRef>,
    /// Package names it supersedes.
    pub replaces: Vec<PkgRef>,
    /// Whether it depends on the exact version of the main package.
    pub depends_on_main: bool,
    /// Its licenses, by default the package's.
    pub license: Vec<String>,
    /// Whether it is emitted even when no file matched.
    pub keep_empty: bool,
}

/// Membership in an alternative group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alternative {
    /// The payload paths the package ships for the group.
    pub paths: Vec<String>,
    /// Higher wins when neither the configuration nor the repository
    /// default chooses.
    pub priority: i64,
    /// The subpackage suffix that holds the paths, or `None` for the main
    /// package.
    pub subpackage: Option<String>,
}

/// Fields of a recipe that never influence a build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operations {
    /// The maintainers, usually `Name <email>`.
    pub maintainers: Vec<String>,
    /// How new upstream versions are found.
    pub update: Option<Update>,
    /// CPE 2.3 product prefixes for CVE matching.
    pub cpe: Vec<String>,
    /// The CI runner and time limit.
    pub ci: Ci,
}

/// How new upstream versions of a package are found. Exactly one of
/// `github`, `gitlab`, `anitya` and `url` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    /// `owner/repo` on GitHub.
    pub github: Option<String>,
    /// `host/group/project` on a GitLab instance.
    pub gitlab: Option<String>,
    /// A project id on release-monitoring.org.
    pub anitya: Option<u32>,
    /// A page whose body `pattern` is matched against.
    pub url: Option<String>,
    /// A regular expression with one capture group for the version, set
    /// exactly when `url` is.
    pub pattern: Option<String>,
    /// A prefix stripped from tags.
    pub tag_prefix: Option<String>,
    /// Only versions with this prefix are considered.
    pub track: Option<String>,
    /// Candidates containing any of these strings are dropped.
    pub ignore: Vec<String>,
    /// Whether prereleases are kept.
    pub prerelease: bool,
    /// Whether version bumps may merge automatically.
    pub automerge: bool,
    /// Why the package is never reported as outdated.
    pub disabled: Option<String>,
}

/// The CI runner and time limit of a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ci {
    /// The class of machine.
    pub runner: Runner,
    /// The longest a build may take.
    pub timeout_minutes: u32,
}

/// The class of machine a package builds on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    /// An ordinary runner.
    Standard,
    /// A runner with more cores, memory and disk.
    Big,
}
