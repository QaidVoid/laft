//! Filesystem and LUKS signatures read directly from a device's first bytes.

/// How many bytes from the start of a device [`probe`] needs to recognize
/// every supported signature. The btrfs superblock lies at 64 KiB.
pub(crate) const PROBE_LEN: usize = 0x11000;

/// What a device holds, as found by [`probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Probe {
    /// The filesystem or container type, for example `ext4` or `luks2`.
    pub(crate) kind: &'static str,
    /// The UUID in its usual text form.
    pub(crate) uuid: String,
}

/// Recognizes the signature at the start of a device from its first bytes,
/// `head`, which may be shorter than [`PROBE_LEN`] for small devices.
///
/// LUKS1 and LUKS2, ext2, ext3, ext4, xfs, btrfs, f2fs and FAT are
/// recognized. Anything else gives `None`.
pub(crate) fn probe(head: &[u8]) -> Option<Probe> {
    luks(head)
        .or_else(|| xfs(head))
        .or_else(|| ext(head))
        .or_else(|| f2fs(head))
        .or_else(|| btrfs(head))
        .or_else(|| fat(head))
}

fn luks(head: &[u8]) -> Option<Probe> {
    if bytes(head, 0, 6)? != b"LUKS\xba\xbe" {
        return None;
    }
    let kind = match u16::from_be_bytes(bytes(head, 6, 2)?.try_into().ok()?) {
        1 => "luks1",
        2 => "luks2",
        _ => return None,
    };
    let field = bytes(head, 168, 40)?;
    let len = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    let uuid = std::str::from_utf8(&field[..len]).ok()?.to_owned();
    Some(Probe { kind, uuid })
}

fn xfs(head: &[u8]) -> Option<Probe> {
    signed(head, 0, b"XFSB", "xfs", 32)
}

const EXT_SUPER: usize = 1024;
const EXT_COMPAT_HAS_JOURNAL: u32 = 0x4;
const EXT_INCOMPAT_EXT4: u32 = 0x40 | 0x80 | 0x200;

fn ext(head: &[u8]) -> Option<Probe> {
    if bytes(head, EXT_SUPER + 0x38, 2)? != 0xEF53u16.to_le_bytes() {
        return None;
    }
    let compat = le32(head, EXT_SUPER + 0x5C)?;
    let incompat = le32(head, EXT_SUPER + 0x60)?;
    let kind = if incompat & EXT_INCOMPAT_EXT4 != 0 {
        "ext4"
    } else if compat & EXT_COMPAT_HAS_JOURNAL != 0 {
        "ext3"
    } else {
        "ext2"
    };
    Some(Probe {
        kind,
        uuid: standard_uuid(bytes(head, EXT_SUPER + 0x68, 16)?),
    })
}

fn f2fs(head: &[u8]) -> Option<Probe> {
    signed(
        head,
        1024,
        &0xF2F5_2010u32.to_le_bytes(),
        "f2fs",
        1024 + 0x6C,
    )
}

fn btrfs(head: &[u8]) -> Option<Probe> {
    signed(head, 0x10040, b"_BHRfS_M", "btrfs", 0x10020)
}

/// A `kind` filesystem when `magic` is at `offset`, with its 16 byte UUID
/// at `uuid_at`.
fn signed(
    head: &[u8],
    offset: usize,
    magic: &[u8],
    kind: &'static str,
    uuid_at: usize,
) -> Option<Probe> {
    if bytes(head, offset, magic.len())? != magic {
        return None;
    }
    Some(Probe {
        kind,
        uuid: standard_uuid(bytes(head, uuid_at, 16)?),
    })
}

fn fat(head: &[u8]) -> Option<Probe> {
    let serial = if bytes(head, 0x52, 8) == Some(b"FAT32   ") {
        le32(head, 0x43)?
    } else if bytes(head, 0x36, 4) == Some(b"FAT1") {
        le32(head, 0x27)?
    } else {
        return None;
    };
    Some(Probe {
        kind: "vfat",
        uuid: format!("{:04X}-{:04X}", serial >> 16, serial & 0xFFFF),
    })
}

fn bytes(head: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    head.get(offset..offset.checked_add(len)?)
}

fn le32(head: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes(head, offset, 4)?.try_into().ok()?))
}

/// The 8-4-4-4-12 lowercase hex form of a 16 byte UUID.
fn standard_uuid(raw: &[u8]) -> String {
    let mut out = String::with_capacity(36);
    for (i, b) in raw.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
