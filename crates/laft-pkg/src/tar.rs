//! The subset of the pax tar format that package files use.
//!
//! Members are POSIX ustar headers. A pax extended header precedes a member
//! only when its path or link target exceeds 100 bytes or its size exceeds
//! the 11 octal digits of the ustar size field, and it carries only the
//! `path`, `linkpath` and `size` records.

pub(crate) const BLOCK: usize = 512;

/// The longest pax extended header this crate reads. Two paths of 4095 bytes
/// plus a size fit easily.
pub(crate) const MAX_PAX_LEN: u64 = 16 * 1024;

const NAME_LEN: usize = 100;
const MAX_USTAR_SIZE: u64 = 0o777_7777_7777;
const PAX_NAME: &[u8] = b"PaxHeader";

/// The member types package files use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    HardLink,
    Symlink,
    Dir,
    PaxHeader,
}

impl Kind {
    fn flag(self) -> u8 {
        match self {
            Self::File => b'0',
            Self::HardLink => b'1',
            Self::Symlink => b'2',
            Self::Dir => b'5',
            Self::PaxHeader => b'x',
        }
    }

    fn from_flag(flag: u8) -> Option<Self> {
        match flag {
            b'0' => Some(Self::File),
            b'1' => Some(Self::HardLink),
            b'2' => Some(Self::Symlink),
            b'5' => Some(Self::Dir),
            b'x' => Some(Self::PaxHeader),
            _ => None,
        }
    }
}

/// One member header. Paths and link targets are bytes, because the reader
/// sees them before it can check that they are UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) kind: Kind,
    pub(crate) path: Vec<u8>,
    pub(crate) link: Vec<u8>,
    pub(crate) mode: u32,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) size: u64,
}

/// The number of zero bytes that pad `size` bytes of content to a block.
pub(crate) fn padding(size: u64) -> usize {
    (BLOCK - (size % BLOCK as u64) as usize) % BLOCK
}

/// Appends the blocks that introduce `header`: a pax extended header when
/// the ustar fields cannot hold a value, then the ustar header.
///
/// Every field that is not part of `header` is fixed: mtime, device numbers
/// and the owner and group names are zero or empty. Ids and modes must fit
/// their 7 octal digits.
pub(crate) fn encode(header: &Header, out: &mut Vec<u8>) {
    let mut records = Vec::new();
    if header.path.len() > NAME_LEN {
        pax_record(&mut records, "path", &header.path);
    }
    if header.link.len() > NAME_LEN {
        pax_record(&mut records, "linkpath", &header.link);
    }
    if header.size > MAX_USTAR_SIZE {
        pax_record(&mut records, "size", header.size.to_string().as_bytes());
    }
    if !records.is_empty() {
        out.extend_from_slice(&ustar(&Header {
            kind: Kind::PaxHeader,
            path: PAX_NAME.to_vec(),
            link: Vec::new(),
            mode: 0,
            uid: 0,
            gid: 0,
            size: records.len() as u64,
        }));
        out.extend_from_slice(&records);
        out.resize(out.len() + padding(records.len() as u64), 0);
    }
    out.extend_from_slice(&ustar(header));
}

