use std::fmt;
use std::io;
use std::path::PathBuf;

use laft_core::{Name, Timestamp};
use laft_pkg::Arch;

use crate::KeyId;

/// An error from parsing, verifying or accepting an index or a `keys.json`.
///
/// The error names the repository when it is known. A rejected rollback
/// prints like this:
///
/// ```text
/// repo core: index generation 1761600000 for current/x86_64-v3 is lower than the accepted generation 1761652800, refusing a possible rollback
/// ```
#[derive(Debug)]
pub struct Error {
    repo: Option<Name>,
    kind: ErrorKind,
}

/// What went wrong.
#[derive(Debug)]
pub enum ErrorKind {
    /// Reading or writing a state record failed.
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A state record exists but cannot be read as one. The repository's
    /// records must be reset before it can be synced again.
    CorruptState {
        /// The record.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// A document, signature, key or trusted comment breaks a format rule.
    Invalid {
        /// What was being read, for example `index` or `public key`.
        what: &'static str,
        /// The rule that is broken.
        reason: String,
    },
    /// The index decompresses to more than the limit.
    TooLarge {
        /// The limit, in bytes.
        limit: u64,
    },
    /// The signature was made by a key that is neither a configured root key
    /// nor an online key of the accepted `keys.json`.
    UnknownSigner {
        /// The signed document, `index` or `keys.json`.
        document: &'static str,
        /// The key that made the signature.
        signer: KeyId,
        /// The keys that would have been accepted, in configuration order
        /// followed by the online keys.
        trusted: Vec<KeyId>,
    },
    /// The signature does not verify: the document or its signature was
    /// modified.
    BadSignature {
        /// The signed document, `index` or `keys.json`.
        document: &'static str,
        /// The key the signature claims to be from.
        signer: KeyId,
    },
    /// The index is signed by a key that the accepted `keys.json` revokes.
    RevokedKey {
        /// The revoked key.
        signer: KeyId,
    },
    /// The index is signed by an online key whose validity window does not
    /// contain the index's signing time.
    KeyNotValid {
        /// The online key.
        signer: KeyId,
        /// The signing time of the index.
        timestamp: Timestamp,
        /// The start of the key's window.
        not_before: Timestamp,
        /// The end of the key's window.
        not_after: Timestamp,
    },
    /// The trusted comment of a signature names another repository, channel
    /// or architecture than the configuration, so a document meant for
    /// somewhere else was substituted.
    WrongTarget {
        /// The signed document, `index` or `keys.json`.
        document: &'static str,
        /// The field, for example `channel`.
        field: &'static str,
        /// The configured value.
        expected: String,
        /// The signed value.
        actual: String,
    },
    /// A field of the document differs from the trusted comment of its
    /// signature.
    Inconsistent {
        /// The signed document, `index` or `keys.json`.
        document: &'static str,
        /// The field, for example `generation`.
        field: &'static str,
        /// The value in the trusted comment.
        signed: String,
        /// The value in the document.
        actual: String,
    },
    /// The index generation is lower than the accepted one, so an old index
    /// may be replayed.
    Rollback {
        /// The channel of the index.
        channel: Name,
        /// The architecture of the index.
        arch: Arch,
        /// The offered generation.
        generation: u64,
        /// The accepted generation.
        accepted: u64,
    },
    /// The index has the accepted generation but different bytes.
    Replaced {
        /// The channel of the index.
        channel: Name,
        /// The architecture of the index.
        arch: Arch,
        /// The generation.
        generation: u64,
    },
    /// The `keys.json` version is lower than the accepted one, so an old
    /// `keys.json` may be replayed to revive a revoked key.
    KeysRollback {
        /// The offered version.
        version: u64,
        /// The accepted version.
        accepted: u64,
    },
    /// The `keys.json` has the accepted version but different bytes.
    KeysReplaced {
        /// The version.
        version: u64,
    },
    /// The repository serves no `keys.json`, but one was accepted before. A
    /// repository never falls back to signatures by the root key alone this
    /// way, because hiding the file would otherwise remove every revocation.
    KeysMissing,
    /// The index expired, so a mirror may be withholding newer ones.
    Expired {
        /// The index generation.
        generation: u64,
        /// When it expired.
        expires: Timestamp,
    },
}

impl Error {
    /// Attaches the repository the error is about, unless one is attached
    /// already.
    pub(crate) fn in_repo(mut self, repo: &Name) -> Self {
        self.repo.get_or_insert_with(|| repo.clone());
        self
    }

