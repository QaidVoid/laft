use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::ServiceKind;

/// An error from writing or reading rendered services, or from the
/// launcher.
///
/// Every launcher error means the service was not started, and the launcher
/// exits with status 111 for all of them, which makes the supervisor try
/// again later.
#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed.
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A file that Laft wrote is malformed, for example a spec file or an
    /// account database line.
    Corrupt {
        /// The malformed file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// A spec cannot be written as canonical JSON.
    Encode {
        /// The service.
        name: String,
        /// Why.
        reason: String,
    },
    /// The active generation has no spec for the service.
    NoSpec(String),
    /// The command does not apply to the service's kind.
    WrongKind {
        /// The service.
        name: String,
        /// The kind the command needs.
        expected: ServiceKind,
    },
    /// The logger was started for a service whose output is not logged.
    NotLogged(String),
    /// A dependency is not ready yet.
    Waiting {
        /// The service.
        name: String,
        /// The first dependency without a ready marker.
        dep: String,
    },
    /// An account the service needs does not exist.
    UnknownAccount {
        /// The service.
        name: String,
        /// `user` or `group`.
        kind: &'static str,
        /// The account name.
        account: String,
    },
    /// A system call that sets up the service's process failed.
    Setup {
        /// The service.
        name: String,
        /// What was being done.
        step: &'static str,
        /// The underlying error.
        source: io::Error,
    },
    /// The service's command could not be executed.
    Exec {
        /// The service.
        name: String,
        /// The program.
        program: String,
        /// The underlying error.
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Corrupt { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::Encode { name, reason } => {
                write!(f, "the spec of service {name} cannot be encoded: {reason}")
            }
            Self::NoSpec(name) => {
                write!(f, "no spec for service {name} in the active generation")
            }
            Self::WrongKind { name, expected } => {
                write!(f, "service {name} is not a {}", expected.as_str())
            }
            Self::NotLogged(name) => write!(f, "service {name} is not logged"),
            Self::Waiting { name, dep } => write!(f, "{name} waiting for {dep}"),
            Self::UnknownAccount {
                name,
                kind,
                account,
            } => write!(f, "service {name}: no {kind} named {account:?}"),
            Self::Setup { name, step, source } => {
                write!(f, "service {name}: cannot {step}: {source}")
            }
            Self::Exec {
                name,
                program,
                source,
            } => write!(f, "service {name}: cannot execute {program}: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::Setup { source, .. } | Self::Exec { source, .. } => {
                Some(source)
            }
            _ => None,
        }
    }
}

/// Wraps an I/O error with the path it is about.
pub(crate) fn io(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

/// The error for a malformed file that Laft wrote.
pub(crate) fn corrupt(path: &Path, reason: impl Into<String>) -> Error {
    Error::Corrupt {
        path: path.to_owned(),
        reason: reason.into(),
    }
}
