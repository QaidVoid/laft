//! The records that decide who may enter a view and what is bound into it,
//! the checks on the files that hold them, and the links that lead to them.
//!
//! Only the fields this program needs are read. Unknown fields are ignored,
//! so the records may carry more than is declared here.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The largest `view.json` or `privileged.json` that is read.
///
/// `view.json` lists every package with every reason it is in the view,
/// and a library has one reason per package that depends on it. A view of
/// 20,000 packages with ten reasons each takes about 25 MiB, so the limit
/// leaves room for views far larger than any real one while still bounding
/// what is read as root.
pub(crate) const MAX_RECORD: u64 = 64 << 20;

/// The format version of both records.
const FORMAT: u32 = 1;

/// The facts about an opened record file that decide whether it is trusted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileFacts {
    /// Whether it is a regular file.
    pub(crate) regular: bool,
    /// The owning uid.
    pub(crate) uid: u32,
    /// The permission bits.
    pub(crate) mode: u32,
    /// The size in bytes.
    pub(crate) size: u64,
}

/// Checks that a record file is a regular file owned by root that no group
/// or other user can write, and at most `max` bytes long.
///
/// # Errors
///
/// Returns the reason the file is not trusted.
pub(crate) fn check_file(facts: FileFacts, max: u64) -> Result<(), &'static str> {
    if !facts.regular {
        return Err("not a regular file");
    }
    if facts.uid != 0 {
        return Err("not owned by root");
    }
    if facts.mode & 0o022 != 0 {
        return Err("writable by group or others");
    }
    if facts.size > max {
        return Err("too large");
    }
    Ok(())
}

/// The fields of `view.json` that authorize entering a view version.
#[derive(Debug, Deserialize)]
pub(crate) struct ViewRecord {
    version: u32,
    name: String,
    view_gen: u64,
    /// The uids allowed to enter.
    pub(crate) allowed_uids: Vec<u32>,
    /// The gids whose members may enter.
    pub(crate) allowed_gids: Vec<u32>,
}

/// Parses `view.json` of version `version` of view `name`.
///
/// # Errors
///
/// Fails when the bytes are not a view record of the supported format, or
/// when the record names another view or another version.
pub(crate) fn parse_view(bytes: &[u8], name: &str, version: u64) -> Result<ViewRecord, String> {
    let record: ViewRecord = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if record.version != FORMAT {
        return Err(format!("unsupported format version {}", record.version));
    }
    if record.name != name || record.view_gen != version {
        return Err(format!(
            "it describes view {:?} version {}",
            record.name, record.view_gen
        ));
    }
    Ok(record)
}

/// The fields of `privileged.json` that say which privileged files each
/// view may receive.
#[derive(Debug, Deserialize)]
pub(crate) struct PrivilegedRecord {
    version: u32,
    views: BTreeMap<String, ViewPrivileges>,
}

/// The privileged files one view may receive.
#[derive(Debug, Deserialize)]
struct ViewPrivileges {
    paths: Vec<String>,
    version: u64,
}

/// Parses `privileged.json`.
///
/// # Errors
///
/// Fails when the bytes are not a privileged record of the supported
/// format.
pub(crate) fn parse_privileged(bytes: &[u8]) -> Result<PrivilegedRecord, String> {
    let record: PrivilegedRecord = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if record.version != FORMAT {
        return Err(format!("unsupported format version {}", record.version));
    }
    Ok(record)
}

/// The privileged files to bind into version `version` of view `name`, or
/// `None` when the record lists none for that exact version. The list was
/// computed for one view version, so it never applies to another.
pub(crate) fn privileged_paths<'a>(
    record: &'a PrivilegedRecord,
    name: &str,
    version: u64,
) -> Option<&'a [String]> {
    record
        .views
        .get(name)
        .filter(|view| view.version == version)
        .map(|view| view.paths.as_slice())
}

/// The active generation named by the target of `/run/laft/system`, which
/// must be exactly `mounts/<number>`.
pub(crate) fn generation_link(target: &[u8]) -> Option<u64> {
    number(target.strip_prefix(b"mounts/")?)
}

/// The view version named by the target of a view's `current` link, which
/// must be exactly `<number>`.
pub(crate) fn version_link(target: &[u8]) -> Option<u64> {
    number(target)
}

/// A positive decimal number without leading zeros or any other byte, so
/// that each number has one spelling and is a plain path component.
fn number(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    if !text.bytes().all(|b| b.is_ascii_digit()) || text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
