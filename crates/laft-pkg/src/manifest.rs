use std::collections::HashSet;
use std::fmt;

use laft_core::{
    Blake3Hash, Name, StoreId, Variant, Version, system_gid, system_uid, to_canonical_json,
};
use serde::{Deserialize, Serialize};

use crate::{Error, path};

/// The version of the manifest and metadata formats that this crate reads and
/// writes.
pub const FORMAT: u32 = 1;

/// The largest integer that canonical JSON represents exactly.
pub(crate) const MAX_SAFE_INTEGER: u64 = 1 << 53;

/// Setuid and setgid bits, which no store file carries.
pub(crate) const PRIVILEGE_BITS: u32 = 0o6000;

/// The architecture a package is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Arch {
    /// x86-64 with the x86-64-v3 baseline, written `x86_64-v3`.
    #[serde(rename = "x86_64-v3")]
    X86_64V3,
    /// A payload that works on every architecture, written `any`.
    #[serde(rename = "any")]
    Any,
}

impl Arch {
    /// The architecture as written in manifests and file names.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X86_64V3 => "x86_64-v3",
            Self::Any => "any",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which package a manifest describes.
///
/// It displays as `<name>-<version>-r<rev>`, plus `+<variant>` for variants
/// other than dynamic, which is how errors and people refer to a package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// The package name.
    pub name: Name,
    /// The upstream version.
    pub version: Version,
    /// The Laft revision, starting at 1.
    pub rev: u32,
    /// The build variant.
    pub variant: Variant,
    /// The architecture.
    pub arch: Arch,
}

impl Identity {
    /// The name of the package file:
    /// `<name>-<version>-r<rev>[+<variant>].<arch>.lpkg`.
    ///
    /// The file name is for people and build output directories. Tools read
    /// identity from the metadata inside the file, never from its name.
    pub fn file_name(&self) -> String {
        format!("{self}.{}.lpkg", self.arch)
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.rev == 0 {
            return Err(Error::invalid("revisions start at 1").in_package(self));
        }
        Ok(())
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-r{}", self.name, self.version, self.rev)?;
        if self.variant != Variant::Dynamic {
            write!(f, "+{}", self.variant)?;
        }
        Ok(())
    }
}

