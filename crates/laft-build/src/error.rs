//! Why a build failed, and the warnings a build reports.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::{Problem, SourceError};

/// Why a build, or a step of one, failed.
#[derive(Debug)]
pub enum BuildError {
    /// A failure with a stable code, such as a phase that failed (`B040`)
    /// or a payload that breaks a post-processing rule (`B1xx`).
    Problem(Problem),
    /// A source could not be fetched or unpacked.
    Source(SourceError),
    /// The build-side store or a view could not be read or written.
    Store(laft_store::Error),
    /// A sandbox could not be set up, for example because the kernel
    /// refuses unprivileged user namespaces. It says nothing about the
    /// recipe.
    Sandbox(laft_sandbox::Error),
    /// A package could not be written or read.
    Package(laft_pkg::Error),
    /// Reading or writing a local file failed.
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The underlying error.
        error: io::Error,
    },
}

impl BuildError {
    /// A failure with `code` and `message`.
    pub fn problem(code: &'static str, message: impl Into<String>) -> Self {
        Self::Problem(Problem::new(code, message))
    }

    /// The stable code of a [`BuildError::Problem`] or a
    /// [`BuildError::Source`], or `None` for the other kinds.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Problem(p) => Some(p.code),
            Self::Source(e) => Some(e.code()),
            _ => None,
        }
    }

    /// Whether the failure lies in the machine rather than in the recipe,
    /// such as a kernel that refuses the sandbox.
    pub fn is_environment(&self) -> bool {
        matches!(self, Self::Sandbox(_))
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Problem(p) => write!(f, "{p}"),
            Self::Source(e) => write!(f, "error[{}]: {e}", e.code()),
            Self::Store(e) => write!(f, "store: {e}"),
            Self::Sandbox(e) => write!(f, "sandbox: {e}"),
            Self::Package(e) => write!(f, "package: {e}"),
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Problem(e) => Some(e),
            Self::Source(e) => Some(e),
            Self::Store(e) => Some(e),
            Self::Sandbox(e) => Some(e),
            Self::Package(e) => Some(e),
            Self::Io { error, .. } => Some(error),
        }
    }
}

impl From<Problem> for BuildError {
    fn from(problem: Problem) -> Self {
        Self::Problem(problem)
    }
}

impl From<SourceError> for BuildError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<laft_store::Error> for BuildError {
    fn from(error: laft_store::Error) -> Self {
        Self::Store(error)
    }
}

impl From<laft_sandbox::Error> for BuildError {
    fn from(error: laft_sandbox::Error) -> Self {
        Self::Sandbox(error)
    }
}

impl From<laft_pkg::Error> for BuildError {
    fn from(error: laft_pkg::Error) -> Self {
        Self::Package(error)
    }
}

/// Maps an I/O error about `path` to a [`BuildError::Io`].
pub(crate) fn io(path: &Path) -> impl Fn(io::Error) -> BuildError + '_ {
    move |error| BuildError::Io {
        path: path.to_path_buf(),
        error,
    }
}

/// Something a build accepted but reports, with its stable code, for
/// example `W110` for a static archive removed from a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// The stable code, for example `W110`.
    pub code: &'static str,
    /// What was found, starting lowercase and without a final period.
    pub message: String,
}

impl Warning {
    /// A warning with `code` and `message`.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "warning[{}]: {}", self.code, self.message)
    }
}
