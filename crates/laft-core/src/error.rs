use std::fmt;

/// An identifier or text form that failed to parse.
///
/// The message names what was being parsed, quotes the input and says what is
/// wrong with it, for example `invalid package name "gtk+3": only lowercase
/// letters, digits, `-`, `_` and `.` are allowed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    what: &'static str,
    input: String,
    reason: &'static str,
}

impl ParseError {
    /// The error for `input`, which was parsed as a `what`, for example
    /// `architecture`, and rejected because of `reason`.
    pub fn new(what: &'static str, input: &str, reason: &'static str) -> Self {
        Self {
            what,
            input: input.to_owned(),
            reason,
        }
    }

    /// The rejected input.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Why the input was rejected.
    pub fn reason(&self) -> &str {
        self.reason
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} {:?}: {}", self.what, self.input, self.reason)
    }
}

impl std::error::Error for ParseError {}
