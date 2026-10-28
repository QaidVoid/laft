//! Reconciling the live `/etc` with a generation's static tree.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};

use laft_core::Blake3Hash;
use laft_store::{EtcRecord, Lock, Root};

use crate::accounts::DATABASES;
use crate::error::{Conflict, ConflictKind, Error, Result, Warning, corrupt, io};
use crate::fsx;
use crate::state::{self, CopiesRecord, ManagedRecord, Record};

/// What reconciling does with a path it would have to overwrite although
/// Laft did not create it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    /// Change nothing and fail with every conflict. Used when applying a
    /// configuration, so that the user can decide.
    Stop,
    /// Leave the path alone, report it as a warning and reconcile
    /// everything else. Used at boot, where nobody can decide.
    Skip,
}

/// What reconciling `/etc` changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EtcReport {
    /// Managed symlinks that were created, relative to `/etc`.
    pub linked: Vec<String>,
    /// Managed symlinks that were removed.
    pub unlinked: Vec<String>,
    /// Paths whose previous content was moved to `<path>.laft-orig`.
    pub adopted: Vec<String>,
    /// Copy-mode files that were written.
    pub copied: Vec<String>,
    /// Things the user should know about.
    pub warnings: Vec<Warning>,
}

/// A managed symlink to create.
struct Link {
    path: String,
    /// Move the existing entry to `<path>.laft-orig` first.
    adopt: bool,
    /// Remove the vacated directory Laft created at the path first.
    replace_dir: bool,
}

/// A copy-mode file to write.
struct Copy {
    path: String,
    bytes: Vec<u8>,
    mode: u32,
    owner: fsx::Owner,
    hash: Blake3Hash,
    /// Move the existing entry to `<path>.laft-orig` first.
    adopt: bool,
    /// The live file has this content already.
    current: bool,
    /// Remove the vacated directory Laft created at the path first.
    replace_dir: bool,
}

/// Everything reconciling would do, decided before anything changes.
#[derive(Default)]
struct Plan {
    links: Vec<Link>,
    copies: Vec<Copy>,
    unlink: Vec<String>,
    /// Copy-mode files that are no longer copied.
    stale: Vec<String>,
    new_dirs: BTreeSet<String>,
    managed: BTreeSet<String>,
    conflicts: Vec<Conflict>,
    warnings: Vec<Warning>,
}

/// The paths of `root` reconciling works with.
struct Paths {
    etc: PathBuf,
    static_etc: PathBuf,
}

/// Lists the conflicts that reconciling `/etc` with generation `generation`
/// would hit, without changing anything. `adopt` holds paths, absolute or
/// relative to `/etc`, that may be moved aside.
///
/// # Errors
///
/// Fails when the generation or the records cannot be read, and with
/// [`Error::Invalid`] when an absolute path in `adopt` is not below `/etc`.
pub fn check_etc(lock: &Lock, generation: u64, adopt: &BTreeSet<String>) -> Result<Vec<Conflict>> {
    let root = lock.root();
    let record = root.generation(generation)?.etc;
    let paths = paths(root, generation);
    let plan = plan(
        &paths,
        &record,
        &ManagedRecord::load(root)?,
        &CopiesRecord::load(root)?,
        &relative_set(adopt)?,
    )?;
    Ok(plan.conflicts)
}

