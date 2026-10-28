use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use laft_core::Blake3Hash;

/// An error from fetching or caching a repository file.
///
/// Every error names the URL it is about when there is one, so it prints
/// like this:
///
/// ```text
/// https://pkgs.example.org/laft/keys.json: timed out
/// ```
///
/// When every URL of a repository failed, [`ErrorKind::AllFailed`] lists
/// each attempt on its own line.
#[derive(Debug)]
pub struct Error {
    url: Option<String>,
    kind: ErrorKind,
}

/// What went wrong.
#[derive(Debug)]
pub enum ErrorKind {
    /// The URL uses a scheme other than `https`, `http` and `file`.
    UnsupportedScheme,
    /// The URL uses `http`, which the repository does not allow.
    HttpNotAllowed,
    /// The URL is malformed.
    InvalidUrl {
        /// What is wrong with it.
        reason: String,
    },
    /// A path inside a repository is empty, absolute, or has a component
    /// that is empty, `.`, `..` or uses a character outside
    /// `[A-Za-z0-9._-]`.
    InvalidPath {
        /// The rejected path.
        path: String,
    },
    /// Offline mode forbids network access.
    Offline,
    /// Offline mode forbids network access, and the file is not cached.
    NotCached {
        /// Where the cached file would be.
        path: PathBuf,
    },
    /// The server answered HTTP 404, or a local repository has no such file.
    NotFound,
    /// The server answered with an unexpected HTTP status.
    Status {
        /// The status code.
        code: u16,
    },
    /// The server did not answer or stopped sending in time.
    Timeout,
    /// The connection, TLS handshake or HTTP exchange failed.
    Transport {
        /// The cause.
        reason: String,
    },
    /// The file is larger than its limit.
    TooLarge {
        /// The limit, in bytes.
        limit: u64,
    },
    /// The file does not have the expected size.
    SizeMismatch {
        /// The expected size, in bytes.
        expected: u64,
        /// The actual size, in bytes.
        actual: u64,
    },
    /// The file does not have the expected BLAKE3 hash.
    HashMismatch {
        /// The expected hash.
        expected: Blake3Hash,
        /// The actual hash.
        actual: Blake3Hash,
    },
    /// Reading or writing a local file failed.
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// Every URL failed. The attempts are in the order they were made.
    AllFailed {
        /// What was being fetched, for example a path in the repository.
        what: String,
        /// The error of each attempt.
        attempts: Vec<Error>,
    },
}

impl Error {
    pub(crate) fn new(url: impl Into<String>, kind: ErrorKind) -> Self {
        Self {
            url: Some(url.into()),
            kind,
        }
    }

    /// The URL the error is about, if any.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Whether trying the same URL again may succeed.
    pub(crate) fn is_transient(&self) -> bool {
        match self.kind {
            ErrorKind::Timeout | ErrorKind::Transport { .. } => true,
            ErrorKind::Status { code } => code == 408 || code == 429 || code >= 500,
            _ => false,
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self { url: None, kind }
    }
}

/// Wraps an I/O error from an operation on `path`.
pub(crate) fn io(path: &Path) -> impl Fn(io::Error) -> Error + '_ {
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
        if let Some(url) = &self.url {
            write!(f, "{url}: ")?;
        }
        fmt::Display::fmt(&self.kind, f)
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedScheme => {
                f.write_str("unsupported URL scheme, expected https://, http:// or file://")
            }
            Self::HttpNotAllowed => {
                f.write_str("plain http:// is refused because the repository does not allow it")
            }
            Self::InvalidUrl { reason } => write!(f, "invalid URL: {reason}"),
            Self::InvalidPath { path } => write!(f, "invalid repository path {path:?}"),
            Self::Offline => f.write_str("network access is disabled in offline mode"),
            Self::NotCached { path } => write!(
                f,
                "not cached at {}, and network access is disabled in offline mode",
                path.display()
            ),
            Self::NotFound => f.write_str("not found"),
            Self::Status { code } => write!(f, "the server answered with HTTP status {code}"),
            Self::Timeout => f.write_str("timed out"),
            Self::Transport { reason } => f.write_str(reason),
            Self::TooLarge { limit } => write!(f, "larger than the limit of {limit} bytes"),
            Self::SizeMismatch { expected, actual } => {
                write!(f, "expected {expected} bytes, got {actual}")
            }
            Self::HashMismatch { expected, actual } => {
                write!(f, "expected BLAKE3 {expected}, got {actual}")
            }
            Self::Io { path, source } => write!(f, "cannot access {}: {source}", path.display()),
            Self::AllFailed { what, attempts } => {
                write!(f, "every URL failed for {what}")?;
                for attempt in attempts {
                    write!(f, "\n  {attempt}")?;
                }
                Ok(())
            }
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
