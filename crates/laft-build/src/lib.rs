//! The Laft package builder: recipe evaluation, build planning, source
//! fetching, sandboxed builds, post-processing and packaging.
//!
//! A recipe is a Stav file, `recipes/<name>/package.stav` in a recipe
//! repository, with one `package` block. This crate supplies what the
//! language needs to check recipes: the prelude with the `Package` schema
//! and the build styles, the std catalog (`std/build`), the symbol types
//! `Pkg` and `Spdx`, and the host module `laft/target`, which tells a
//! recipe which variant it is evaluated for.
//!
//! Building a set of recipes takes these steps:
//!
//! 1. [`Repos::open`] reads the recipe repositories, lowest priority
//!    first. A later repository is an overlay: its recipe for a package
//!    merges into the earlier ones through Stav merging.
//! 2. [`Repos::evaluate`] evaluates one package for each variant it
//!    builds, checks it and converts it into a [`Recipe`], with the build
//!    style rendered into phase scripts and the whole environment
//!    computed.
//! 3. [`BuildGraph::new`] resolves the dependencies between the recipes of
//!    a run, and [`KeyInputs`] gives each item its build key and, against
//!    the [`BuildRecord`] of its latest build, its rebuild [`Trigger`]s.
//! 4. A [`Schedule`] orders the dirty items and, as each build finishes,
//!    either stops at unchanged outputs or marks the dependents whose
//!    imports or static libraries changed.
//! 5. A [`SourceCache`] fetches every source into a cache addressed by
//!    SHA-256, verifying the hash before anything enters it, and
//!    [`unpack`] places a fetched source in the source tree, extracting
//!    archives with rules that keep every entry inside its destination.
//! 6. A [`Builder`] builds each item the schedule hands out: it unpacks
//!    the sources, assembles the build view from the build-side store
//!    ([`view`]), runs every phase in a sandbox with fixed paths and no
//!    network through an [`Executor`], turns the output into packages with
//!    [`post`], and feeds the store ids back into the schedule
//!    ([`Builder::run`]). [`seed()`] adds the packages of another build
//!    root, such as those of a bootstrap, to the build-side store, where
//!    they stand in until builds here make their own. [`Mode::Bootstrap`]
//!    builds without sandbox or store, for the first stages of a new
//!    system.
//!    [`Builder::install`] installs such a build straight into a system
//!    under construction instead of making packages, and
//!    [`Builder::build_into`] makes the packages and unpacks them there. A
//!    [`Stage`] runs such builds and scripts in order, on the host or
//!    inside the root entered as a chroot, or store builds and scripts in
//!    views of the store, with [`run_stage`].
//! 7. A [`Buildinfo`] records what one build used and produced, as
//!    canonical JSON written next to its packages, and the [`Db`] keeps
//!    the record that plans the next run. [`check_repro`] rebuilds and
//!    compares store ids.
//!
//! # Recipe checks
//!
//! Stav checks types, refinements, required fields and merge conflicts.
//! After evaluation, the rules below are checked, and each problem is
//! reported with the source it is about, together with the warnings of the
//! language.
//!
//! | Code | Problem |
//! |---|---|
//! | `R001` | a recipe that declares a package other than its directory name, or none |
//! | `R002` | a path literal outside the package's recipe directory and `common/`, or a path or import that leaves the recipe repositories |
//! | `R003` | a recipe directory, subpackage or alternative group whose name is not a package name |
//! | `R004` | a repository without exactly one `recipe_repo` block, listed twice, or extending one not listed before it |
//! | `R006` | a path literal naming a file that cannot be read |
//! | `R010` | a source URL that is not `https://` |
//! | `R011` | a source that sets both or neither of `url` and `git`, an empty `url` list, or `file` or `extract` on a git source |
//! | `R012` | a URL source without `sha256` |
//! | `R013` | a git source without `commit` |
//! | `R014` | a vendor block whose `source` names no source |
//! | `R015` | a `dest`, `dir`, `subdir`, file name, license file, or `setuid`, `caps` or alternative path that is not a plain relative path |
//! | `R016` | an empty package with sources or a style other than `custom`, or a package without sources that is not empty |
//! | `R017` | the `cargo` or `go` style without the matching `vendor` block |
//! | `R018` | a source label that is not a plain name of letters, digits, `-`, `_` and `.` that does not start with `.` |
//! | `R019` | an alternative whose `subpackage` is neither an explicit nor an automatic subpackage of the package |
//! | `R030` | an `env` key that does not match `[A-Z_][A-Z0-9_]*` |
//! | `R031` | an `env` key that the builder sets itself, unless it is set with `force(...)`, and always `JOBS` and `MAKEFLAGS` |
//! | `R050` | an `update` block without exactly one of `github`, `gitlab`, `anitya` and `url`, or with `pattern` and `url` not set together |
//! | `R060` | a stage file without exactly one stage, a stage name that does not start with a lowercase letter or digit followed by lowercase letters, digits and dashes, a step that sets both or neither of `packages` and `script`, a step name used twice, a search directory that is not a plain relative path, a missing recipe repository, a package that builds nothing in the variant named, a step or search directory that a stage building into the store cannot have, a view script outside such a stage or without packages, a view named by a step that does not run in one, or a stage given a root it cannot have or missing one it needs |
//!
//! Between recipes, and while planning, these problems are found:
//!
//! | Code | Problem |
//! |---|---|
//! | `R020` | a reference to a package of the run in a variant it does not build |
//! | `R021` | a reference that no recipe of the run and no published package satisfies |
//! | `R022` | more than one recipe of the run for the same package and variant, reported once with their repositories |
//! | `B010` | dirty items that depend on each other in a cycle |
//!
//! Fetching and unpacking sources reports [`SourceError`]s:
//!
//! | Code | Problem |
//! |---|---|
//! | `B020` | a location served a file whose SHA-256 hash differs from the recipe's |
//! | `B021` | an archive entry or link that leads outside its destination |
//! | `B022` | no URL and no mirror has the file |
//! | `B023` | a local file that cannot be read or written |
//! | `B024` | an archive that cannot be read, or a git source, which this version cannot fetch |
//!
//! Builds report [`BuildError`]s and [`Warning`]s:
//!
//! | Code | Problem |
//! |---|---|
//! | `B023` | a recipe file that changed since evaluation or cannot be read, a license file that is missing or outside the source tree, a build record or reproducibility report that cannot be read or serialized, or an install or stage record that is neither a build key line nor a build key record of a format this version reads |
//! | `B030` | a phase, or a stage script inside the root, that ran longer than the timeout |
//! | `B031` | a build root with less than 10 GiB free |
//! | `B032` | a build root whose lock another run holds |
//! | `B040` | a phase or tool that failed |
//! | `B041` | an installation or unpacking into a directory outside bootstrap mode |
//! | `B042` | a stage root whose top-level entries differ from Laft's layout, or a `laft-build` that is not a static ELF executable for a step inside the root |
//! | `B043` | a payload path that is a directory in a package and something else in the system it is unpacked into, or the other way round |
//! | `B050` | a build view that the build-side store cannot satisfy |
//! | `B060` | a reproducibility check of a package without a successful build |
//! | `B061` | a reproducibility check with a package that differs or could not be checked |
//! | `B1xx`, `R040`, `W1xx` | a payload that breaks a post-processing rule, see [`post`] |