/// Makes the live `/etc` match generation `generation`.
///
/// Every managed path becomes a symlink to `/etc/static/<path>`, which
/// follows the active generation. Missing parent directories are created
/// and recorded, so that they are removed again once they are empty and
/// unused. Managed symlinks of the previous generation that this one does
/// not manage are removed. Copy-mode files are written with a crash safe
/// replacement, unless they were changed outside Laft since Laft wrote
/// them. The account databases are left to [`crate::apply_accounts`].
///
/// A path that exists although Laft did not create it is a conflict,
/// unless it is listed in `adopt`, absolute or relative to `/etc`. A
/// directory Laft created that holds nothing but the managed symlinks this
/// reconcile removes is replaced without a conflict. Adopting
/// moves it to `<path>.laft-orig`. With [`OnConflict::Stop`], any conflict
/// stops reconciling before anything changes. A managed symlink that the
/// user replaced is not a conflict: it is left alone with a warning on
/// every reconcile until the configuration stops managing it or it is
/// reset with [`reset_etc`].
///
/// What Laft put into `/etc` is recorded in `laft/state/etc-managed.json`
/// and `laft/state/etc-copies.json`. Both are widened before `/etc`
/// changes, so that reconciling again after a crash at any point removes or
/// replaces everything Laft may have created.
///
/// # Errors
///
/// Fails with [`Error::Conflicts`] under [`OnConflict::Stop`], when the
/// generation or the records cannot be read, and when `/etc` cannot be
/// changed. Fails with [`Error::Invalid`] when an absolute path in `adopt`
/// is not below `/etc`.
pub fn reconcile_etc(
    lock: &Lock,
    generation: u64,
    adopt: &BTreeSet<String>,
    on_conflict: OnConflict,
) -> Result<EtcReport> {
    let root = lock.root();
    let record = root.generation(generation)?.etc;
    let paths = paths(root, generation);
    let previous = ManagedRecord::load(root)?;
    let mut copies = CopiesRecord::load(root)?;
    let mut plan = plan(&paths, &record, &previous, &copies, &relative_set(adopt)?)?;
    if !plan.conflicts.is_empty() && on_conflict == OnConflict::Stop {
        return Err(Error::Conflicts(plan.conflicts));
    }
    let mut report = EtcReport {
        warnings: std::mem::take(&mut plan.warnings),
        ..EtcReport::default()
    };
    report
        .warnings
        .extend(plan.conflicts.drain(..).map(Warning::Skipped));

    let mut dirs: BTreeSet<String> = previous.dirs.union(&plan.new_dirs).cloned().collect();
    ManagedRecord {
        generation: Some(generation),
        managed: previous.managed.union(&plan.managed).cloned().collect(),
        dirs: dirs.clone(),
        ..ManagedRecord::default()
    }
    .save(root)?;
    for copy in &plan.copies {
        copies.pending.insert(copy.path.clone(), copy.hash);
    }
    copies.save(root)?;

    for path in &plan.unlink {
        let live = paths.etc.join(path);
        fsx::remove_if_present(&live)?;
        fsx::sync_dir(fsx::parent(&live))?;
        report.unlinked.push(path.clone());
    }
    for dir in &plan.new_dirs {
        let live = paths.etc.join(dir);
        if fsx::lstat(&live)?.is_none() {
            fs::create_dir(&live).map_err(io(&live))?;
            fsx::sync_dir(fsx::parent(&live))?;
        }
    }
    for link in &plan.links {
        let live = paths.etc.join(&link.path);
        if link.replace_dir {
            remove_vacated(&live)?;
            forget_dirs(&mut dirs, &link.path);
        }
        if link.adopt {
            adopt_entry(&live)?;
            report.adopted.push(link.path.clone());
        }
        let target = static_target(&link.path);
        let tmp = fsx::sibling(&live, ".laft-tmp");
        fsx::remove_if_present(&tmp)?;
        symlink(&target, &tmp).map_err(io(&tmp))?;
        fsx::commit(&tmp, &live)?;
        report.linked.push(link.path.clone());
    }
    for copy in &plan.copies {
        let live = paths.etc.join(&copy.path);
        if copy.replace_dir {
            remove_vacated(&live)?;
            forget_dirs(&mut dirs, &copy.path);
        }
        if copy.adopt {
            adopt_entry(&live)?;
            report.adopted.push(copy.path.clone());
        }
        if !copy.current {
            fsx::write_atomic(&live, &copy.bytes, copy.mode, copy.owner)?;
            report.copied.push(copy.path.clone());
        }
        copies.finish(&copy.path, copy.hash);
    }
    for path in &plan.stale {
        let live = paths.etc.join(path);
        fsx::remove_if_present(&live)?;
        fsx::sync_dir(fsx::parent(&live))?;
    }
    let declared = |path: &String| record.copy.contains(path) || DATABASES.contains(&path.as_str());
    copies.files.retain(|path, _| declared(path));
    copies.pending.retain(|path, _| declared(path));

    let mut kept: BTreeSet<&str> = plan.managed.iter().map(String::as_str).collect();
    kept.extend(record.copy.iter().map(String::as_str));
    for dir in dirs.clone().iter().rev() {
        let below = format!("{dir}/");
        if kept.iter().any(|p| p.starts_with(&below)) {
            continue;
        }
        let live = paths.etc.join(dir);
        if remove_empty_dir(&live)? {
            dirs.remove(dir);
        }
    }

    ManagedRecord {
        generation: Some(generation),
        managed: plan.managed,
        dirs,
        ..ManagedRecord::default()
    }
    .save(root)?;
    copies.save(root)?;
    Ok(report)
}

