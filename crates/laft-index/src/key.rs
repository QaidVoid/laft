use std::fmt;
use std::str::FromStr;

use crate::Error;

/// The id of a minisign key, which every signature names.
///
/// The text form is the 16 lowercase hexadecimal digits of the 8 id bytes
/// read as a little-endian integer. It is how minisign prints key ids, apart
/// from the case.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId([u8; 8]);

impl FromStr for KeyId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid = s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        let n = u64::from_str_radix(s, 16)
            .ok()
            .filter(|_| valid)
            .ok_or_else(|| {
                Error::invalid(
                    "key id",
                    format!("{s:?} is not 16 lowercase hexadecimal digits"),
                )
            })?;
        Ok(Self(n.to_le_bytes()))
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", u64::from_le_bytes(self.0))
    }
}

impl fmt::Debug for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A minisign Ed25519 public key.
///
/// The text form is the base64 line of a minisign `.pub` file, which is what
/// repository configurations and `keys.json` carry.
///
/// ```
/// use laft_index::PublicKey;
///
/// let key: PublicKey = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3".parse()?;
/// assert_eq!(key.id().to_string(), "e7620f1842b4e81f");
/// # Ok::<(), laft_index::Error>(())
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey {
    text: String,
    id: KeyId,
    key: minisign_verify::PublicKey,
}

impl PublicKey {
    /// The key's id.
    pub fn id(&self) -> KeyId {
        self.id
    }

    /// Whether `signature` is a valid signature of `data` by this key,
    /// including the global signature over its trusted comment.
    ///
    /// Only prehashed signatures, the kind minisign makes by default, are
    /// accepted.
    pub fn verifies(&self, data: &[u8], signature: &Signature) -> bool {
        signature.key_id == self.id && self.key.verify(data, &signature.inner, false).is_ok()
    }
}

impl FromStr for PublicKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason| Error::invalid("public key", format!("{s:?} {reason}"));
        let bytes = decode_base64(s).ok_or_else(|| err("is not base64"))?;
        if bytes.len() != 42 || bytes[..2] != *b"Ed" {
            return Err(err("is not a minisign Ed25519 public key"));
        }
        let key = minisign_verify::PublicKey::from_base64(s)
            .map_err(|_| err("is not a minisign Ed25519 public key"))?;
        Ok(Self {
            text: s.to_owned(),
            id: key_id(&bytes),
            key,
        })
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.id)
    }
}

/// A minisign signature file (`.minisig`), as downloaded next to the file it
/// signs.
///
/// Decoding only checks the format. [`PublicKey::verifies`] checks the
/// signature itself.
#[derive(Clone)]
pub struct Signature {
    inner: minisign_verify::Signature,
    key_id: KeyId,
}

impl Signature {
    /// Decodes the bytes of a signature file.
    ///
    /// # Errors
    ///
    /// Fails when `bytes` are not a minisign signature with an Ed25519
    /// signature and a trusted comment.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let err = |reason| Error::invalid("signature", reason);
        let text = std::str::from_utf8(bytes).map_err(|_| err("the file is not UTF-8 text"))?;
        let inner = minisign_verify::Signature::decode(text)
            .map_err(|_| err("the file is not a minisign signature"))?;
        let line = text
            .lines()
            .nth(1)
            .and_then(decode_base64)
            .filter(|b| b.len() == 74)
            .ok_or_else(|| err("the file is not a minisign signature"))?;
        Ok(Self {
            inner,
            key_id: key_id(&line),
        })
    }

    /// The id of the key that claims to have made the signature.
    pub fn key_id(&self) -> KeyId {
        self.key_id
    }

    /// The trusted comment, which the signature covers too.
    pub fn trusted_comment(&self) -> &str {
        self.inner.trusted_comment()
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signature")
            .field("key_id", &self.key_id)
            .field("trusted_comment", &self.trusted_comment())
            .finish()
    }
}

/// The key id stored after the two algorithm bytes of a minisign key or
/// signature.
fn key_id(bytes: &[u8]) -> KeyId {
    let mut id = [0; 8];
    id.copy_from_slice(&bytes[2..10]);
    KeyId(id)
}

/// Decodes padded standard base64 (RFC 4648).
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if s.is_empty() || !s.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| -> Option<u32> {
        Some(u32::from(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (chunks, _) = s.as_chunks::<4>();
    for (i, chunk) in chunks.iter().enumerate() {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && i + 1 < chunks.len()) {
            return None;
        }
        let mut n = 0;
        for &c in &chunk[..4 - pad] {
            n = n << 6 | value(c)?;
        }
        n <<= 6 * pad;
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    Some(out)
}

macro_rules! serde_as_text {
    ($($ty:ty),*) => {$(
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }
    )*};
}

serde_as_text!(KeyId, PublicKey);

#[cfg(test)]
#[path = "key_tests.rs"]
mod tests;
