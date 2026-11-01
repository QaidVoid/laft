//! Package option values.
//!
//! A recipe MAY declare typed options, and the build of the package
//! records the value every option had. A value is either a [`Bool`] or a
//! choice among named tags. Builds with different values are different
//! packages with different store ids.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ParseError;

/// The longest option name or tag, in characters.
pub const MAX_NAME_CHARS: usize = 32;

/// The most options one package declares.
pub const MAX_OPTIONS: usize = 32;

/// The value of one package option: a bool or a tag of the sum type the
/// option declares.
///
/// The stored form of a value inside metadata and build keys is its text:
/// `true`, `false` or the bare tag.
///
/// ```
/// use laft_core::OptionValue;
///
/// let value: OptionValue = "false".parse()?;
/// assert_eq!(value.to_string(), "false");
/// let tag: OptionValue = "amd".parse()?;
/// assert!(matches!(tag, OptionValue::Tag(tag) if tag == "amd"));
/// assert!("Vulkan".parse::<OptionValue>().is_err());
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionValue {
    /// A `Bool` option.
    Bool(bool),
    /// The bare tag of the sum type the option declares.
    Tag(String),
}

/// Whether `name` is written like an option name or tag: a lowercase
/// letter followed by lowercase letters, digits and `_`, at most
/// [`MAX_NAME_CHARS`] long.
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    name.chars().count() <= MAX_NAME_CHARS
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl OptionValue {
    /// The `true` value.
    pub const TRUE: Self = Self::Bool(true);

    /// The `false` value.
    pub const FALSE: Self = Self::Bool(false);

    /// The tag `name`.
    pub fn tag(name: &str) -> Self {
        Self::Tag(name.to_owned())
    }

    /// The value as a `Bool`, or `None` for a tag.
    pub fn as_bool(self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(b),
            Self::Tag(_) => None,
        }
    }

    /// The tag, or `None` for a `Bool`.
    pub fn as_tag(&self) -> Option<&str> {
        match self {
            Self::Bool(_) => None,
            Self::Tag(tag) => Some(tag),
        }
    }
}

impl FromStr for OptionValue {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "true" => return Ok(Self::Bool(true)),
            "false" => return Ok(Self::Bool(false)),
            _ => {}
        }
        if is_valid_name(s) {
            Ok(Self::Tag(s.to_owned()))
        } else {
            Err(ParseError::new(
                "option value",
                s,
                "expected `true`, `false` or a tag of lowercase letters, digits and `_`",
            ))
        }
    }
}

impl fmt::Display for OptionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(true) => f.write_str("true"),
            Self::Bool(false) => f.write_str("false"),
            Self::Tag(tag) => f.write_str(tag),
        }
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