/// Makes each of `paths`, absolute or relative to `/etc`, match the
/// generation `/etc` was last reconciled with, and returns the paths whose
/// previous content was kept as `<path>.laft-old`.
///
/// A managed path becomes the symlink into the static tree again, which
/// ends the warnings about a user replaced file. A copy-mode file is
/// rewritten with the generation's content. Content Laft wrote itself is
/// replaced without a backup. Each new entry is prepared next to the live
/// one before the live one is touched, and the backup is a second link to
/// the live entry, so the live path never goes missing.
///
/// # Errors
///
/// Fails with [`Error::NotResettable`] before changing anything when a path
/// is not below `/etc`, is not managed or copied by that generation, is an
/// account database, or has content to keep while `<path>.laft-old` exists
/// already. Fails with [`Error::Invalid`] when `/etc` was never reconciled,
/// and with an I/O error when the static tree cannot be read or `/etc`
/// cannot be changed.
pub fn reset_etc(lock: &Lock, paths: &[String]) -> Result<Vec<String>> {
    let root = lock.root();
    let mut previous = ManagedRecord::load(root)?;
    let generation = previous
        .generation
        .ok_or_else(|| Error::Invalid("/etc has not been activated yet".into()))?;
    let record = root.generation(generation)?.etc;
    let mut copies = CopiesRecord::load(root)?;
    let dirs = self::paths(root, generation);
    let mut resets = Vec::new();
    for given in paths {
        let not_resettable = |path: &str, reason| Error::NotResettable {
            path: path.to_owned(),
            reason,
        };
        let Some(path) = relative(given) else {
            return Err(not_resettable(given, "the path is not below /etc"));
        };
        if DATABASES.contains(&path.as_str()) {
            return Err(not_resettable(
                &path,
                "the account databases are written from the declared accounts at activation",
            ));
        }
        let live = dirs.etc.join(&path);
        let metadata = fsx::lstat(&live)?;
        let (copy, backup) = if record.managed.contains(&path) {
            let target = static_target(&path);
            if metadata.is_some() && link_target(&live)?.as_deref() == Some(&target) {
                previous.managed.insert(path);
                continue;
            }
            (None, metadata.is_some())
        } else if record.copy.contains(&path) {
            let copy = static_copy(&dirs, &path)?;
            let laft_content = match &metadata {
                Some(m) if m.is_file() => {
                    let hash = hash_file(&live)?;
                    hash == copy.hash || copies.wrote(&path, &hash)
                }
                _ => false,
            };
            (Some(copy), metadata.is_some() && !laft_content)
        } else {
            return Err(not_resettable(
                &path,
                "the active configuration does not manage this path",
            ));
        };
        if backup && fsx::lstat(&fsx::sibling(&live, ".laft-old"))?.is_some() {
            return Err(not_resettable(
                &path,
                "the backup <path>.laft-old exists already, so move it away first",
            ));
        }
        resets.push((path, copy, backup));
    }

    for (path, copy, _) in &resets {
        if let Some(copy) = copy {
            copies.pending.insert(path.clone(), copy.hash);
        }
    }
    copies.save(root)?;
    let mut kept = Vec::new();
    for (path, copy, backup) in resets {
        let live = dirs.etc.join(&path);
        let parent = fsx::parent(&live);
        fs::create_dir_all(parent).map_err(io(parent))?;
        let tmp = fsx::sibling(&live, ".laft-tmp");
        match &copy {
            Some(copy) => fsx::stage(&tmp, &copy.bytes, copy.mode, copy.owner)?,
            None => {
                fsx::remove_if_present(&tmp)?;
                symlink(static_target(&path), &tmp).map_err(io(&tmp))?;
            }
        }
        if backup {
            keep_aside(&live, &fsx::sibling(&live, ".laft-old"))?;
            kept.push(path.clone());
        }
        fsx::commit(&tmp, &live)?;
        match copy {
            Some(copy) => copies.finish(&path, copy.hash),
            None => {
                previous.managed.insert(path);
            }
        }
    }
    previous.save(root)?;
    copies.save(root)?;
    Ok(kept)
}

