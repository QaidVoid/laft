//! New generations of a channel index and their signatures.
//!
//! A generation is written in the order a reader of the repository can
//! follow safely: the immutable snapshot and its signature first, then the
//! entry point, then the entry point's signature. Snapshots beyond the
//! number to keep are removed last, oldest first.

use std::cmp::Reverse;

use laft_core::Timestamp;
use laft_index::{Index, IndexComment, Keys, next_generation};

use crate::error::{Error, Result};
use crate::fsx::{self, FILE_MODE};
use crate::layout::{Channel, KEYS, Repository, sig};
use crate::sign::Signer;

/// How long a yanked package stays listed, in seconds: 90 days.
pub(crate) const YANK_RETENTION: u64 = 90 * 86_400;

/// When and for how long a new generation is valid, and how many snapshots
/// to keep.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Options {
    /// The signing time, which is also the generation.
    pub(crate) now: Timestamp,
    /// Seconds from signing to expiry.
    pub(crate) expires_in: u64,
    /// How many snapshots of the channel to keep, at least 1.
    pub(crate) keep: usize,
}

/// Turns `index`, whose packages, yanked packages and defaults are filled
/// in, into the next generation of `channel`: the generation and timestamp
/// become the signing time, the expiry follows from it, both lists are
/// sorted and yanked packages older than [`YANK_RETENTION`] are dropped.
///
/// Fails when the signing time is not later than the last generation of
/// the channel, as an entry point or a snapshot, or when the result is not
/// a valid index.
pub(crate) fn prepare(
    repo: &Repository,
    channel: &Channel,
    mut index: Index,
    options: &Options,
) -> Result<Index> {
    let now = options.now;
    index.generation = next_generation(now, last_generation(repo, channel)?)?;
    index.timestamp = now;
    index.expires = Timestamp::from_secs(now.as_secs().saturating_add(options.expires_in));
    index.channel = channel.name.clone();
    index.arch = channel.arch;
    index
        .yanked
        .retain(|y| now.as_secs().saturating_sub(y.time.as_secs()) <= YANK_RETENTION);
    sort(&mut index);
    index.validate()?;
    Ok(index)
}

/// The highest generation of `channel` in the repository, if any.
fn last_generation(repo: &Repository, channel: &Channel) -> Result<Option<u64>> {
    let current = repo.read_index(channel)?.map(|i| i.generation);
    let snapshot = repo.snapshots(channel)?.last().copied();
    Ok(current.max(snapshot))
}

/// Sorts packages by name, then by version and revision descending, then
/// dynamic before static, and yanked packages the same way.
pub(crate) fn sort(index: &mut Index) {
    index.packages.sort_by(|a, b| {
        let key = |e: &laft_index::IndexEntry| {
            let m = &e.meta;
            (
                m.name.clone(),
                Reverse((m.version.clone(), m.rev)),
                m.variant,
            )
        };
        key(a).cmp(&key(b))
    });
    index.yanked.sort_by(|a, b| {
        let key = |y: &laft_index::Yank| {
            (
                y.name.clone(),
                Reverse((y.version.clone(), y.rev)),
                y.variant,
            )
        };
        key(a).cmp(&key(b))
    });
}

/// Writes `index` as the new generation of `channel`, signed with `signer`
/// when one is given, and removes the oldest snapshots beyond
/// `options.keep`.
///
/// An unsigned generation removes the old signature before it replaces
/// the entry point, so the entry point never carries a signature of
/// another generation. `laft-repo sign` signs it later.
pub(crate) fn write(
    repo: &Repository,
    channel: &Channel,
    index: &Index,
    signer: Option<&Signer>,
    options: &Options,
) -> Result<()> {
    let bytes = index.encode()?;
    let signature = match signer {
        Some(signer) => {
            check_signer(repo, signer, index)?;
            Some(signer.sign(&bytes, &index.comment().to_string())?)
        }
        None => None,
    };
    let snapshot = channel.snapshot(index.generation);
    let entry = channel.index();
    fsx::write_atomic(&repo.path(&snapshot), &bytes, FILE_MODE)?;
    match &signature {
        Some(signature) => fsx::write_atomic(&repo.path(&sig(&snapshot)), signature, FILE_MODE)?,
        None => drop(fsx::remove(&repo.path(&sig(&entry)))?),
    }
    fsx::write_atomic(&repo.path(&entry), &bytes, FILE_MODE)?;
    if let Some(signature) = &signature {
        fsx::write_atomic(&repo.path(&sig(&entry)), signature, FILE_MODE)?;
    }
    prune(repo, channel, options.keep.max(1))
}

/// Removes the oldest snapshots of `channel` until `keep` remain.
fn prune(repo: &Repository, channel: &Channel, keep: usize) -> Result<()> {
    let generations = repo.snapshots(channel)?;
    let excess = generations.len().saturating_sub(keep);
    for generation in &generations[..excess] {
        let snapshot = channel.snapshot(*generation);
        fsx::remove(&repo.path(&sig(&snapshot)))?;
        fsx::remove(&repo.path(&snapshot))?;
    }
    Ok(())
}

/// Signs the current entry point of `channel` and its snapshot with
/// `signer`, using the trusted comment the index names.
pub(crate) fn sign(repo: &Repository, channel: &Channel, signer: &Signer) -> Result<IndexComment> {
    let entry = channel.index();
    let path = repo.path(&entry);
    let bytes = fsx::read_optional(&path)?
        .ok_or_else(|| Error::new(format!("{} does not exist", path.display())))?;
    let index =
        Index::decode(&bytes).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
    check_signer(repo, signer, &index)?;
    let comment = index.comment();
    let signature = signer.sign(&bytes, &comment.to_string())?;
    let snapshot = channel.snapshot(index.generation);
    if fsx::read_optional(&repo.path(&snapshot))?.as_deref() != Some(&bytes) {
        fsx::write_atomic(&repo.path(&snapshot), &bytes, FILE_MODE)?;
    }
    fsx::write_atomic(&repo.path(&sig(&snapshot)), &signature, FILE_MODE)?;
    fsx::write_atomic(&repo.path(&sig(&entry)), &signature, FILE_MODE)?;
    Ok(comment)
}

/// Refuses a key that clients would refuse for `index`: a key the
/// repository's `keys.json` revokes, or a listed online key whose validity
/// window does not contain the signing time. A key that `keys.json` does
/// not list is taken as a root key, which clients always accept.
pub(crate) fn check_signer(repo: &Repository, signer: &Signer, index: &Index) -> Result<()> {
    let path = repo.path(KEYS);
    let Some(bytes) = fsx::read_optional(&path)? else {
        return Ok(());
    };
    let keys =
        Keys::from_json(&bytes).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
    let id = signer.id();
    if keys.is_revoked(id) {
        return Err(Error::new(format!("key {id} is revoked in {KEYS}")));
    }
    if let Some(online) = keys.key(id)
        && !(online.not_before <= index.timestamp && index.timestamp <= online.not_after)
    {
        return Err(Error::new(format!(
            "key {id} is valid from {} to {}, which does not contain the signing time {}",
            online.not_before, online.not_after, index.timestamp
        )));
    }
    Ok(())
}
