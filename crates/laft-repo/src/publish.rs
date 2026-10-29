//! `laft-repo publish`: copies a repository directory to a static target
//! in an order that keeps the target consistent for readers.
//!
//! Publishing first computes a plan, a list of [`Step`]s, from the
//! repository and the target's current state, then applies the steps in
//! order and stops at the first failure:
//!
//! 1. Package files and buildinfo the target does not have yet.
//! 2. A check that the target now holds every package file the new
//!    indexes reference, with the right size.
//! 3. `keys.json`, before its signature, when it changed.
//! 4. The snapshots of new generations, each before its signature.
//! 5. Each changed entry point, then its signature.
//! 6. Snapshots the repository no longer keeps.
//!
//! Every file is written to a temporary name and renamed into place, so a
//! reader sees either the old file or the new one. An index therefore never
//! references a package file the target lacks, whenever publishing stops.
//! Files below `pool/`, `buildinfo/` and the snapshots never change once
//! published, and a target copy that differs from the repository's stops
//! publishing before anything is copied.

use std::collections::BTreeSet;
use std::path::Path;

use laft_core::Name;
use laft_index::{Index, IndexComment, Keys, KeysComment, Signature};

use crate::error::{Error, Result, io};
use crate::fsx;
use crate::layout::{Channel, KEYS, Repository, SIG, sig};

/// One operation on the target, with paths relative to the repository
/// root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Copies a file from the repository to the same path in the target.
    Copy(String),
    /// Fails unless the target holds each file with the given size.
    Verify(Vec<(String, u64)>),
    /// Removes a file from the target.
    Remove(String),
}

/// The steps of a publication and the channels it publishes.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// The steps, in the order they must run.
    pub(crate) steps: Vec<Step>,
    /// The published channels and their generations.
    pub(crate) channels: Vec<(Channel, u64)>,
}

/// What publishing did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    /// Files copied.
    pub(crate) copied: usize,
    /// Files removed.
    pub(crate) removed: usize,
    /// The published channels and generations.
    pub(crate) channels: Vec<(Channel, u64)>,
}

/// Publishes the channels of `repo`, or only those named `only`, to the
/// directory `target`. Every published channel must be signed, so a
/// single unsigned channel fails the whole publication.
pub(crate) fn run(repo: &Repository, target: &Path, only: Option<&Name>) -> Result<Summary> {
    let plan = plan(repo, target, only)?;
    let mut summary = Summary {
        channels: plan.channels,
        ..Summary::default()
    };
    for step in &plan.steps {
        apply(repo, target, step)?;
        match step {
            Step::Copy(_) => summary.copied += 1,
            Step::Remove(_) => summary.removed += 1,
            Step::Verify(_) => {}
        }
    }
    Ok(summary)
}

/// Computes the steps that bring `target` up to date with `repo`, in the
/// order they must run, and the channels they publish.
pub(crate) fn plan(repo: &Repository, target: &Path, only: Option<&Name>) -> Result<Plan> {
    let channels: Vec<Channel> = repo
        .channels()?
        .into_iter()
        .filter(|c| only.is_none_or(|name| c.name == *name))
        .collect();
    if channels.is_empty() {
        return Err(Error::new(format!(
            "{} has no channel index to publish",
            repo.root().display()
        )));
    }
    let mut published = Vec::new();
    let mut referenced = Vec::new();
    let mut indexes = Vec::new();
    for channel in &channels {
        let index = signed_index(repo, channel)?;
        if let Some(bytes) = fsx::read_optional(&target.join(channel.index()))? {
            let current = Index::decode(&bytes).map_err(|e| {
                Error::new(format!("{}: {e}", target.join(channel.index()).display()))
            })?;
            if current.generation > index.generation {
                return Err(Error::new(format!(
                    "the target already has generation {} of {channel}, which is newer than {}",
                    current.generation, index.generation
                )));
            }
        }
        for entry in &index.packages {
            match fsx::size(&repo.path(&entry.file))? {
                Some(size) if size == entry.size => {}
                _ => {
                    return Err(Error::new(format!(
                        "{channel} references {}, which the repository lacks or holds with another size",
                        entry.file
                    )));
                }
            }
            referenced.push((entry.file.clone(), entry.size));
        }
        published.push((channel.clone(), index.generation));
        indexes.push(index);
    }
    referenced.sort();
    referenced.dedup();

    let mut steps = Vec::new();
    let mut removals = Vec::new();
    for dir in ["pool", "buildinfo"] {
        for name in fsx::list(&repo.path(dir))? {
            if name.starts_with('.') {
                continue;
            }
            let relative = format!("{dir}/{name}");
            immutable(repo, target, &relative, &mut steps)?;
        }
    }
    steps.push(Step::Verify(referenced));
    if fsx::size(&repo.path(KEYS))?.is_some() {
        if fsx::size(&repo.path(&sig(KEYS)))?.is_none() {
            return Err(Error::new(format!("{KEYS} is not signed")));
        }
        check_keys_signature(repo)?;
        changed(repo, target, KEYS, &mut steps)?;
        changed(repo, target, &sig(KEYS), &mut steps)?;
    }
    for (channel, index) in channels.iter().zip(&indexes) {
        let kept = repo.snapshots(channel)?;
        for generation in &kept {
            let snapshot = channel.snapshot(*generation);
            let signed = fsx::size(&repo.path(&sig(&snapshot)))?.is_some();
            if !signed && *generation != index.generation {
                continue;
            }
            immutable(repo, target, &snapshot, &mut steps)?;
            changed(repo, target, &sig(&snapshot), &mut steps)?;
        }
        changed(repo, target, &channel.index(), &mut steps)?;
        changed(repo, target, &sig(&channel.index()), &mut steps)?;
        let dir = format!("{}/snapshots", channel.dir());
        let stale: BTreeSet<u64> = fsx::list(&target.join(&dir))?
            .iter()
            .filter_map(|name| {
                let name = name.strip_suffix(SIG).unwrap_or(name);
                name.strip_suffix(".json.zst")?.parse().ok()
            })
            .filter(|g| !kept.contains(g))
            .collect();
        for generation in stale {
            let snapshot = channel.snapshot(generation);
            removals.push(Step::Remove(sig(&snapshot)));
            removals.push(Step::Remove(snapshot));
        }
    }
    steps.extend(removals);
    Ok(Plan {
        steps,
        channels: published,
    })
}

