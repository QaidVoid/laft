//! Privileged copies: the files of a system generation that keep setuid,
//! setgid or file capabilities, and the record of which views may receive
//! them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, fchown};
use std::path::{Component, Path};

use laft_core::{FileCapabilities, Name, StoreId, system_gid, system_uid};
use laft_store::{Generation, Reason, Root};
use serde::{Deserialize, Serialize};

use crate::config::Privileged;
use crate::error::{ConfigError, Error, Result, Warning, corrupt, io};
use crate::{Declared, fsx, tree};

/// The name of the privileged copies record in a generation.
pub const PRIVILEGED_FILE: &str = "privileged.json";

/// The version of [`PrivilegedRecord`].
const VERSION: u32 = 1;

/// The suffix of a privileged copy before it replaces the hard link.
const COPY_SUFFIX: &str = ".laft-priv";

/// The privileged copies of a generation, stored as `privileged.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegedRecord {
    /// The format version, 1.
    pub version: u32,
    /// Every privileged copy, sorted by path.
    pub files: Vec<PrivilegedFile>,
    /// For each view of the generation, the privileged files it may
    /// receive, by view name.
    pub views: BTreeMap<Name, ViewPrivileges>,
}

/// A privileged copy in a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegedFile {
    /// The absolute path, such as `/usr/bin/doas`.
    pub path: String,
    /// The owning uid.
    pub uid: u32,
    /// The owning gid.
    pub gid: u32,
    /// The mode, including setuid and setgid bits.
    pub mode: u32,
    /// The file capabilities in text form, if any.
    pub caps: Option<String>,
    /// The store id of the entry the copy was made from.
    pub source: StoreId,
}

/// The privileged files one view may receive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewPrivileges {
    /// The absolute paths, sorted.
    pub paths: Vec<String>,
    /// The view version the list was computed for.
    pub version: u64,
}

impl PrivilegedRecord {
    /// Reads the record of the generation directory `generation_dir`.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or is not a supported version.
    pub fn read(generation_dir: &Path) -> Result<Self> {
        let path = generation_dir.join(PRIVILEGED_FILE);
        let bytes = fs::read(&path).map_err(io(&path))?;
        let record: Self = serde_json::from_slice(&bytes).map_err(|e| corrupt(&path, e))?;
        if record.version != VERSION {
            return Err(corrupt(&path, "unsupported format version"));
        }
        Ok(record)
    }
}

/// Replaces the hard link of every enabled entry of `entries` in the
/// generation tree `tree` with a privileged copy, then writes
/// `privileged.json` into `tree`.
///
/// `generation` is the record the generation gets, `owner` names the store
/// entry that contributes a tree path, and `views` the declared views of
/// the generation with their versions. Without root, the copies keep the
/// caller as owner and get no capabilities, and a warning says so.
pub(crate) fn apply(
    root: &Root,
    tree: &Path,
    entries: &BTreeMap<String, Privileged>,
    declared: &Declared,
    generation: &Generation,
    owner: impl Fn(&str) -> Option<StoreId>,
    warnings: &mut Vec<Warning>,
) -> Result<PrivilegedRecord> {
    let mut errors = Vec::new();
    let mut planned = Vec::new();
    for (path, entry) in entries.iter().filter(|(_, e)| e.enable) {
        if let Some(file) = plan(path, entry, declared, &mut errors) {
            planned.push(file);
        }
    }
    if !errors.is_empty() {
        return Err(Error::Config(errors));
    }
    let privileged = fsx::is_root();
    let mut files = Vec::new();
    for (path, uid, gid, mode, caps) in planned {
        let rel = &path[1..];
        let source = match (tree::lstat(tree, rel)?, owner(rel)) {
            (Some(metadata), Some(source)) if metadata.is_file() => source,
            _ => {
                warnings.push(Warning::InactivePrivileged(path));
                continue;
            }
        };
        copy(
            tree,
            rel,
            uid,
            gid,
            mode,
            caps.as_ref().map(|(_, c)| c),
            privileged,
        )?;
        files.push(PrivilegedFile {
            path,
            uid,
            gid,
            mode,
            caps: caps.map(|(text, _)| text),
            source,
        });
    }
    if !privileged && !files.is_empty() {
        warnings.push(Warning::PrivilegesNotApplied(
            files.iter().map(|f| f.path.clone()).collect(),
        ));
    }
    let mut views = BTreeMap::new();
    for (name, &version) in &generation.views {
        let paths = view_paths(root, name, version, &files, generation)?;
        views.insert(name.clone(), ViewPrivileges { paths, version });
    }
    let record = PrivilegedRecord {
        version: VERSION,
        files,
        views,
    };
    let path = tree.join(PRIVILEGED_FILE);
    let bytes = laft_core::to_canonical_json(&record)
        .map_err(|e| corrupt(&path, format!("not canonical JSON: {e}")))?;
    fsx::write_new(&path, &bytes, 0o444)?;
    Ok(record)
}

/// A privileged file to copy: its path, uid, gid, mode and capabilities.
type Planned = (String, u32, u32, u32, Option<(String, FileCapabilities)>);

