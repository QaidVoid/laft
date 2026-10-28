use std::fmt;

/// A file that starts with the ELF magic but is malformed, or that names a
/// library in a form Laft cannot express as a dependency.
///
/// The message says what is wrong, for example `invalid ELF file: more than
/// one DT_SONAME entry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    reason: String,
}

impl Error {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub(crate) fn object(error: object::read::Error) -> Self {
        Self::new(error.to_string())
    }

    /// Why the file was rejected.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid ELF file: {}", self.reason)
    }
}

impl std::error::Error for Error {}