/// Applies one step.
pub(crate) fn apply(repo: &Repository, target: &Path, step: &Step) -> Result<()> {
    match step {
        Step::Copy(relative) => fsx::copy_atomic(&repo.path(relative), &target.join(relative)),
        Step::Verify(files) => {
            for (relative, size) in files {
                if fsx::size(&target.join(relative))? != Some(*size) {
                    return Err(Error::new(format!(
                        "the target does not hold {relative} with {size} bytes after copying it"
                    )));
                }
            }
            Ok(())
        }
        Step::Remove(relative) => fsx::remove(&target.join(relative)).map(drop),
    }
}

/// Reads the entry point of `channel` and refuses it unless its signature
/// file exists and names exactly this index in its trusted comment.
fn signed_index(repo: &Repository, channel: &Channel) -> Result<Index> {
    let path = repo.path(&channel.index());
    let bytes = fsx::read_optional(&path)?
        .ok_or_else(|| Error::new(format!("{} does not exist", path.display())))?;
    let index =
        Index::decode(&bytes).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
    let sig_path = repo.path(&sig(&channel.index()));
    let signature = fsx::read_optional(&sig_path)?.ok_or_else(|| {
        Error::new(format!(
            "{channel} generation {} is not signed, run laft-repo sign first",
            index.generation
        ))
    })?;
    let signature = Signature::decode(&signature)
        .map_err(|e| Error::new(format!("{}: {e}", sig_path.display())))?;
    let comment: IndexComment = signature
        .trusted_comment()
        .parse()
        .map_err(|e| Error::new(format!("{}: {e}", sig_path.display())))?;
    if comment != index.comment() {
        return Err(Error::new(format!(
            "{} is signed as \"{comment}\", but the index is \"{}\"",
            path.display(),
            index.comment()
        )));
    }
    Ok(index)
}

/// Refuses the repository's `keys.json` unless its signature names exactly
/// this document in its trusted comment.
fn check_keys_signature(repo: &Repository) -> Result<()> {
    let path = repo.path(KEYS);
    let sig_path = repo.path(&sig(KEYS));
    let keys = Keys::from_json(&std::fs::read(&path).map_err(io(&path))?)
        .map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
    let comment: KeysComment = Signature::decode(&std::fs::read(&sig_path).map_err(io(&sig_path))?)
        .map_err(|e| Error::new(format!("{}: {e}", sig_path.display())))?
        .trusted_comment()
        .parse()
        .map_err(|e| Error::new(format!("{}: {e}", sig_path.display())))?;
    if comment != keys.comment() {
        return Err(Error::new(format!(
            "{} is signed as \"{comment}\", but the document is \"{}\"",
            path.display(),
            keys.comment()
        )));
    }
    Ok(())
}

/// Adds a copy of a file that never changes once published, unless the
/// target already has it. A target copy of another size is an error.
fn immutable(
    repo: &Repository,
    target: &Path,
    relative: &str,
    steps: &mut Vec<Step>,
) -> Result<()> {
    let size = fsx::size(&repo.path(relative))?;
    match fsx::size(&target.join(relative))? {
        None => steps.push(Step::Copy(relative.to_owned())),
        Some(published) if Some(published) == size => {}
        Some(_) => {
            return Err(Error::new(format!(
                "the target holds another {relative}, but published files never change"
            )));
        }
    }
    Ok(())
}

/// Adds a copy of a file whose content may change, unless the target holds
/// the same bytes.
fn changed(repo: &Repository, target: &Path, relative: &str, steps: &mut Vec<Step>) -> Result<()> {
    let source = fsx::read_optional(&repo.path(relative))?;
    if source.is_some() && source != fsx::read_optional(&target.join(relative))? {
        steps.push(Step::Copy(relative.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
