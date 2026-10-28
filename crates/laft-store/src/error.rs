use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use laft_core::{Name, StoreId};

use crate::{LockInfo, State};

/// An error from the store, view assembly, a transaction or garbage
/// collection.
#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed.
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A package file or a store entry's documents are invalid or do not
    /// match their manifest.
    Package(laft_pkg::Error),
    /// A package's store id differs from the one the caller expected.
    IdMismatch {
        /// The id the caller expected.
        expected: StoreId,
        /// The id computed from the package's manifest.
        actual: StoreId,
    },
    /// No store entry has this directory name.
    MissingEntry(String),
    /// A file that Laft writes itself is malformed, for example a journal or
    /// a `gen.json` that does not parse, or a record that cannot be written
    /// as canonical JSON.
    Corrupt {
        /// The malformed file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// Several packages in one view own the same paths.
    Conflicts {
        /// The view being assembled.
        view: String,
        /// Every conflicting path, sorted.
        conflicts: Vec<Conflict>,
    },
    /// The chosen member of an alternatives group is not in the view.
    AlternativeAbsent {
        /// The view being assembled.
        view: String,
        /// The alternatives group.
        group: Name,
        /// The package the caller chose.
        chosen: Name,
        /// The members of the group that are in the view, sorted.
        present: Vec<Name>,
    },
    /// Several members of an alternatives group are in the view and none was
    /// chosen.
    AlternativeAmbiguous {
        /// The view being assembled.
        view: String,
        /// The alternatives group.
        group: Name,
        /// The members of the group that are in the view, sorted.
        members: Vec<Name>,
    },
    /// Two store entries of the same package name were given for one view.
    DuplicatePackage {
        /// The view being assembled.
        view: String,
        /// The package name.
        name: Name,
    },
    /// Linking a store file once more would come too close to the
    /// filesystem's hard link limit.
    LinkLimit {
        /// The store file.
        path: PathBuf,
        /// Its current link count.
        links: u64,
        /// The filesystem's link limit.
        limit: u64,
    },
    /// Another process holds the transaction lock.
    Busy(Option<LockInfo>),
    /// An interrupted transaction is still recorded and must be recovered
    /// first.
    Pending {
        /// The interrupted transaction.
        txid: String,
        /// The state it was interrupted in.
        state: State,
    },
    /// A request that the current state does not allow, for example adding
    /// a package after assembly has started. The message says which.
    Invalid(String),
    /// A failure injected by a test at a named point.
    #[cfg(test)]
    Injected(&'static str),
}

/// One path that several packages in a view own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The path, relative to the view tree.
    pub path: String,
    /// Every owner, as `<name> <version>-r<rev>`, in the order the packages
    /// were considered.
    pub owners: Vec<String>,
}

/// The result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Wraps an I/O error from an operation on `path`.
pub(crate) fn io<E: Into<io::Error>>(path: &Path) -> impl FnOnce(E) -> Error + '_ {
    move |e| Error::Io {
        path: path.to_path_buf(),
        source: e.into(),
    }
}

/// An error for a malformed file Laft wrote.
pub(crate) fn corrupt(path: &Path, reason: impl fmt::Display) -> Error {
    Error::Corrupt {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

impl From<laft_pkg::Error> for Error {
    fn from(e: laft_pkg::Error) -> Self {
        Self::Package(e)
    }
}

fn list(names: &[Name]) -> String {
    names
        .iter()
        .map(Name::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Package(e) => e.fmt(f),
            Self::IdMismatch { expected, actual } => write!(
                f,
                "package has store id {actual}, but {expected} was expected"
            ),
            Self::MissingEntry(dir) => write!(f, "store entry {dir} does not exist"),
            Self::Corrupt { path, reason } => {
                write!(f, "{} is corrupt: {reason}", path.display())
            }
            Self::Conflicts { view, conflicts } => {
                let s = if conflicts.len() == 1 { "" } else { "s" };
                write!(f, "view {view:?}: {} path conflict{s}", conflicts.len())?;
                for conflict in conflicts {
                    write!(f, "\n  {}", conflict.path)?;
                    for owner in &conflict.owners {
                        write!(f, "\n    {owner}")?;
                    }
                }
                write!(
                    f,
                    "\n  hint: these packages do not declare a common alternatives group, so remove one of them"
                )
            }
            Self::AlternativeAbsent {
                view,
                group,
                chosen,
                present,
            } => write!(
                f,
                "alternatives.{group} = {chosen}, but {chosen} is not in view {view:?}\n  members present: {}",
                list(present)
            ),
            Self::AlternativeAmbiguous {
                view,
                group,
                members,
            } => write!(
                f,
                "alternatives group \"{group}\" has several members in view {view:?}: {}\n  hint: choose one with `alternatives.{group} = {}`",
                list(members),
                members.first().map_or("<package>", Name::as_str)
            ),
            Self::DuplicatePackage { view, name } => {
                write!(f, "view {view:?} contains package {name} more than once")
            }
            Self::LinkLimit { path, links, limit } => write!(
                f,
                "{} has {links} links (limit {limit})\n  hint: run `laft gc` or lower gc.keep_generations",
                path.display()
            ),
            Self::Busy(Some(info)) => write!(
                f,
                "the lock is held by pid {} ({}, started {})",
                info.pid, info.command, info.started
            ),
            Self::Busy(None) => f.write_str("the lock is held by another process"),
            Self::Pending { txid, state } => write!(
                f,
                "transaction {txid} was interrupted while {state} and must be recovered first"
            ),
            Self::Invalid(message) => f.write_str(message),
            #[cfg(test)]
            Self::Injected(point) => write!(f, "injected failure at {point}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Package(e) => Some(e),
            _ => None,
        }
    }
}
