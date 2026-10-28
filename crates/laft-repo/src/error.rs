//! The error of every command.

use std::fmt;
use std::io;
use std::path::Path;

/// A failed command, printed as `laft-repo: <message>`.
#[derive(Debug)]
pub(crate) struct Error(String);

/// The result of a command step.
pub(crate) type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// An error with `message`, which names what failed and why.
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// Wraps an I/O error so that it names `path`.
pub(crate) fn io(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |e| Error(format!("{}: {e}", path.display()))
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<laft_index::Error> for Error {
    fn from(e: laft_index::Error) -> Self {
        Self(e.to_string())
    }
}

impl From<laft_pkg::Error> for Error {
    fn from(e: laft_pkg::Error) -> Self {
        Self(e.to_string())
    }
}

impl From<minisign::PError> for Error {
    fn from(e: minisign::PError) -> Self {
        Self(format!("minisign: {e}"))
    }
}
