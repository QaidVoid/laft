//! `laft-repo promote`: copies a vetted channel into another one.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use laft_core::StoreId;
use laft_index::Index;

use crate::check::{self, Report};
use crate::error::{Error, Result, io};
use crate::generation::{self, Options};
use crate::index::{Outcome, empty};
use crate::layout::{Channel, Repository};
use crate::sign::Signer;

/// The conditions the source generation must meet.
#[derive(Debug, Default)]
pub(crate) struct Gates<'a> {
    /// The least time, in seconds, since the source generation was signed.
    pub(crate) min_soak: Option<u64>,
    /// The JSON report of `laft-repo check --json` for the source
    /// generation, which must have no errors.
    pub(crate) check_report: Option<&'a Path>,
}

/// Replaces the packages and defaults of `to` with those of the current
/// generation of `from`, once every gate holds, checks the result and
/// writes it as a new generation of `to`.
///
/// Yanked packages of both channels stay yanked unless the new package
/// list contains them again. The pool is shared, so no package file is
/// copied.
pub(crate) fn run(
    repo: &Repository,
    from: &Channel,
    to: &Channel,
    gates: &Gates<'_>,
    bases: &[Index],
    signer: Option<&Signer>,
    options: &Options,
) -> Result<(u64, Outcome)> {
    let source = repo
        .read_index(from)?
        .ok_or_else(|| Error::new(format!("channel {from} has no index")))?;
    if let Some(min_soak) = gates.min_soak {
        let age = options
            .now
            .as_secs()
            .saturating_sub(source.timestamp.as_secs());
        if age < min_soak {
            return Err(Error::new(format!(
                "gate failed: {from} generation {} is {age} seconds old, promotion needs {min_soak}",
                source.generation
            )));
        }
    }
    if let Some(path) = gates.check_report {
        check_gate(path, &source)?;
    }
    let previous = repo.read_index(to)?;
    let mut index = previous
        .clone()
        .unwrap_or_else(|| empty(source.repo.clone(), to));
    if index.repo != source.repo {
        return Err(Error::new(format!(
            "channel {to} belongs to repository {}, but {from} belongs to {}",
            index.repo, source.repo
        )));
    }
    let available: HashSet<StoreId> = source.packages.iter().map(|e| e.id).collect();
    let mut yanked = BTreeMap::new();
    for yank in index.yanked.iter().chain(&source.yanked) {
        if !available.contains(&yank.id) {
            yanked.entry(yank.id).or_insert_with(|| yank.clone());
        }
    }
    index.packages = source.packages.clone();
    index.defaults = source.defaults.clone();
    index.yanked = yanked.into_values().collect();
    let index = generation::prepare(repo, to, index, options)?;
    let others = repo.other_indexes(to)?;
    let report = check::check(&check::Input {
        repo,
        index: &index,
        staged: &BTreeMap::new(),
        staged_buildinfo: &BTreeMap::new(),
        published: previous.as_ref(),
        others: &others,
        bases,
        allow_bootstrap: false,
    });
    if report.errors > 0 {
        return Ok((source.generation, Outcome::Refused(report)));
    }
    generation::write(repo, to, &index, signer, options)?;
    Ok((
        source.generation,
        Outcome::Written {
            index: Box::new(index),
            added: 0,
            report,
        },
    ))
}

/// Accepts the check report at `path` only when it is a report of exactly
/// the source generation and has no errors.
fn check_gate(path: &Path, source: &Index) -> Result<()> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    let report: Report = serde_json::from_slice(&bytes)
        .map_err(|e| Error::new(format!("{}: not a check report: {e}", path.display())))?;
    let expected = (
        &source.repo,
        &source.channel,
        source.arch,
        source.generation,
    );
    let actual = (
        &report.repo,
        &report.channel,
        report.arch,
        report.generation,
    );
    if actual != expected {
        return Err(Error::new(format!(
            "gate failed: {} reports on {} {}/{} generation {}, not on generation {} of {}/{}",
            path.display(),
            report.repo,
            report.channel,
            report.arch,
            report.generation,
            source.generation,
            source.channel,
            source.arch
        )));
    }
    if report.errors > 0 {
        return Err(Error::new(format!(
            "gate failed: {} reports {} errors",
            path.display(),
            report.errors
        )));
    }
    Ok(())
}
