//! Reproducibility checks: rebuilding a package and comparing the result.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use laft_core::{Blake3Hash, Name, Timestamp};
use laft_pkg::{Entry, Reader};
use serde::{Deserialize, Serialize};

use crate::build::Builder;
use crate::db::{Db, Stored};
use crate::error::{BuildError, io};
use crate::{Item, Recipe, fsx};

/// One payload path that two builds produced differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DifferingFile {
    /// The package that holds it.
    pub package: Name,
    /// The payload path.
    pub path: String,
    /// What the first build had there, written like `file 1234 b3:...`,
    /// or `None` when it had nothing.
    pub first: Option<String>,
    /// What the second build had there.
    pub second: Option<String>,
}

/// The result of [`check_repro`], kept as `db/repro/<name>+<variant>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReproReport {
    /// The build key of both builds.
    pub build_key: Blake3Hash,
    /// Whether every output has the same store id in both builds.
    pub reproducible: bool,
    /// The paths that differ, by package and path.
    pub differing_files: Vec<DifferingFile>,
    /// When the check ran.
    pub checked: Timestamp,
}

/// Rebuilds `recipe` with exactly the build key inputs of its latest build
/// `previous`, whose packages are in `previous_out`, and compares the store
/// ids of every output. When they differ, the payload manifests are
/// compared and every path whose type, mode, size or content differs is
/// listed.
///
/// `builder` should use another working directory, another output
/// directory and another number of jobs than the first build, so that the
/// check exposes builds that depend on them, and should not add its
/// packages to the store.
///
/// # Errors
///
/// Fails when the rebuild fails, and when a package file cannot be read.
pub fn check_repro(
    builder: &Builder<'_>,
    recipe: &Recipe,
    previous: &Stored,
    previous_out: &Path,
) -> Result<ReproReport, BuildError> {
    let outcome = builder.build(recipe, &previous.record.inputs)?;
    let reproducible = outcome.built.outputs == previous.record.outputs;
    let differing_files = if reproducible {
        Vec::new()
    } else {
        let first: Vec<PathBuf> = previous
            .files
            .iter()
            .map(|f| previous_out.join(f))
            .collect();
        compare(&first, &outcome.packages)?
    };
    Ok(ReproReport {
        build_key: previous.record.key,
        reproducible,
        differing_files,
        checked: Timestamp::now(),
    })
}

/// The payload paths that differ between two sets of package files of the
/// same build.
///
/// # Errors
///
/// Fails when a package file cannot be read.
pub fn compare(first: &[PathBuf], second: &[PathBuf]) -> Result<Vec<DifferingFile>, BuildError> {
    let (a, b) = (entries(first)?, entries(second)?);
    let keys: std::collections::BTreeSet<&(Name, String)> = a.keys().chain(b.keys()).collect();
    Ok(keys
        .into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .map(|(package, path)| DifferingFile {
            package: package.clone(),
            path: path.clone(),
            first: a.get(&(package.clone(), path.clone())).cloned(),
            second: b.get(&(package.clone(), path.clone())).cloned(),
        })
        .collect())
}

/// Every payload path of the package files `files`, with a description of
/// its entry.
fn entries(files: &[PathBuf]) -> Result<BTreeMap<(Name, String), String>, BuildError> {
    let mut all = BTreeMap::new();
    for file in files {
        let reader = Reader::new(BufReader::new(File::open(file).map_err(io(file))?))?;
        let manifest = reader.manifest();
        let package = manifest.package().name.clone();
        for entry in manifest.entries() {
            let text = match entry {
                Entry::Dir { mode, .. } => format!("dir {mode:o}"),
                Entry::File {
                    mode, size, hash, ..
                } => format!("file {mode:o} {size} {hash}"),
                Entry::Symlink { target, .. } => format!("symlink {target}"),
            };
            all.insert((package.clone(), entry.path().to_owned()), text);
        }
    }
    Ok(all)
}

/// Keeps `report` as the latest reproducibility result of `item` in `db`.
///
/// # Errors
///
/// Fails when the report cannot be written.
pub fn record_repro(db: &Db, item: &Item, report: &ReproReport) -> Result<(), BuildError> {
    let variant = item.variant;
    let path = db
        .dir()
        .join("repro")
        .join(format!("{}+{variant}.json", item.name));
    let json = laft_core::to_canonical_json(report)
        .map_err(|e| BuildError::problem("B023", format!("{}: {e}", path.display())))?;
    fsx::write_atomic(&path, &json, 0o644)
}

#[cfg(test)]
#[path = "repro_tests.rs"]
mod tests;
