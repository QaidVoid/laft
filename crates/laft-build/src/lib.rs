//! The Laft package builder: recipe evaluation, build planning and source
//! fetching.
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
//!
//! # Recipe checks
//!
//! Stav checks types, refinements, required fields and merge conflicts.
//! A path literal that leaves its recipe repository is the Stav error
//! `E0702`. After evaluation, the rules below are checked, and each
//! problem is reported with the source it is about.
//!
//! | Code | Problem |
//! |---|---|
//! | `R001` | a recipe that declares a package other than its directory name, or none |
//! | `R002` | a path literal outside the package's recipe directory and `common/` |
//! | `R003` | a recipe directory, subpackage or alternative group whose name is not a package name |
//! | `R004` | a repository without exactly one `recipe_repo` block, listed twice, or extending one not listed before it |
//! | `R006` | a path literal naming a file that cannot be read |
//! | `R010` | a source URL that is not `https://` |
//! | `R011` | a source that sets both or neither of `url` and `git`, an empty `url` list, or `file` or `extract` on a git source |
//! | `R012` | a URL source without `sha256` |
//! | `R013` | a git source without `commit` |
//! | `R014` | a vendor block whose `source` names no source |
//! | `R015` | a `dest`, `dir`, `subdir` or file name that is not a plain relative path |
//! | `R016` | an empty package with sources or a style other than `custom`, or a package without sources that is not empty |
//! | `R017` | the `cargo` or `go` style without the matching `vendor` block |
//! | `R030` | an `env` key that does not match `[A-Z_][A-Z0-9_]*` |
//! | `R031` | an `env` key that the builder sets itself |
//! | `R050` | an `update` block without exactly one of `github`, `gitlab`, `anitya` and `url`, or with `pattern` and `url` not set together |
//!
//! Between recipes, and while planning, these problems are found:
//!
//! | Code | Problem |
//! |---|---|
//! | `R020` | a reference to a package of the run in a variant it does not build |
//! | `R021` | a reference that no recipe of the run and no published package satisfies |
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

#![forbid(unsafe_code)]

mod convert;
mod env;
mod extract;
mod graph;
mod host;
mod loader;
mod plan;
mod recipe;
mod repo;
mod schema;
mod source;
mod style;
mod target;

#[cfg(test)]
mod test_support;

use std::fmt;

pub use env::SOURCE_DATE_EPOCH;
pub use extract::unpack;
pub use graph::{BuildGraph, Deps, Item};
pub use plan::{
    BuildRecord, Built, Cutoff, Dirtied, KeyInputs, LinkInput, Schedule, StaticInput, Trigger,
};
pub use recipe::{
    Alternative, BuildOptions, Ci, Fetch, FileRef, Metadata, Operations, Outputs, Phases, Recipe,
    Runner, Source, Split, Subpackage, Update, Vendor, VendorKind,
};
pub use repo::{Evaluation, RecipeRepo, Repos};
pub use source::{SourceCache, SourceError};
pub use target::{ARCH, CPU, GNU_TRIPLE, MUSL_TRIPLE, SYSROOT, Target};

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