#![forbid(unsafe_code)]

mod build;
mod buildinfo;
mod convert;
mod db;
mod deploy;
mod env;
mod error;
mod exec;
mod extract;
mod fsx;
mod glob;
mod graph;
mod host;
mod loader;
mod lock;
mod plan;
pub mod post;
mod recipe;
mod repo;
mod repro;
mod schema;
mod seed;
mod source;
mod stage;
mod style;
mod target;
mod vendor;
pub mod view;

#[cfg(test)]
mod test_support;

use std::fmt;

pub use build::{BuildRoot, Builder, Event, Mode, Outcome, Session, installed};
pub use buildinfo::{
    BUILDINFO_FORMAT, Buildinfo, BuildinfoError, BuildinfoHost, BuiltPackage, Output, RepoState,
    SourceHash, VendorHash, ViewPackage,
};
pub use db::{Db, Stored};
pub use env::SOURCE_DATE_EPOCH;
pub use error::{BuildError, Warning};
pub use exec::{DEFAULT_TIMEOUT, Executor};
pub use extract::unpack;
pub use graph::{BuildGraph, Deps, Item};
pub use lock::{LOCK_ENV, RootLock};
pub use plan::{
    BuildRecord, Built, Cutoff, Dirtied, KeyInputs, LinkInput, Schedule, StaticInput, Trigger,
};
pub use recipe::{
    Alternative, BuildOptions, Ci, Fetch, FileRef, Metadata, Operations, Outputs, Phases, Recipe,
    Runner, Shell, Source, Split, Subpackage, Update, Vendor, VendorKind,
};
pub use repo::{Evaluation, RecipeRepo, Repos};
pub use repro::{
    DifferingFile, ReproReport, check_repro, compare, differing_packages, record_repro,
};
pub use seed::seed;
pub use source::{SourceCache, SourceError};
pub use stage::{
    Install, Place, Stage, StageEvent, StageOptions, StageState, Step, Work, run_stage,
};
pub use target::{ARCH, CPU, GNU_TRIPLE, MUSL_TRIPLE, SYSROOT, Target};
pub use vendor::normalized_tarball;

/// Diagnostics about Stav files, with the names and texts of the files
/// they point at, ready for a front end to render.
pub type Report = stav::Diagnostics;

/// A problem that is not about one place in a file, such as a dependency
/// between recipes, with its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The stable code, for example `R020`.
    pub code: &'static str,
    /// What is wrong, starting lowercase and without a final period.
    pub message: String,
}

impl Problem {
    /// A problem with `code` and `message`.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error[{}]: {}", self.code, self.message)
    }
}

impl std::error::Error for Problem {}

/// Why recipes could not be opened or evaluated.
#[derive(Debug, Clone)]
pub enum Error {
    /// Problems in Stav files: syntax, types, merging, refinements and the
    /// recipe checks, each pointing at its source.
    Report(Report),
    /// Problems that are not about one place in a file.
    Problems(Vec<Problem>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Report(report) => write!(f, "{report}"),
            Self::Problems(problems) => problems.iter().try_for_each(|p| writeln!(f, "{p}")),
        }
    }
}

impl std::error::Error for Error {}