    pub(crate) fn invalid(what: &'static str, reason: impl fmt::Display) -> Self {
        ErrorKind::Invalid {
            what,
            reason: reason.to_string(),
        }
        .into()
    }

    /// The repository, when the error happened while accepting a document
    /// for one.
    pub fn repo(&self) -> Option<&Name> {
        self.repo.as_ref()
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self { repo: None, kind }
    }
}

/// Wraps an I/O error from an operation on `path`.
pub(crate) fn io(path: &std::path::Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| {
        ErrorKind::Io {
            path: path.to_path_buf(),
            source,
        }
        .into()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(repo) = &self.repo {
            write!(f, "repo {repo}: ")?;
        }
        fmt::Display::fmt(&self.kind, f)
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot access {}: {source}", path.display()),
            Self::CorruptState { path, reason } => write!(
                f,
                "state record {} is corrupt and must be reset: {reason}",
                path.display()
            ),
            Self::Invalid { what, reason } => write!(f, "invalid {what}: {reason}"),
            Self::TooLarge { limit } => {
                write!(f, "the index decompresses to more than {limit} bytes")
            }
            Self::UnknownSigner {
                document,
                signer,
                trusted,
            } => {
                write!(f, "{document} is signed by untrusted key {signer}, trusted keys are ")?;
                if trusted.is_empty() {
                    f.write_str("none")?;
                }
                for (i, key) in trusted.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{key}")?;
                }
                Ok(())
            }
            Self::BadSignature { document, signer } => write!(
                f,
                "the signature of {document} by key {signer} does not verify, the file or its signature was modified"
            ),
            Self::RevokedKey { signer } => write!(
                f,
                "the index is signed by key {signer}, which keys.json revokes"
            ),
            Self::KeyNotValid {
                signer,
                timestamp,
                not_before,
                not_after,
            } => write!(
                f,
                "the index was signed at {timestamp} by key {signer}, which is valid only from {not_before} to {not_after}"
            ),
            Self::WrongTarget {
                document,
                field,
                expected,
                actual,
            } => write!(
                f,
                "{document} is signed for {field} {actual}, but the repository is configured for {field} {expected}"
            ),
            Self::Inconsistent {
                document,
                field,
                signed,
                actual,
            } => write!(
                f,
                "{document} has {field} {actual}, but its signature says {signed}"
            ),
            Self::Rollback {
                channel,
                arch,
                generation,
                accepted,
            } => write!(
                f,
                "index generation {generation} for {channel}/{arch} is lower than the accepted generation {accepted}, refusing a possible rollback"
            ),
            Self::Replaced {
                channel,
                arch,
                generation,
            } => write!(
                f,
                "index generation {generation} for {channel}/{arch} differs from the index accepted with the same generation"
            ),
            Self::KeysRollback { version, accepted } => write!(
                f,
                "keys.json version {version} is lower than the accepted version {accepted}, refusing a possible rollback"
            ),
            Self::KeysReplaced { version } => write!(
                f,
                "keys.json version {version} differs from the keys.json accepted with the same version"
            ),
            Self::KeysMissing => f.write_str(
                "keys.json is missing although one was accepted before, refusing signatures by the root key alone",
            ),
            Self::Expired {
                generation,
                expires,
            } => write!(
                f,
                "index generation {generation} expired at {expires}, refusing a possible freeze"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