/// Checks one enabled entry and resolves its owner and group.
fn plan(
    path: &str,
    entry: &Privileged,
    declared: &Declared,
    errors: &mut Vec<ConfigError>,
) -> Option<Planned> {
    let rel = Path::new(path).strip_prefix("/").ok();
    let normalized = rel.is_some_and(|rel| {
        (rel.starts_with("usr/bin") || rel.starts_with("usr/lib"))
            && rel.components().count() > 2
            && rel.components().all(|c| matches!(c, Component::Normal(_)))
    }) && !path.ends_with('/')
        && !path.contains("//");
    if !normalized {
        errors.push(ConfigError::PrivilegedPath(path.to_owned()));
        return None;
    }
    if entry.mode > 0o7777 {
        errors.push(ConfigError::InvalidValue {
            context: format!("privileged {path:?}"),
            reason: "the mode may only hold permission bits up to 0o7777",
        });
        return None;
    }
    let caps = match &entry.caps {
        None => None,
        Some(text) => match text.parse::<FileCapabilities>() {
            Ok(caps) => Some((text.clone(), caps)),
            Err(_) => {
                errors.push(ConfigError::InvalidCaps {
                    path: path.to_owned(),
                    caps: text.clone(),
                });
                return None;
            }
        },
    };
    let uid = declared
        .users
        .get(&entry.owner)
        .map(|u| u.uid)
        .or_else(|| system_uid(&entry.owner));
    let gid = declared
        .groups
        .get(&entry.group)
        .map(|g| g.gid)
        .or_else(|| system_gid(&entry.group));
    let unknown = |kind, name: &str| ConfigError::UnknownAccount {
        kind,
        name: name.to_owned(),
        context: format!("privileged {path:?}"),
    };
    match (uid, gid) {
        (Some(uid), Some(gid)) => Some((path.to_owned(), uid, gid, entry.mode, caps)),
        (uid, gid) => {
            if uid.is_none() {
                errors.push(unknown("user", &entry.owner));
            }
            if gid.is_none() {
                errors.push(unknown("group", &entry.group));
            }
            None
        }
    }
}

/// Copies the file `rel` of `tree` to a new inode and renames the copy
/// over it, so the store file it was linked to is never opened for
/// writing. The owner is set before the mode, because changing the owner
/// clears setuid bits.
fn copy(
    tree: &Path,
    rel: &str,
    uid: u32,
    gid: u32,
    mode: u32,
    caps: Option<&FileCapabilities>,
    privileged: bool,
) -> Result<()> {
    let path = tree.join(rel);
    let tmp = fsx::sibling(&path, COPY_SUFFIX);
    fsx::remove_if_present(&tmp)?;
    let mut source = File::open(&path).map_err(io(&path))?;
    let mut copy = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(io(&tmp))?;
    io::copy(&mut source, &mut copy).map_err(io(&tmp))?;
    if privileged {
        fchown(&copy, Some(uid), Some(gid)).map_err(io(&tmp))?;
    }
    copy.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(io(&tmp))?;
    if let Some(caps) = caps.filter(|_| privileged) {
        rustix::fs::fsetxattr(
            &copy,
            "security.capability",
            &caps.to_xattr(),
            rustix::fs::XattrFlags::empty(),
        )
        .map_err(|e| io(&tmp)(e.into()))?;
    }
    fs::rename(&tmp, &path).map_err(io(&path))
}

/// The privileged files that version `version` of view `name` may receive:
/// those the view has as regular files, and whose package has its whole
/// runtime closure in the view with the same store ids as in the system.
fn view_paths(
    root: &Root,
    name: &Name,
    version: u64,
    files: &[PrivilegedFile],
    generation: &Generation,
) -> Result<Vec<String>> {
    let view = root.view(name, version)?;
    let in_view: BTreeMap<&Name, StoreId> = view.packages.iter().map(|p| (&p.name, p.id)).collect();
    let by_id: BTreeMap<StoreId, &Name> = generation
        .packages
        .iter()
        .map(|p| (p.id, &p.name))
        .collect();
    let system: BTreeMap<&Name, StoreId> = generation
        .packages
        .iter()
        .map(|p| (&p.name, p.id))
        .collect();
    let mut depends: BTreeMap<&Name, BTreeSet<&Name>> = BTreeMap::new();
    for package in &generation.packages {
        for reason in &package.reasons {
            if let Reason::Dependency { by, .. } = reason {
                depends.entry(by).or_default().insert(&package.name);
            }
        }
    }
    let view_dir = root
        .path()
        .join("laft/views")
        .join(name.as_str())
        .join(version.to_string());
    let mut paths = Vec::new();
    for file in files {
        if !tree::lstat(&view_dir, &file.path[1..])?.is_some_and(|m| m.is_file()) {
            continue;
        }
        let Some(&package) = by_id.get(&file.source) else {
            continue;
        };
        let mut closure = BTreeSet::from([package]);
        let mut pending = vec![package];
        while let Some(next) = pending.pop() {
            for dep in depends.get(next).into_iter().flatten() {
                if closure.insert(dep) {
                    pending.push(dep);
                }
            }
        }
        if closure
            .iter()
            .all(|name| in_view.contains_key(name) && in_view.get(name) == system.get(name))
        {
            paths.push(file.path.clone());
        }
    }
    Ok(paths)
}

#[cfg(test)]
#[path = "privileged_tests.rs"]
mod tests;
