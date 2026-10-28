//! Lowercase base32 without padding, using the RFC 4648 alphabet.

const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Number of characters needed to encode `bytes` bytes.
pub(crate) const fn encoded_len(bytes: usize) -> usize {
    (bytes * 8).div_ceil(5)
}

pub(crate) fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(encoded_len(data.len()));
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Decodes exactly `N` bytes. Returns `None` for a wrong length, a character
/// outside the alphabet, or nonzero padding bits, so every byte string has
/// exactly one accepted encoding.
pub(crate) fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != encoded_len(N) {
        return None;
    }
    let mut out = [0u8; N];
    let mut buffer: u32 = 0;
    let mut bits = 0;
    let mut written = 0;
    for c in text.bytes() {
        let value = ALPHABET.iter().position(|&a| a == c)? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out[written] = (buffer >> bits) as u8;
            written += 1;
        }
    }
    (buffer & ((1 << bits) - 1) == 0).then_some(out)
}

#[cfg(test)]
#[path = "base32_tests.rs"]
mod tests;
