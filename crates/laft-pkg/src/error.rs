use std::fmt;
use std::io;

/// An error from validating, scanning, writing or reading a package.
///
/// The error names the package when it is known and the path inside the
/// package or payload when there is one. A content mismatch prints like this:
///
/// ```text
/// package ripgrep-14.1.1-r1 is corrupt: content hash differs
///   path:     usr/bin/rg
///   expected: b3:7q2m...
///   actual:   b3:k93x...
/// ```
#[derive(Debug)]
pub struct Error {
    package: Option<String>,
    path: Option<String>,
    kind: ErrorKind,
}

/// What went wrong.
#[derive(Debug)]
pub enum ErrorKind {
    /// Reading or writing a file or stream failed.
    Io(io::Error),
    /// The package, manifest, metadata or payload breaks a format rule. The
    /// message says which.
    Invalid(String),
    /// A value differs from the one the manifest or metadata records.
    Mismatch {
        /// What differs, for example `content hash`.
        what: &'static str,
        /// The recorded value.
        expected: String,
        /// The value found.
        actual: String,
    },
}

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid(message.into()))
    }

    fn new(kind: ErrorKind) -> Self {
        Self {
            package: None,
            path: None,
            kind,
        }
    }

    /// Attaches the path the error is about, unless one is attached already.
    pub(crate) fn at(mut self, path: &str) -> Self {
        self.path.get_or_insert_with(|| path.to_owned());
        self
    }

    /// Attaches the package the error is about, unless one is attached
    /// already.
    pub(crate) fn in_package(mut self, package: impl fmt::Display) -> Self {
        self.package.get_or_insert_with(|| package.to_string());
        self
    }

    /// The package, as `<name>-<version>-r<rev>[+<variant>]`, when the error
    /// happened after its identity was known.
    pub fn package(&self) -> Option<&str> {
        self.package.as_deref()
    }

    /// The path inside the package or payload, when the error concerns one.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::new(ErrorKind::Io(e))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("package")?;
        if let Some(package) = &self.package {
            write!(f, " {package}")?;
        }
        match &self.kind {
            ErrorKind::Io(e) => write!(f, ": {e}")?,
            ErrorKind::Invalid(message) => write!(f, " is invalid: {message}")?,
            ErrorKind::Mismatch { what, .. } => write!(f, " is corrupt: {what} differs")?,
        }
        if let Some(path) = &self.path {
            write!(f, "\n  path:     {path}")?;
        }
        if let ErrorKind::Mismatch {
            expected, actual, ..
        } = &self.kind
        {
            write!(f, "\n  expected: {expected}\n  actual:   {actual}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io(e) => Some(e),
            _ => None,
        }
    }
}
