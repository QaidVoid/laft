use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use laft_core::{Blake3Hash, Name, StoreId, system_gid, system_uid};
use laft_pkg::{Entry, Manifest, Meta, Reader};

use crate::error::{Error, Result, corrupt, io};
use crate::{PackageInput, PackageRecord, fsx, view};

/// The directory inside an entry that holds its documents.
const DOCS_DIR: &str = ".laft";
/// The metadata, with the exact bytes from the package file.
const META_PATH: &str = ".laft/meta.json";
/// The payload manifest, with the exact bytes from the package file.
const MANIFEST_PATH: &str = ".laft/manifest.json";
/// The mode of both documents.
const DOC_MODE: u32 = 0o444;
/// The extended attribute that holds file capabilities.
const CAPABILITY_XATTR: &str = "security.capability";

/// The package store: immutable entries, each a directory named
/// `<id>-<name>-<version>[+<variant>]` that holds a package's payload and,
/// under `.laft/`, its metadata and manifest.
///
/// An entry exists exactly when its directory exists. It appears with one
/// rename and is never written to after that, so a reader sees an entry
/// whole or not at all. Entries are added by [`crate::Transaction::add`] and
/// removed only by garbage collection.
///
/// Files the manifest gives to root are owned by the user that added the
/// entry, which is root on a real system. Verification expects that.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

/// One way in which a store entry differs from its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// The path inside the entry.
    pub path: String,
    /// What differs, for example `mode is 0o644, the manifest says 0o444`.
    pub problem: String,
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.problem)
    }
}

/// Whether `name` can name a store entry: one path component that is not
/// hidden, so joining it to the store directory never leaves it.
pub(crate) fn is_entry_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\0'])
}

/// A package unpacked into the staging directory and ready to publish.
pub(crate) struct Staged {
    pub(crate) dir_name: String,
    path: PathBuf,
}

/// The result of preparing a package for the store.
pub(crate) enum Prepared {
    /// The entry exists already.
    Exists(String),
    /// The entry is staged.
    Staged(Staged),
}

