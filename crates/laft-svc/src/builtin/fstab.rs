//! Reading `/etc/fstab` and the mount table.

use std::collections::BTreeSet;

/// One filesystem line of an fstab file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The device, such as `UUID=4A2B-91C0`, `/dev/sda1` or `tmpfs`.
    pub device: String,
    /// The mountpoint, or `none` for swap.
    pub mountpoint: String,
    /// The filesystem type, such as `ext4` or `swap`.
    pub fstype: String,
    /// The mount options, `defaults` when the line gives none.
    pub options: String,
    /// The fsck pass: 0 for no check, 1 for the root filesystem and 2 for
    /// the others.
    pub pass: u32,
}

impl Entry {
    /// Whether the line names swap rather than a filesystem.
    pub fn is_swap(&self) -> bool {
        self.fstype == "swap"
    }

    /// Whether `mount -a` skips the entry because of `noauto`.
    pub fn is_noauto(&self) -> bool {
        self.options.split(',').any(|o| o == "noauto")
    }
}

/// Parses the text of an fstab file. Blank lines and lines that start with
/// `#` are skipped, and the octal escapes `\040`, `\011`, `\012` and
/// `\134` in the first two fields are decoded. The dump and pass fields
/// may be missing and default to 0.
///
/// # Errors
///
/// Returns the 1-based number of a line with fewer than three fields or a
/// pass that is not a number, with why.
pub fn parse_fstab(text: &str) -> Result<Vec<Entry>, (usize, String)> {
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [device, mountpoint, fstype, rest @ ..] = fields.as_slice() else {
            return Err((index + 1, "a line has at least three fields".into()));
        };
        let pass = match rest.get(2) {
            None => 0,
            Some(pass) => pass
                .parse()
                .map_err(|_| (index + 1, format!("the pass {pass:?} is not a number")))?,
        };
        entries.push(Entry {
            device: unescape(device),
            mountpoint: unescape(mountpoint),
            fstype: (*fstype).to_owned(),
            options: rest.first().map_or("defaults", |o| *o).to_owned(),
            pass,
        });
    }
    Ok(entries)
}

/// The mountpoints of a `/proc/self/mountinfo` text, decoded.
pub fn mountpoints(mountinfo: &str) -> BTreeSet<String> {
    mountinfo
        .lines()
        .filter_map(|line| line.split(' ').nth(4))
        .map(unescape)
        .collect()
}

/// Decodes the octal escapes of fstab and mountinfo fields, such as
/// `\040` for a space. Anything else is kept as it is.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|digits| bytes[i] == b'\\' && digits.iter().all(|d| (b'0'..=b'7').contains(d)))
            .and_then(|d| {
                let value = d.iter().fold(0u32, |n, d| n * 8 + u32::from(d - b'0'));
                u8::try_from(value).ok()
            });
        match octal {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Encodes a field for fstab: a space, a tab, a newline and a backslash
/// become their octal escapes.
pub fn escape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for c in field.chars() {
        match c {
            ' ' => out.push_str("\\040"),
            '\t' => out.push_str("\\011"),
            '\n' => out.push_str("\\012"),
            '\\' => out.push_str("\\134"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
#[path = "fstab_tests.rs"]
mod tests;
