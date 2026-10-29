use std::mem::size_of;

use object::Endianness;
use object::elf;
use object::endian::Endian as _;
use object::read::elf::{Dyn, FileHeader, SectionHeader};

use crate::Error;

/// Rewrites the library search paths in the dynamic section of the ELF file
/// `data` in place, and returns whether anything changed.
///
/// - A `DT_RPATH` or `DT_RUNPATH` entry whose whole value is `/usr/lib` is
///   removed, because the dynamic loader searches that directory anyway.
/// - A remaining `DT_RPATH` becomes a `DT_RUNPATH`, which the loader searches
///   after `LD_LIBRARY_PATH` and which does not apply to the libraries the
///   object loads. When the object has a `DT_RUNPATH` already, the
///   `DT_RPATH` is removed instead, because the loader ignores it then.
///
/// Removing an entry moves the later entries of the dynamic array up by one
/// and zeroes the freed entry, which is another `DT_NULL`. The section keeps
/// its size, every string stays where it is, and nothing else in the file
/// changes. A file that is not ELF, or has no dynamic section, is left
/// alone.
///
/// # Errors
///
/// Fails when the file starts with the ELF magic but its header, its
/// section table, its dynamic section or a search path string cannot be
/// read. `data` is unchanged then.
///
/// # Examples
///
/// ```
/// let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/libversioned.so");
/// let mut data = std::fs::read(path)?;
/// assert!(laft_elf::normalize_search_paths(&mut data)?);
/// let library = laft_elf::inspect(&data)?.expect("the file is ELF");
/// assert_eq!(library.runpath, ["/opt/legacy/lib"]);
/// assert!(library.rpath.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn normalize_search_paths(data: &mut [u8]) -> Result<bool, Error> {
    if !data.starts_with(&elf::ELFMAG) {
        return Ok(false);
    }
    let class = data.get(4).copied().map(elf::FileClass);
    let edits = if class == Some(elf::ELFCLASS32) {
        edits::<elf::FileHeader32<Endianness>>(data)?
    } else if class == Some(elf::ELFCLASS64) {
        edits::<elf::FileHeader64<Endianness>>(data)?
    } else {
        return Err(Error::new("unknown ELF class"));
    };
    let Some(edits) = edits else {
        return Ok(false);
    };
    if edits.keep.len() == edits.live && edits.retag.is_empty() {
        return Ok(false);
    }
    let start = edits.offset;
    let end = start + edits.region * edits.size;
    let old = data[start..end].to_vec();
    let mut new = Vec::with_capacity(old.len());
    for &index in &edits.keep {
        let record = &old[index * edits.size..(index + 1) * edits.size];
        let tag_len = edits.runpath_tag.len();
        if edits.retag.contains(&index) {
            new.extend_from_slice(&edits.runpath_tag);
            new.extend_from_slice(&record[tag_len..]);
        } else {
            new.extend_from_slice(record);
        }
    }
    new.resize(old.len(), 0);
    data[start..end].copy_from_slice(&new);
    Ok(true)
}

/// The changes to one dynamic array, computed before anything is written.
struct Edits {
    /// The file offset of the dynamic array.
    offset: usize,
    /// The size of one entry.
    size: usize,
    /// The number of entries before the first `DT_NULL`.
    live: usize,
    /// The number of entries that are rewritten: the live ones and the
    /// terminating `DT_NULL`, when there is one.
    region: usize,
    /// The live entries that stay, in order.
    keep: Vec<usize>,
    /// The kept entries that change from `DT_RPATH` to `DT_RUNPATH`.
    retag: Vec<usize>,
    /// The tag field of a `DT_RUNPATH` entry, in the file's byte order.
    runpath_tag: Vec<u8>,
}

fn edits<H: FileHeader<Endian = Endianness>>(data: &[u8]) -> Result<Option<Edits>, Error> {
    let header = H::parse(data).map_err(Error::object)?;
    let endian = header.endian().map_err(Error::object)?;
    let sections = header.sections(endian, data).map_err(Error::object)?;
    for section in sections.iter() {
        let Some((entries, link)) = section.dynamic(endian, data).map_err(Error::object)? else {
            continue;
        };
        let strings = sections
            .strings(endian, data, link)
            .map_err(Error::object)?;
        let live = entries
            .iter()
            .position(|e| e.tag(endian) == elf::DT_NULL)
            .unwrap_or(entries.len());
        let is_search = |e: &H::Dyn| matches!(e.tag(endian), elf::DT_RPATH | elf::DT_RUNPATH);
        let mut keep = Vec::with_capacity(live);
        for (index, entry) in entries[..live].iter().enumerate() {
            if is_search(entry)
                && entry.string(endian, strings).map_err(Error::object)? == b"/usr/lib"
            {
                continue;
            }
            keep.push(index);
        }
        let has_runpath = keep
            .iter()
            .any(|&i| entries[i].tag(endian) == elf::DT_RUNPATH);
        let rpath = |i: &usize| entries[*i].tag(endian) == elf::DT_RPATH;
        if has_runpath {
            keep.retain(|i| !rpath(i));
        }
        let retag = keep.iter().copied().filter(rpath).collect();
        let size = size_of::<H::Dyn>();
        let runpath_tag = if size == 8 {
            endian.write_i32(elf::DT_RUNPATH.0 as i32).to_vec()
        } else {
            endian.write_i64(elf::DT_RUNPATH.0).to_vec()
        };
        let offset: u64 = section.sh_offset(endian).into();
        let offset = usize::try_from(offset)
            .map_err(|_| Error::new("the dynamic section offset is out of range"))?;
        return Ok(Some(Edits {
            offset,
            size,
            live,
            region: (live + 1).min(entries.len()),
            keep,
            retag,
            runpath_tag,
        }));
    }
    Ok(None)
}

#[cfg(test)]
#[path = "rewrite_tests.rs"]
mod tests;