impl Store {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The store in the directory `dir`, for a store that lives outside a
    /// [`crate::Root`], such as the store a package builder keeps its build
    /// dependencies in. The directory is created when the first entry is
    /// added.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self::new(dir.into())
    }

    /// Adds the package file `lpkg`, whose store id must be `expected`,
    /// without a transaction, and returns its entry's directory name.
    ///
    /// The package is unpacked and verified in a new directory below the
    /// staging directory `tmp`, which must be on the same filesystem as the
    /// store, and published with one rename, exactly as
    /// [`crate::Transaction::add`] does. Nothing is journaled, so this is
    /// only for a store that no generation uses: an interrupted call leaves
    /// at most a staging directory in `tmp`, never a partial entry. Adding a
    /// package whose entry exists does nothing.
    ///
    /// # Errors
    ///
    /// Fails when the package is invalid or does not match its manifest,
    /// when its id is not `expected`, or when the store cannot be written.
    pub fn insert(&self, lpkg: &Path, expected: &StoreId, tmp: &Path) -> Result<String> {
        for dir in [self.dir.as_path(), tmp] {
            fs::create_dir_all(dir).map_err(io(dir))?;
        }
        let tag = format!("insert-{}", std::process::id());
        match self.stage(lpkg, expected, tmp, &tag)? {
            Prepared::Exists(dir) => Ok(dir),
            Prepared::Staged(staged) => {
                let dir = staged.dir_name.clone();
                self.publish(staged)?;
                Ok(dir)
            }
        }
    }

    /// Assembles the `usr` tree of the entries `packages` as `tree/usr`, for
    /// a view that no generation records, such as the build view of one
    /// package build, and returns the records of the packages.
    ///
    /// The tree is laid out as [`crate::Transaction::assemble_view`] lays
    /// out a declared view: a hard link to every store file, a copy of every
    /// symlink and a directory for every directory under `usr`, with
    /// `alternatives` choosing the member of every alternatives group
    /// present. Its directories stay writable, so the caller can remove the
    /// tree. `tree` must exist and be on the same filesystem as the store.
    ///
    /// # Errors
    ///
    /// Fails when an entry is missing, when two packages own a path or have
    /// the same name, when `alternatives` does not choose a member present
    /// for every group, when a store file is near the hard link limit, or
    /// when writing fails.
    pub fn assemble(
        &self,
        packages: &[PackageInput],
        alternatives: &BTreeMap<Name, Name>,
        tree: &Path,
    ) -> Result<Vec<PackageRecord>> {
        let layout = view::layout(self, "build", packages, alternatives, &[])?;
        layout.create(tree)?;
        Ok(layout.packages)
    }

    /// The path of the entry `dir_name`.
    pub fn path(&self, dir_name: &str) -> PathBuf {
        self.dir.join(dir_name)
    }

    /// The directory names of every entry, sorted.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be read.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let items = match fs::read_dir(&self.dir) {
            Ok(items) => items,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(names),
            Err(e) => return Err(io(&self.dir)(e)),
        };
        for item in items {
            let name = item.map_err(io(&self.dir))?.file_name();
            names.push(name.to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    /// Reads and validates the manifest of entry `dir_name`.
    ///
    /// # Errors
    ///
    /// Fails when the entry does not exist, when the manifest is invalid, or
    /// when the entry's directory name is not the one its manifest yields.
    pub fn manifest(&self, dir_name: &str) -> Result<Manifest> {
        let path = self.entry_path(dir_name)?.join(MANIFEST_PATH);
        let bytes = fs::read(&path).map_err(|e| missing(dir_name, &path, e))?;
        let manifest = Manifest::from_json(&bytes)?;
        if entry_dir_name(&manifest) != dir_name {
            return Err(corrupt(&path, "the manifest belongs to a different entry"));
        }
        Ok(manifest)
    }

    /// Reads and validates the metadata of entry `dir_name`.
    ///
    /// # Errors
    ///
    /// Fails as [`Self::manifest`] does, and when the metadata is invalid or
    /// is not the metadata the manifest names.
    pub fn meta(&self, dir_name: &str) -> Result<Meta> {
        self.load(dir_name).map(|(_, meta)| meta)
    }

    /// Reads the manifest and the metadata of entry `dir_name`.
    pub(crate) fn load(&self, dir_name: &str) -> Result<(Manifest, Meta)> {
        let manifest = self.manifest(dir_name)?;
        let meta = self.meta_of(dir_name, &manifest)?;
        Ok((manifest, meta))
    }

    /// Reads the metadata of entry `dir_name`, whose manifest is `manifest`.
    fn meta_of(&self, dir_name: &str, manifest: &Manifest) -> Result<Meta> {
        let path = self.path(dir_name).join(META_PATH);
        let bytes = fs::read(&path).map_err(io(&path))?;
        if Blake3Hash::of(&bytes) != *manifest.meta_hash() {
            return Err(corrupt(
                &path,
                "the metadata hash differs from the manifest",
            ));
        }
        let meta = Meta::from_json(&bytes)?;
        if meta.identity() != *manifest.package() {
            return Err(corrupt(&path, "the metadata names a different package"));
        }
        Ok(meta)
    }

    /// Compares entry `dir_name` with its manifest and returns every
    /// difference.
    ///
    /// Checks every path's type, mode, owner and group, every file's size,
    /// every symlink's target, that no file carries capabilities, and that
    /// the entry holds no path the manifest does not list. Directories must
    /// have mode `0o555`, and the documents under `.laft/` mode `0o444`.
    /// With `full`, file contents are hashed and compared as well.
    ///
    /// A missing or invalid manifest or metadata is reported as a difference
    /// of that document, because it leaves nothing to compare.
    ///
    /// # Errors
    ///
    /// Fails when the entry does not exist or cannot be read.
    pub fn verify(&self, dir_name: &str, full: bool) -> Result<Vec<Difference>> {
        let root = self.entry_path(dir_name)?;
        fs::symlink_metadata(&root).map_err(|e| missing(dir_name, &root, e))?;
        let mut differences = Vec::new();
        let mut report = |path: &str, problem: String| {
            differences.push(Difference {
                path: path.to_owned(),
                problem,
            });
        };
        let manifest = match self.manifest(dir_name) {
            Ok(manifest) => manifest,
            Err(e) => {
                report(MANIFEST_PATH, e.to_string());
                return Ok(differences);
            }
        };
        if let Err(e) = self.meta_of(dir_name, &manifest) {
            report(META_PATH, e.to_string());
            return Ok(differences);
        }
        let owner = (
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        );
        check_dir(&root, "", owner, &mut report)?;
        check_dir(&root, DOCS_DIR, owner, &mut report)?;
        for doc in [META_PATH, MANIFEST_PATH] {
            let metadata = fs::symlink_metadata(root.join(doc)).map_err(io(&root))?;
            check_mode(doc, metadata.mode(), DOC_MODE, &mut report);
        }
        for entry in manifest.entries() {
            check_entry(&root, entry, owner, full, &mut report)?;
        }
        let mut listed: BTreeSet<&str> = manifest.entries().iter().map(Entry::path).collect();
        listed.extend([DOCS_DIR, META_PATH, MANIFEST_PATH]);
        let mut extra = Vec::new();
        unlisted(&root, "", &listed, &mut extra)?;
        for path in extra {
            report(&path, "is not in the manifest".into());
        }
        Ok(differences)
    }

    /// The path of entry `dir_name`, which must be a single path component.
    fn entry_path(&self, dir_name: &str) -> Result<PathBuf> {
        if !is_entry_name(dir_name) {
            return Err(Error::MissingEntry(dir_name.to_owned()));
        }
        Ok(self.path(dir_name))
    }

    /// Unpacks the package file `lpkg` into the staging directory `tmp`
    /// unless its entry exists already.
    ///
    /// The staged tree is complete: documents written, modification times
    /// set to 1 second after the epoch and directories sealed, except the
    /// top directory, which [`Self::publish`] seals after the rename.
    pub(crate) fn stage(
        &self,
        lpkg: &Path,
        expected: &StoreId,
        tmp: &Path,
        tag: &str,
    ) -> Result<Prepared> {
        let file = File::open(lpkg).map_err(io(lpkg))?;
        let reader = Reader::new(BufReader::new(file))?;
        let actual = reader.manifest().store_id();
        if actual != *expected {
            return Err(Error::IdMismatch {
                expected: *expected,
                actual,
            });
        }
        let dir_name = entry_dir_name(reader.manifest());
        if fs::symlink_metadata(self.path(&dir_name)).is_ok() {
            return Ok(Prepared::Exists(dir_name));
        }
        let staging = tmp.join(format!("store-{actual}.{tag}"));
        fsx::remove_tree(&staging)?;
        fsx::create_dir(&staging, 0o700)?;
        match unpack(reader, &staging, expected) {
            Ok(()) => Ok(Prepared::Staged(Staged {
                dir_name,
                path: staging,
            })),
            Err(e) => {
                let _ = fsx::remove_tree(&staging);
                Err(e)
            }
        }
    }

    /// Publishes a staged entry with one rename. When the entry appeared in
    /// the meantime, the staged copy is discarded.
    pub(crate) fn publish(&self, staged: Staged) -> Result<()> {
        let target = self.path(&staged.dir_name);
        if let Err(e) = fs::rename(&staged.path, &target) {
            let exists = fs::symlink_metadata(&target).is_ok();
            fsx::remove_tree(&staged.path)?;
            return if exists { Ok(()) } else { Err(io(&target)(e)) };
        }
        fsx::set_mode(&target, fsx::SEALED_DIR)
    }
}

/// The directory name of the entry for `manifest`.
pub(crate) fn entry_dir_name(manifest: &Manifest) -> String {
    let package = manifest.package();
    manifest
        .store_id()
        .dir_name(&package.name, &package.version, package.variant)
}

fn missing(dir_name: &str, path: &Path, e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::NotFound {
        Error::MissingEntry(dir_name.to_owned())
    } else {
        io(path)(e)
    }
}

fn unpack<R: io::Read>(reader: Reader<R>, staging: &Path, expected: &StoreId) -> Result<()> {
    let meta = reader.meta_bytes().to_vec();
    let manifest = reader.manifest_bytes().to_vec();
    reader.unpack(staging)?;
    let docs = staging.join(DOCS_DIR);
    fsx::create_dir(&docs, 0o755)?;
    fsx::write_new(&staging.join(META_PATH), &meta, DOC_MODE)?;
    let manifest_path = staging.join(MANIFEST_PATH);
    fsx::write_new(&manifest_path, &manifest, DOC_MODE)?;
    let written = fs::read(&manifest_path).map_err(io(&manifest_path))?;
    let id = StoreId::from_manifest_hash(&Blake3Hash::of(&written));
    if id != *expected {
        return Err(Error::IdMismatch {
            expected: *expected,
            actual: id,
        });
    }
    fsx::seal(staging, true)
}

type Report<'a> = dyn FnMut(&str, String) + 'a;

fn check_mode(path: &str, actual: u32, expected: u32, report: &mut Report<'_>) {
    let actual = actual & 0o7777;
    if actual != expected {
        report(
            path,
            format!("mode is {actual:#o}, the manifest says {expected:#o}"),
        );
    }
}

fn check_owner(path: &str, metadata: &fs::Metadata, expected: (u32, u32), report: &mut Report<'_>) {
    let actual = (metadata.uid(), metadata.gid());
    if actual != expected {
        report(
            path,
            format!(
                "owner is {}:{}, expected {}:{}",
                actual.0, actual.1, expected.0, expected.1
            ),
        );
    }
}

fn check_dir(root: &Path, path: &str, owner: (u32, u32), report: &mut Report<'_>) -> Result<()> {
    let full = root.join(path);
    match fs::symlink_metadata(&full) {
        Ok(m) if m.is_dir() => {
            check_mode(path, m.mode(), fsx::SEALED_DIR, report);
            check_owner(path, &m, owner, report);
        }
        Ok(_) => report(path, "is not a directory".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => report(path, "is missing".into()),
        Err(e) => return Err(io(&full)(e)),
    }
    Ok(())
}

fn check_entry(
    root: &Path,
    entry: &Entry,
    caller: (u32, u32),
    full: bool,
    report: &mut Report<'_>,
) -> Result<()> {
    let path = entry.path();
    let on_disk = root.join(path);
    let metadata = match fs::symlink_metadata(&on_disk) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            report(path, "is missing".into());
            return Ok(());
        }
        Err(e) => return Err(io(&on_disk)(e)),
    };
    let ids = |owner: &Option<String>, group: &Option<String>| {
        (
            owner.as_deref().map_or(Some(caller.0), system_uid),
            group.as_deref().map_or(Some(caller.1), system_gid),
        )
    };
    match entry {
        Entry::Dir { owner, group, .. } => {
            if !metadata.is_dir() {
                report(path, "is not a directory".into());
                return Ok(());
            }
            check_mode(path, metadata.mode(), fsx::SEALED_DIR, report);
            if let (Some(uid), Some(gid)) = ids(owner, group) {
                check_owner(path, &metadata, (uid, gid), report);
            }
        }
        Entry::File {
            mode,
            size,
            hash,
            owner,
            group,
            ..
        } => {
            if !metadata.is_file() {
                report(path, "is not a regular file".into());
                return Ok(());
            }
            check_mode(path, metadata.permissions().mode(), *mode, report);
            if let (Some(uid), Some(gid)) = ids(owner, group) {
                check_owner(path, &metadata, (uid, gid), report);
            }
            if metadata.len() != *size {
                report(
                    path,
                    format!("size is {}, the manifest says {size}", metadata.len()),
                );
            } else if full {
                let actual = hash_file(&on_disk)?;
                if actual != *hash {
                    report(
                        path,
                        format!("content hash is {actual}, the manifest says {hash}"),
                    );
                }
            }
            match rustix::fs::lgetxattr_query(&on_disk, CAPABILITY_XATTR) {
                Ok(_) => report(path, "carries file capabilities".into()),
                Err(rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP) => {}
                Err(e) => return Err(io(&on_disk)(e)),
            }
        }
        Entry::Symlink { target, .. } => {
            if !metadata.is_symlink() {
                report(path, "is not a symlink".into());
                return Ok(());
            }
            let actual = fs::read_link(&on_disk).map_err(io(&on_disk))?;
            if actual.as_os_str() != target.as_str() {
                report(
                    path,
                    format!("points to {}, the manifest says {target}", actual.display()),
                );
            }
            check_owner(path, &metadata, caller, report);
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<Blake3Hash> {
    let mut file = File::open(path).map_err(io(path))?;
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher).map_err(io(path))?;
    Ok(Blake3Hash::from_bytes(*hasher.finalize().as_bytes()))
}

/// Collects every path below `dir` that `listed` does not contain.
fn unlisted(
    dir: &Path,
    prefix: &str,
    listed: &BTreeSet<&str>,
    extra: &mut Vec<String>,
) -> Result<()> {
    for item in fs::read_dir(dir).map_err(io(dir))? {
        let item = item.map_err(io(dir))?;
        let name = item.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if !listed.contains(path.as_str()) {
            extra.push(path);
        } else if item.file_type().map_err(io(dir))?.is_dir() {
            unlisted(&item.path(), &path, listed, extra)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
