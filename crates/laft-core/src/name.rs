use std::fmt;
use std::str::FromStr;

use crate::ParseError;

/// A package name.
///
/// A name is 1 to 64 bytes of lowercase ASCII letters, digits, `-` and `_`. A
/// `.` is allowed only when a digit follows it, so `openssl1.1` and
/// `python3.12` are names while `foo.bar` is not. Names never contain `+`,
/// `@` or `:`, which keeps package references unambiguous.
///
/// ```
/// use laft_core::Name;
///
/// let name: Name = "openssl1.1".parse()?;
/// assert_eq!(name.as_str(), "openssl1.1");
/// assert!("Foo".parse::<Name>().is_err());
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

/// The longest package name, in bytes.
const MAX_LEN: usize = 64;

impl Name {
    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Name {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason| ParseError::new("package name", s, reason);
        if s.is_empty() {
            return Err(err("a name cannot be empty"));
        }
        if s.len() > MAX_LEN {
            return Err(err("a name is at most 64 bytes long"));
        }
        let bytes = s.as_bytes();
        if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
            return Err(err("a name starts with a lowercase letter or a digit"));
        }
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {}
                b'.' if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) => {}
                b'.' => return Err(err("a `.` must be followed by a digit")),
                b'A'..=b'Z' => return Err(err("uppercase letters are not allowed")),
                _ => {
                    return Err(err(
                        "only lowercase letters, digits, `-`, `_` and `.` are allowed",
                    ));
                }
            }
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[path = "name_tests.rs"]
mod tests;
