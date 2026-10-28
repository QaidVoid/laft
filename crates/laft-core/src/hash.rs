use std::fmt;
use std::str::FromStr;

use crate::{ParseError, base32};

/// A BLAKE3 hash, the hash Laft uses for everything it produces itself.
///
/// The text form is `b3:` followed by the 52 character lowercase base32
/// encoding of the 32 byte hash.
///
/// ```
/// use laft_core::Blake3Hash;
///
/// let hash = Blake3Hash::of(b"hello");
/// assert!(hash.to_string().starts_with("b3:"));
/// assert_eq!(hash.to_string().parse::<Blake3Hash>()?, hash);
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Blake3Hash([u8; 32]);

impl Blake3Hash {
    /// Hashes `data`.
    pub fn of(data: &[u8]) -> Self {
        Self(*blake3::hash(data).as_bytes())
    }

    /// Wraps a hash computed elsewhere, for example by a streaming hasher.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw hash.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl FromStr for Blake3Hash {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let encoded = s
            .strip_prefix("b3:")
            .ok_or_else(|| ParseError::new("BLAKE3 hash", s, "expected the `b3:` prefix"))?;
        base32::decode(encoded).map(Self).ok_or_else(|| {
            ParseError::new(
                "BLAKE3 hash",
                s,
                "expected 52 lowercase base32 characters after `b3:`",
            )
        })
    }
}

impl fmt::Display for Blake3Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b3:{}", base32::encode(&self.0))
    }
}

impl fmt::Debug for Blake3Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A SHA-256 hash, used only to verify upstream source archives, because
/// upstream projects publish SHA-256 checksums.
///
/// The text form is `sha256:` followed by 64 lowercase hexadecimal digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Hash([u8; 32]);

impl Sha256Hash {
    /// Wraps a hash computed elsewhere.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw hash.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl FromStr for Sha256Hash {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason| ParseError::new("SHA-256 hash", s, reason);
        let hex = s
            .strip_prefix("sha256:")
            .ok_or_else(|| err("expected the `sha256:` prefix"))?;
        if hex.len() != 64 {
            return Err(err("expected 64 hexadecimal digits after `sha256:`"));
        }
        let mut bytes = [0u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            let digit = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            let (Some(high), Some(low)) = (digit(pair[0]), digit(pair[1])) else {
                return Err(err("hexadecimal digits must be lowercase `0-9a-f`"));
            };
            *byte = high << 4 | low;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
#[path = "hash_tests.rs"]
mod tests;
