//! `laft-repo index`: adds build outputs to a channel as a new generation.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use laft_core::{Name, StoreId, Timestamp};
use laft_index::{Defaults, Index};
use laft_pkg::Arch;

use crate::check::{self, Report, label};
use crate::error::{Error, Result};
use crate::fsx;
use crate::generation::{self, Options};
use crate::layout::{Channel, Repository};
use crate::package::Inputs;
use crate::sign::Signer;

/// What `laft-repo index` works on.
pub(crate) struct Request<'a> {
    /// The repository directory.
    pub(crate) repo: &'a Repository,
    /// The channel to add to.
    pub(crate) channel: &'a Channel,
    /// The repository name, needed when the channel has no index yet.
    pub(crate) name: Option<&'a Name>,
    /// Package files and directories of build outputs.
    pub(crate) inputs: &'a [PathBuf],
    /// Alternatives defaults to set, by group.
    pub(crate) alternatives: &'a [(Name, Name)],
    /// Provider defaults to set, by capability.
    pub(crate) providers: &'a [(String, Name)],
    /// Indexes of base repositories for the dependency checks.
    pub(crate) bases: &'a [Index],
    /// Whether packages from dirty or bootstrap builds are allowed.
    pub(crate) allow_bootstrap: bool,
    /// The key to sign the new generation with, if any.
    pub(crate) signer: Option<&'a Signer>,
    /// Timing and snapshot options.
    pub(crate) options: Options,
}

/// The outcome of a command that writes a new generation after checking
/// it.
pub(crate) enum Outcome {
    /// The generation was written. The report may hold warnings.
    Written {
        /// The new index.
        index: Box<Index>,
        /// How many packages are new in it.
        added: usize,
        /// The check report.
        report: Report,
    },
    /// The checks found errors, and nothing was written.
    Refused(Report),
}

/// Adds the packages of `request.inputs` to the channel, sets the given
/// defaults, checks the result and writes it as a new generation.
///
/// Package files are copied into the pool by store id, and the buildinfo
/// of each output to `buildinfo/<id>.json`. Files that already exist there
/// are kept, because published files never change. A package already in
/// the channel is left as it is.
pub(crate) fn run(request: &Request<'_>) -> Result<Outcome> {
    let Request { repo, channel, .. } = *request;
    let previous = repo.read_index(channel)?;
    let name = match (request.name, &previous) {
        (Some(name), Some(previous)) if *name != previous.repo => {
            return Err(Error::new(format!(
                "channel {channel} belongs to repository {}, not {name}",
                previous.repo
            )));
        }
        (Some(name), _) => name.clone(),
        (None, Some(previous)) => previous.repo.clone(),
        (None, None) => {
            return Err(Error::new(format!(
                "channel {channel} has no index yet, so --name is required"
            )));
        }
    };
    let inputs = Inputs::read(request.inputs)?;
    let mut index = previous.clone().unwrap_or_else(|| empty(name, channel));
    let listed: HashSet<StoreId> = index.packages.iter().map(|e| e.id).collect();
    let yanked: HashSet<_> = index
        .yanked
        .iter()
        .filter(|y| generation::retained(y, request.options.now))
        .map(|y| (&y.name, &y.version, y.rev, y.variant))
        .collect();
    let mut staged = BTreeMap::new();
    let mut added = 0;
    for (id, package) in &inputs.packages {
        let meta = &package.meta;
        if meta.arch != channel.arch && meta.arch != Arch::Any {
            return Err(Error::new(format!(
                "{}: {} is built for {}, not {}",
                package.path.display(),
                label(meta),
                meta.arch,
                channel.arch
            )));
        }
        if yanked.contains(&(&meta.name, &meta.version, meta.rev, meta.variant)) {
            return Err(Error::new(format!(
                "{} was yanked from {channel}, so publish a new revision instead",
                label(meta)
            )));
        }
        if listed.contains(id) {
            continue;
        }
        if let Some(other) = index.packages.iter().find(|e| {
            let m = &e.meta;
            (&m.name, &m.version, m.rev, m.variant)
                == (&meta.name, &meta.version, meta.rev, meta.variant)
        }) {
            return Err(Error::new(format!(
                "KR005 revision not bumped: {} is already listed with id {}, the new build has id {id}",
                label(meta),
                other.id
            )));
        }
        let mut entry = package.entry();
        let pool = repo.pool(*id);
        if fsx::size(&pool)?.is_some() {
            (entry.size, entry.blake3) = fsx::hash_file(&pool)?;
        } else {
            staged.insert(*id, package.path.clone());
        }
        index.packages.push(entry);
        added += 1;
    }
    for (group, member) in request.alternatives {
        index
            .defaults
            .alternatives
            .insert(group.clone(), member.clone());
    }
    for (provide, name) in request.providers {
        index
            .defaults
            .providers
            .insert(provide.clone(), name.clone());
    }
    let index = generation::prepare(repo, channel, index, &request.options)?;
    let others = repo.other_indexes(channel)?;
    let report = check::check(&check::Input {
        repo,
        index: &index,
        staged: &staged,
        staged_buildinfo: &inputs.buildinfo,
        published: previous.as_ref(),
        others: &others,
        bases: request.bases,
        allow_bootstrap: request.allow_bootstrap,
    });
    if report.errors > 0 {
        return Ok(Outcome::Refused(report));
    }
    if let Some(signer) = request.signer {
        generation::check_signer(repo, signer, &index)?;
    }
    for (id, path) in &staged {
        fsx::copy_atomic(path, &repo.pool(*id))?;
    }
    for entry in &index.packages {
        let target = repo.buildinfo(entry.id);
        if let Some(source) = inputs.buildinfo.get(&entry.id)
            && fsx::size(&target)?.is_none()
        {
            fsx::copy_atomic(source, &target)?;
        }
    }
    generation::write(repo, channel, &index, request.signer, &request.options)?;
    Ok(Outcome::Written {
        index: Box::new(index),
        added,
        report,
    })
}

/// An index of `channel` without packages, to start a new channel from.
pub(crate) fn empty(repo: Name, channel: &Channel) -> Index {
    Index {
        format: Index::FORMAT,
        repo,
        channel: channel.name.clone(),
        arch: channel.arch,
        generation: 0,
        timestamp: Timestamp::from_secs(0),
        expires: Timestamp::from_secs(1),
        defaults: Defaults::default(),
        packages: Vec::new(),
        yanked: Vec::new(),
    }
}