fn ustar(header: &Header) -> [u8; BLOCK] {
    let mut block = [0u8; BLOCK];
    let truncate = |bytes: &[u8]| bytes.len().min(NAME_LEN);
    let path_len = truncate(&header.path);
    block[..path_len].copy_from_slice(&header.path[..path_len]);
    write_octal(&mut block[100..108], header.mode.into());
    write_octal(&mut block[108..116], header.uid.into());
    write_octal(&mut block[116..124], header.gid.into());
    let size = if header.size > MAX_USTAR_SIZE {
        0
    } else {
        header.size
    };
    write_octal(&mut block[124..136], size);
    write_octal(&mut block[136..148], 0);
    block[156] = header.kind.flag();
    let link_len = truncate(&header.link);
    block[157..157 + link_len].copy_from_slice(&header.link[..link_len]);
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    write_octal(&mut block[329..337], 0);
    write_octal(&mut block[337..345], 0);
    let sum = checksum(&block);
    block[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
    block[154] = 0;
    block[155] = b' ';
    block
}

/// The header checksum: the sum of all bytes, with the checksum field read as
/// spaces.
fn checksum(block: &[u8; BLOCK]) -> u32 {
    block
        .iter()
        .enumerate()
        .map(|(i, &b)| u32::from(if (148..156).contains(&i) { b' ' } else { b }))
        .sum()
}

/// Writes `value` as zero-padded octal digits followed by a NUL.
fn write_octal(field: &mut [u8], value: u64) {
    let digits = format!("{value:0width$o}", width = field.len() - 1);
    field[..digits.len()].copy_from_slice(digits.as_bytes());
    field[digits.len()] = 0;
}

pub(crate) fn pax_record(out: &mut Vec<u8>, key: &str, value: &[u8]) {
    // The length prefix counts its own digits, so grow it until it is stable.
    let base = key.len() + value.len() + 3;
    let mut len = base + 1;
    while len != base + len.to_string().len() {
        len = base + len.to_string().len();
    }
    out.extend_from_slice(format!("{len} {key}=").as_bytes());
    out.extend_from_slice(value);
    out.push(b'\n');
}

/// Parses a ustar header block. Pax extended headers come back as
/// [`Kind::PaxHeader`], with their data still unread.
pub(crate) fn decode(block: &[u8; BLOCK]) -> Result<Header, String> {
    let stored = parse_octal(&block[148..156]).ok_or("the header checksum is malformed")?;
    if stored != u64::from(checksum(block)) {
        return Err("the header checksum is wrong".into());
    }
    if &block[257..263] != b"ustar\0" || &block[263..265] != b"00" {
        return Err("the header is not a POSIX ustar header".into());
    }
    let kind = Kind::from_flag(block[156]).ok_or_else(|| {
        format!(
            "member type {:?} is not allowed, only files, hard links, symlinks and directories are",
            char::from(block[156])
        )
    })?;
    let field = |range: std::ops::Range<usize>, name: &str| {
        parse_octal(&block[range]).ok_or_else(|| format!("the {name} field is malformed"))
    };
    let mode = field(100..108, "mode")?;
    if mode > 0o7777 {
        return Err(format!(
            "mode {mode:#o} has bits other than permission bits"
        ));
    }
    let id = |value: u64| u32::try_from(value).map_err(|_| "an id does not fit in 32 bits");
    let mut path = Vec::new();
    let prefix = until_nul(&block[345..500]);
    if !prefix.is_empty() {
        path.extend_from_slice(prefix);
        path.push(b'/');
    }
    path.extend_from_slice(until_nul(&block[..NAME_LEN]));
    Ok(Header {
        kind,
        path,
        link: until_nul(&block[157..257]).to_vec(),
        mode: mode as u32,
        uid: id(field(108..116, "uid")?)?,
        gid: id(field(116..124, "gid")?)?,
        size: field(124..136, "size")?,
    })
}

fn until_nul(field: &[u8]) -> &[u8] {
    field
        .iter()
        .position(|&b| b == 0)
        .map_or(field, |end| &field[..end])
}

/// Parses an octal field: optional leading spaces, at least one octal digit,
/// then only NULs and spaces. The base-256 extension is not accepted.
fn parse_octal(field: &[u8]) -> Option<u64> {
    let start = field.iter().position(|&b| b != b' ')?;
    let digits = field[start..]
        .iter()
        .take_while(|b| (b'0'..=b'7').contains(b))
        .count();
    let rest = &field[start + digits..];
    if digits == 0 || !rest.iter().all(|&b| b == 0 || b == b' ') {
        return None;
    }
    field[start..start + digits]
        .iter()
        .try_fold(0u64, |acc, &d| {
            acc.checked_mul(8)?.checked_add(u64::from(d - b'0'))
        })
}

/// The values a pax extended header overrides.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Pax {
    pub(crate) path: Option<Vec<u8>>,
    pub(crate) link: Option<Vec<u8>>,
    pub(crate) size: Option<u64>,
}

/// Parses the records of a pax extended header. Only `path`, `linkpath` and
/// `size` are allowed, each at most once, so extended attributes and every
/// other extension are rejected.
pub(crate) fn parse_pax(mut data: &[u8]) -> Result<Pax, String> {
    let malformed = || String::from("a pax extended header record is malformed");
    let mut pax = Pax::default();
    while !data.is_empty() {
        let space = data.iter().position(|&b| b == b' ').ok_or_else(malformed)?;
        let len = parse_decimal(&data[..space]).ok_or_else(malformed)?;
        let len = usize::try_from(len).map_err(|_| malformed())?;
        if len <= space + 1 || len > data.len() || data[len - 1] != b'\n' {
            return Err(malformed());
        }
        let record = &data[space + 1..len - 1];
        data = &data[len..];
        let eq = record
            .iter()
            .position(|&b| b == b'=')
            .ok_or_else(malformed)?;
        let (key, value) = (&record[..eq], &record[eq + 1..]);
        let duplicate = match key {
            b"path" => pax.path.replace(value.to_vec()).is_some(),
            b"linkpath" => pax.link.replace(value.to_vec()).is_some(),
            b"size" => pax
                .size
                .replace(parse_decimal(value).ok_or_else(malformed)?)
                .is_some(),
            _ => {
                return Err(format!(
                    "pax record {:?} is not allowed, only path, linkpath and size are",
                    String::from_utf8_lossy(key)
                ));
            }
        };
        if duplicate {
            return Err("a pax extended header repeats a record".into());
        }
    }
    Ok(pax)
}

fn parse_decimal(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u64, |acc, &d| {
        if !d.is_ascii_digit() {
            return None;
        }
        acc.checked_mul(10)?.checked_add(u64::from(d - b'0'))
    })
}

#[cfg(test)]
#[path = "tar_tests.rs"]
mod tests;
