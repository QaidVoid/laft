//! `laft-repo yank`: withdraws packages from a channel.

use laft_core::{Name, Variant, Version};
use laft_index::{Index, Yank};

use crate::error::{Error, Result};
use crate::generation::{self, Options};
use crate::layout::{Channel, Repository};
use crate::sign::Signer;

/// Which packages to withdraw and why.
pub(crate) struct Request<'a> {
    /// The package name.
    pub(crate) name: &'a Name,
    /// The upstream version.
    pub(crate) version: &'a Version,
    /// Only this revision, or every revision of the version.
    pub(crate) rev: Option<u32>,
    /// Only this variant, or both.
    pub(crate) variant: Option<Variant>,
    /// Why, in one line shown to users.
    pub(crate) reason: &'a str,
}

/// Moves every package of `channel` that `request` selects from the
/// available packages to the yanked ones, with the reason and the current
/// time, and writes the result as a new generation.
///
/// The repository checks do not run, because withdrawing a broken
/// package must not wait for its dependents to be fixed.
pub(crate) fn run(
    repo: &Repository,
    channel: &Channel,
    request: &Request<'_>,
    signer: Option<&Signer>,
    options: &Options,
) -> Result<(Index, Vec<Yank>)> {
    if request.reason.trim().is_empty() || request.reason.chars().any(char::is_control) {
        return Err(Error::new("the reason is one line of text"));
    }
    let mut index = repo
        .read_index(channel)?
        .ok_or_else(|| Error::new(format!("channel {channel} has no index")))?;
    let selected = |m: &laft_pkg::Meta| {
        m.name == *request.name
            && m.version == *request.version
            && request.rev.is_none_or(|rev| rev == m.rev)
            && request.variant.is_none_or(|variant| variant == m.variant)
    };
    let (withdrawn, kept): (Vec<_>, Vec<_>) =
        index.packages.into_iter().partition(|e| selected(&e.meta));
    if withdrawn.is_empty() {
        return Err(Error::new(format!(
            "channel {channel} lists no package {} {}",
            request.name, request.version
        )));
    }
    index.packages = kept;
    let yanks: Vec<Yank> = withdrawn
        .into_iter()
        .map(|e| Yank {
            id: e.id,
            name: e.meta.name,
            version: e.meta.version,
            rev: e.meta.rev,
            variant: e.meta.variant,
            reason: request.reason.trim().to_owned(),
            time: options.now,
        })
        .collect();
    index.yanked.extend(yanks.iter().cloned());
    let index = generation::prepare(repo, channel, index, options)?;
    generation::write(repo, channel, &index, signer, options)?;
    Ok((index, yanks))
}