/// Keeps the entry at `live` as `old`, which must not exist. A file or
/// symlink gets a second link, so it stays at `live` until it is replaced.
/// A directory cannot, so it is moved.
fn keep_aside(live: &Path, old: &Path) -> Result<()> {
    let is_dir = fsx::lstat(live)?.is_some_and(|m| m.is_dir());
    if is_dir {
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            live,
            rustix::fs::CWD,
            old,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|e| io(live)(e.into()))?;
    } else {
        fs::hard_link(live, old).map_err(io(old))?;
    }
    fsx::sync_dir(fsx::parent(live))
}

fn paths(root: &Root, generation: u64) -> Paths {
    Paths {
        etc: state::etc_dir(root),
        static_etc: state::gen_dir(root, generation).join("etc"),
    }
}

/// `path` relative to `/etc`, whether it is given absolute or relative, or
/// `None` for an absolute path that is not below `/etc`.
fn relative(path: &str) -> Option<String> {
    let path = match path.strip_prefix('/') {
        Some(absolute) => absolute.strip_prefix("etc/")?,
        None => path,
    };
    Some(path.trim_matches('/').to_owned())
}

/// `paths` relative to `/etc`.
///
/// # Errors
///
/// Fails with [`Error::Invalid`] for an absolute path not below `/etc`.
fn relative_set(paths: &BTreeSet<String>) -> Result<BTreeSet<String>> {
    paths
        .iter()
        .map(|p| relative(p).ok_or_else(|| Error::Invalid(format!("{p} is not below /etc"))))
        .collect()
}

/// The target of the managed symlink for `path`.
fn static_target(path: &str) -> String {
    format!("/etc/static/{path}")
}

/// The target of the symlink `path`, or `None` when it is not a symlink.
fn link_target(path: &Path) -> Result<Option<String>> {
    match fsx::lstat(path)? {
        Some(m) if m.file_type().is_symlink() => {
            let target = fs::read_link(path).map_err(io(path))?;
            Ok(Some(target.to_string_lossy().into_owned()))
        }
        _ => Ok(None),
    }
}

/// Moves `path` to `<path>.laft-orig`.
fn adopt_entry(path: &Path) -> Result<()> {
    let orig = fsx::sibling(path, ".laft-orig");
    fs::rename(path, &orig).map_err(io(path))?;
    fsx::sync_dir(fsx::parent(path))
}