/// One path in a package payload.
///
/// Owners and groups are names from the static system account registry.
/// `None` means `root`, which is never written out, so every payload has
/// exactly one manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Entry {
    /// A directory.
    Dir {
        /// The payload path.
        path: String,
        /// Permission bits. The store creates every directory with mode
        /// `0o555` whatever this says.
        mode: u32,
        /// The owning user, `None` for root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        /// The owning group, `None` for root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    /// A regular file.
    File {
        /// The payload path.
        path: String,
        /// Permission bits, without write, setuid or setgid bits.
        mode: u32,
        /// The size in bytes.
        size: u64,
        /// The BLAKE3 hash of the content.
        hash: Blake3Hash,
        /// The owning user, `None` for root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        /// The owning group, `None` for root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    /// A symbolic link.
    Symlink {
        /// The payload path.
        path: String,
        /// The link target, verbatim. It may dangle or point into another
        /// package.
        target: String,
    },
}

impl Entry {
    /// The payload path.
    pub fn path(&self) -> &str {
        match self {
            Self::Dir { path, .. } | Self::File { path, .. } | Self::Symlink { path, .. } => path,
        }
    }

    /// The numeric owner and group, resolved through the static system
    /// account registry.
    pub(crate) fn ids(&self) -> Result<(u32, u32), Error> {
        let (owner, group) = match self {
            Self::Dir { owner, group, .. } | Self::File { owner, group, .. } => {
                (owner.as_deref(), group.as_deref())
            }
            Self::Symlink { .. } => (None, None),
        };
        Ok((
            account_id("owner", owner, system_uid)?,
            account_id("group", group, system_gid)?,
        ))
    }

    fn validate(&self) -> Result<(), Error> {
        path::check(self.path()).map_err(Error::invalid)?;
        self.ids()?;
        match self {
            Self::Dir { mode, .. } => check_mode(*mode),
            Self::File { mode, size, .. } => {
                check_mode(*mode)?;
                if mode & 0o222 != 0 {
                    return Err(Error::invalid(format!(
                        "file mode {mode:#o} has write bits, which store files never carry"
                    )));
                }
                if *size > MAX_SAFE_INTEGER {
                    return Err(Error::invalid("a file is at most 2^53 bytes long"));
                }
                Ok(())
            }
            Self::Symlink { target, .. } => check_symlink_target(target),
        }
    }
}

/// Resolves an account name, where `None` means root.
pub(crate) fn account_id(
    field: &str,
    name: Option<&str>,
    lookup: fn(&str) -> Option<u32>,
) -> Result<u32, Error> {
    match name {
        None => Ok(0),
        Some("root") => Err(Error::invalid(format!(
            "{field} `root` is the default and must be omitted"
        ))),
        Some(name) => lookup(name).ok_or_else(|| {
            Error::invalid(format!(
                "{field} `{name}` is not in the static system account registry"
            ))
        }),
    }
}

fn check_mode(mode: u32) -> Result<(), Error> {
    if mode > 0o7777 {
        return Err(Error::invalid(format!(
            "mode {mode:#o} has bits other than permission bits"
        )));
    }
    if mode & PRIVILEGE_BITS != 0 {
        return Err(Error::invalid(format!(
            "mode {mode:#o} has setuid or setgid bits, which store files never carry"
        )));
    }
    Ok(())
}

fn check_symlink_target(target: &str) -> Result<(), Error> {
    if target.is_empty() || target.len() > 4095 || target.contains('\0') {
        return Err(Error::invalid(
            "a symlink target is 1 to 4095 bytes long and has no NUL byte",
        ));
    }
    Ok(())
}

/// The payload manifest of a package: its identity, the hash of its
/// metadata and every payload path.
///
/// The manifest is stored as canonical JSON (RFC 8785), and the BLAKE3 hash of
/// those bytes is the package's store id. It therefore covers the payload, the
/// identity and, through the metadata hash, the metadata.
///
/// A `Manifest` is always valid:
///
/// - Every path follows the payload path rules, and paths are unique.
/// - Entries are sorted by path, compared as byte strings, which places every
///   directory before its contents.
/// - The parent of every entry is a directory entry, and the top-level `usr`
///   and `etc` entries are directories.
/// - Owners and groups are static system accounts other than root, which is
///   written by omitting them.
/// - No entry has setuid or setgid bits, and no file has write bits.
///
/// ```
/// use laft_core::Blake3Hash;
/// use laft_pkg::{Arch, Entry, Identity, Manifest};
///
/// let identity = Identity {
///     name: "hello".parse()?,
///     version: "2.12".parse()?,
///     rev: 1,
///     variant: Default::default(),
///     arch: Arch::X86_64V3,
/// };
/// let entries = vec![
///     Entry::File {
///         path: "usr/bin/hello".into(),
///         mode: 0o555,
///         size: 5,
///         hash: Blake3Hash::of(b"hello"),
///         owner: None,
///         group: None,
///     },
///     Entry::Dir { path: "usr/bin".into(), mode: 0o755, owner: None, group: None },
///     Entry::Dir { path: "usr".into(), mode: 0o755, owner: None, group: None },
/// ];
/// let manifest = Manifest::new(identity, Blake3Hash::of(b"{}"), entries)?;
/// assert_eq!(manifest.entries()[0].path(), "usr");
/// assert_eq!(Manifest::from_json(&manifest.to_json())?, manifest);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Manifest {
    format: u32,
    package: Identity,
    meta: Blake3Hash,
    entries: Vec<Entry>,
}