/// Whether the live directory `rel` was created by Laft, as `created`
/// records, and holds nothing but managed symlinks in `unlinking` and
/// directories that are vacated in the same way. Such a directory may be
/// replaced without a conflict once the symlinks are removed.
fn vacated(
    paths: &Paths,
    rel: &str,
    unlinking: &BTreeSet<&str>,
    created: &BTreeSet<String>,
) -> Result<bool> {
    if !created.contains(rel) {
        return Ok(false);
    }
    let dir = paths.etc.join(rel);
    for item in fs::read_dir(&dir).map_err(io(&dir))? {
        let item = item.map_err(io(&dir))?;
        let child = format!("{rel}/{}", item.file_name().to_string_lossy());
        let gone = if item.file_type().map_err(io(&dir))?.is_dir() {
            vacated(paths, &child, unlinking, created)?
        } else {
            unlinking.contains(child.as_str())
        };
        if !gone {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Removes the vacated directory `path` and the empty directories below
/// it. Anything else that appeared there makes the removal fail.
fn remove_vacated(path: &Path) -> Result<()> {
    for item in fs::read_dir(path).map_err(io(path))? {
        let item = item.map_err(io(path))?;
        if item.file_type().map_err(io(path))?.is_dir() {
            remove_vacated(&item.path())?;
        }
    }
    fs::remove_dir(path).map_err(io(path))?;
    fsx::sync_dir(fsx::parent(path))
}

/// Removes `path` and every directory below it from the directories Laft
/// created.
fn forget_dirs(dirs: &mut BTreeSet<String>, path: &str) {
    let below = format!("{path}/");
    dirs.retain(|dir| dir != path && !dir.starts_with(&below));
}

/// Removes the directory `path` when it exists and is empty, and returns
/// whether it is gone. A directory that something else replaced is gone
/// too.
fn remove_empty_dir(path: &Path) -> Result<bool> {
    match fs::read_dir(path) {
        Ok(mut items) => {
            if items.next().is_some() {
                return Ok(false);
            }
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(true);
        }
        Err(e) => return Err(io(path)(e)),
    }
    fs::remove_dir(path).map_err(io(path))?;
    fsx::sync_dir(fsx::parent(path))?;
    Ok(true)
}

/// The content, mode and owner of the copy-mode file `path` in the static
/// tree.
fn static_copy(paths: &Paths, path: &str) -> Result<Copy> {
    let source = paths.static_etc.join(path);
    let metadata = fs::symlink_metadata(&source).map_err(io(&source))?;
    if !metadata.is_file() {
        return Err(corrupt(&source, "a copy-mode entry is not a regular file"));
    }
    let bytes = fs::read(&source).map_err(io(&source))?;
    Ok(Copy {
        path: path.to_owned(),
        hash: Blake3Hash::of(&bytes),
        bytes,
        mode: metadata.mode() & 0o777,
        owner: (metadata.uid(), metadata.gid()),
        adopt: false,
        current: false,
        replace_dir: false,
    })
}

/// Decides what reconciling would do. Nothing is changed.
fn plan(
    paths: &Paths,
    record: &EtcRecord,
    previous: &ManagedRecord,
    copies: &CopiesRecord,
    adopt: &BTreeSet<String>,
) -> Result<Plan> {
    let mut plan = Plan::default();
    let managed: BTreeSet<&str> = record.managed.iter().map(String::as_str).collect();
    for path in previous
        .managed
        .difference(&record.managed.iter().cloned().collect())
    {
        let live = paths.etc.join(path);
        if link_target(&live)?.as_deref() == Some(&static_target(path)) {
            plan.unlink.push(path.clone());
        }
    }
    let unlinking: BTreeSet<&str> = plan.unlink.iter().map(String::as_str).collect();

    for path in &record.managed {
        let conflict = |kind| Conflict {
            path: path.clone(),
            kind,
        };
        if let Some(kind) = check_parents(paths, path, &unlinking, &mut plan.new_dirs)? {
            plan.conflicts.push(conflict(kind));
            continue;
        }
        let live = paths.etc.join(path);
        let Some(metadata) = fsx::lstat(&live)? else {
            plan.links.push(Link {
                path: path.clone(),
                adopt: false,
                replace_dir: false,
            });
            plan.managed.insert(path.clone());
            continue;
        };
        let target = link_target(&live)?;
        if target.as_deref() == Some(&static_target(path)) {
            plan.managed.insert(path.clone());
            continue;
        }
        let adopting = adopt.contains(path);
        if adopting && fsx::lstat(&fsx::sibling(&live, ".laft-orig"))?.is_some() {
            plan.conflicts.push(conflict(ConflictKind::OrigExists));
            continue;
        }
        let laft_copy = metadata.is_file() && copies.wrote(path, &hash_file(&live)?);
        let replace_dir = metadata.is_dir() && vacated(paths, path, &unlinking, &previous.dirs)?;
        if adopting || laft_copy || replace_dir {
            plan.links.push(Link {
                path: path.clone(),
                adopt: adopting && !replace_dir,
                replace_dir,
            });
            plan.managed.insert(path.clone());
        } else if let Some(target) = target {
            plan.conflicts
                .push(conflict(ConflictKind::ForeignLink(target)));
        } else if previous.managed.contains(path) {
            plan.warnings.push(Warning::UserReplaced(path.clone()));
            plan.managed.insert(path.clone());
        } else {
            plan.conflicts.push(conflict(ConflictKind::Unmanaged));
        }
    }

    for path in &record.copy {
        if DATABASES.contains(&path.as_str()) {
            continue;
        }
        let conflict = |kind| Conflict {
            path: path.clone(),
            kind,
        };
        if let Some(kind) = check_parents(paths, path, &unlinking, &mut plan.new_dirs)? {
            plan.conflicts.push(conflict(kind));
            continue;
        }
        let mut copy = static_copy(paths, path)?;
        let live = paths.etc.join(path);
        let Some(metadata) = fsx::lstat(&live)? else {
            plan.copies.push(copy);
            continue;
        };
        let adopting = adopt.contains(path);
        let target = link_target(&live)?;
        let current = if metadata.is_file() {
            Some(hash_file(&live)?)
        } else {
            None
        };
        let written_by_laft = current.is_some_and(|hash| copies.wrote(path, &hash))
            || unlinking.contains(path.as_str());
        if current == Some(copy.hash) {
            copy.current = true;
        } else if metadata.is_dir() && vacated(paths, path, &unlinking, &previous.dirs)? {
            copy.replace_dir = true;
        } else if adopting {
            if fsx::lstat(&fsx::sibling(&live, ".laft-orig"))?.is_some() {
                plan.conflicts.push(conflict(ConflictKind::OrigExists));
                continue;
            }
            copy.adopt = true;
        } else if !written_by_laft {
            let kind = match target {
                Some(target) => ConflictKind::ForeignLink(target),
                None if copies.files.contains_key(path) => ConflictKind::Changed,
                None => ConflictKind::Unmanaged,
            };
            plan.conflicts.push(conflict(kind));
            continue;
        }
        plan.copies.push(copy);
    }

    for (path, hash) in &copies.files {
        if DATABASES.contains(&path.as_str())
            || record.copy.contains(path)
            || managed.contains(path.as_str())
        {
            continue;
        }
        let live = paths.etc.join(path);
        if fsx::lstat(&live)?.is_some_and(|m| m.is_file()) {
            if hash_file(&live)? == *hash {
                plan.stale.push(path.clone());
            } else {
                plan.warnings.push(Warning::ChangedCopyKept(path.clone()));
            }
        }
    }
    Ok(plan)
}

/// Checks the directories above `path` in the live `/etc`. A missing one is
/// added to `new_dirs`. A managed symlink that is about to be removed counts
/// as missing. Returns why the path cannot be placed, if it cannot.
fn check_parents(
    paths: &Paths,
    path: &str,
    unlinking: &BTreeSet<&str>,
    new_dirs: &mut BTreeSet<String>,
) -> Result<Option<ConflictKind>> {
    let mut parent = String::new();
    let components: Vec<&str> = path.split('/').collect();
    for component in &components[..components.len().saturating_sub(1)] {
        if !parent.is_empty() {
            parent.push('/');
        }
        parent.push_str(component);
        if new_dirs.contains(&parent) {
            continue;
        }
        match fsx::lstat(&paths.etc.join(&parent))? {
            Some(m) if m.is_dir() => {}
            Some(_) if !unlinking.contains(parent.as_str()) => {
                return Ok(Some(ConflictKind::NotDirectory(parent)));
            }
            _ => {
                new_dirs.insert(parent.clone());
            }
        }
    }
    Ok(None)
}

fn hash_file(path: &Path) -> Result<Blake3Hash> {
    fs::read(path)
        .map(|bytes| Blake3Hash::of(&bytes))
        .map_err(io(path))
}

#[cfg(test)]
#[path = "etc_tests.rs"]
mod tests;