/// The stored form before validation. `Manifest` has no `Deserialize`, so
/// that every `Manifest` is validated.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    format: u32,
    package: Identity,
    meta: Blake3Hash,
    entries: Vec<Entry>,
}

impl Manifest {
    /// Builds the manifest of package `package`, whose canonical metadata
    /// hashes to `meta`. Entries may come in any order and are sorted.
    ///
    /// # Errors
    ///
    /// Fails when the result would break one of the rules listed on
    /// [`Manifest`].
    pub fn new(
        package: Identity,
        meta: Blake3Hash,
        mut entries: Vec<Entry>,
    ) -> Result<Self, Error> {
        entries.sort_by(|a, b| a.path().cmp(b.path()));
        let manifest = Self {
            format: FORMAT,
            package,
            meta,
            entries,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parses a manifest from its stored form.
    ///
    /// # Errors
    ///
    /// Fails when `bytes` are not the canonical JSON of a valid manifest of
    /// the supported format.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let stored: Stored = serde_json::from_slice(bytes)
            .map_err(|e| Error::invalid(format!("malformed manifest: {e}")))?;
        let manifest = Self {
            format: stored.format,
            package: stored.package,
            meta: stored.meta,
            entries: stored.entries,
        };
        manifest.validate()?;
        if manifest.to_json() != bytes {
            return Err(
                Error::invalid("the manifest is not canonical JSON").in_package(&manifest.package)
            );
        }
        Ok(manifest)
    }

    /// The stored form: canonical JSON (RFC 8785).
    pub fn to_json(&self) -> Vec<u8> {
        to_canonical_json(self)
            .expect("a valid manifest has no integer above 2^53, so it is canonical JSON")
    }

    /// The package this manifest describes.
    pub fn package(&self) -> &Identity {
        &self.package
    }

    /// The BLAKE3 hash of the package's canonical metadata.
    pub fn meta_hash(&self) -> &Blake3Hash {
        &self.meta
    }

    /// The payload entries, sorted by path.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The sum of all file sizes, in bytes.
    pub fn installed_size(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| match entry {
                Entry::File { size, .. } => *size,
                _ => 0,
            })
            .sum()
    }

    /// The store id: the first 160 bits of the BLAKE3 hash of
    /// [`Self::to_json`].
    pub fn store_id(&self) -> StoreId {
        StoreId::from_manifest_hash(&Blake3Hash::of(&self.to_json()))
    }

    fn validate(&self) -> Result<(), Error> {
        if self.format != FORMAT {
            return Err(Error::invalid(format!(
                "manifest format {} is not supported, only {FORMAT} is",
                self.format
            )));
        }
        self.package.validate()?;
        let in_package = |e: Error| e.in_package(&self.package);
        let mut dirs = HashSet::new();
        let mut total: u64 = 0;
        let mut previous: Option<&str> = None;
        for entry in &self.entries {
            let path = entry.path();
            let at = |e: Error| in_package(e.at(path));
            entry.validate().map_err(at)?;
            if previous.is_some_and(|p| p >= path) {
                return Err(at(Error::invalid(
                    "entries are sorted by path without duplicates",
                )));
            }
            previous = Some(path);
            match path.rsplit_once('/') {
                Some((parent, _)) if !dirs.contains(parent) => {
                    return Err(at(Error::invalid(format!(
                        "the parent `{parent}` is not a directory entry"
                    ))));
                }
                None if !matches!(entry, Entry::Dir { .. }) => {
                    return Err(at(Error::invalid(
                        "the top-level `usr` and `etc` entries are directories",
                    )));
                }
                _ => {}
            }
            match entry {
                Entry::Dir { .. } => {
                    dirs.insert(path);
                }
                Entry::File { size, .. } => {
                    total = total.saturating_add(*size);
                    if total > MAX_SAFE_INTEGER {
                        return Err(at(Error::invalid("a payload is at most 2^53 bytes")));
                    }
                }
                Entry::Symlink { .. } => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
