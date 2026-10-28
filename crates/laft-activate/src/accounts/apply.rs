//! Writing the account databases at activation.

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, chown};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use laft_core::{Blake3Hash, system_gid};
use laft_store::{Lock, Root};
use rustix::fs::FlockOperation;

use super::db::Databases;
use super::merge::merge;
use super::{Allocations, Declared, Password, valid_hash};
use crate::error::{Error, Result, Warning, io};
use crate::fsx;
use crate::state::{self, CopiesRecord, Record};

/// How long to wait for another program to release the account database
/// lock.
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

/// What writing the account databases changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountsReport {
    /// Users that are no longer in the databases.
    pub removed_users: Vec<String>,
    /// Groups that are no longer in the databases.
    pub removed_groups: Vec<String>,
    /// Home directories that were created, as absolute paths.
    pub created_homes: Vec<String>,
    /// Things the user should know about.
    pub warnings: Vec<Warning>,
}

/// Writes `/etc/passwd`, `/etc/group`, `/etc/shadow` and `/etc/gshadow`
/// from the accounts that generation `generation` declares.
///
/// The databases are locked the way `lckpwdf(3)` locks them, so that tools
/// such as `passwd` and `useradd` never interleave with Laft. Declared
/// accounts are enforced. When the generation's accounts are mutable,
/// undeclared accounts are kept, except those Laft managed before, which
/// are removed. So activating an older generation removes the accounts
/// that only a newer one declared, while their ids stay recorded.
///
/// Each database is written to `<file>+` and synced, the old one is kept
/// as `<file>-`, and the new one is renamed into place. `passwd` and
/// `group` get mode `0o644` and owner `root:root`, `shadow` and `gshadow`
/// mode `0o640` and owner `root:shadow`. Their hashes are recorded in
/// `laft/state/etc-copies.json`. Finally, missing home directories of
/// declared users that want one are created with mode `0o700`. Homes are
/// never changed or removed.
///
/// # Errors
///
/// Fails with [`Error::LockTimeout`] when the lock is not acquired within
/// 15 seconds, with [`Error::Malformed`] when a database cannot be parsed,
/// with [`Error::IdCollision`] when an undeclared account has a declared
/// id, and with [`Error::Secret`] or [`Error::InvalidHash`] for a bad
/// password file. Nothing is written in those cases.
pub fn apply_accounts(lock: &Lock, generation: u64) -> Result<AccountsReport> {
    let root = lock.root();
    let declared = Declared::read(&state::gen_dir(root, generation))?;
    write_accounts(root, &declared)
}

fn write_accounts(root: &Root, declared: &Declared) -> Result<AccountsReport> {
    let etc = state::etc_dir(root);
    let guard = lock_databases(&etc)?;
    let current = Databases::read(&etc)?;
    let hashes = read_hashes(root, declared)?;
    let mut allocations = Allocations::load(root)?;
    let merged = merge(declared, &hashes, &allocations, current)?;

    let rendered = merged.databases.render();
    let mut copies = CopiesRecord::load(root)?;
    for (name, text) in &rendered {
        copies
            .pending
            .insert((*name).to_owned(), Blake3Hash::of(text.as_bytes()));
    }
    copies.save(root)?;
    let shadow_gid = system_gid("shadow")
        .ok_or_else(|| Error::Invalid("the static accounts have no shadow group".into()))?;
    for (name, text) in &rendered {
        let (mode, owner) = match *name {
            "shadow" | "gshadow" => (0o640, (0, shadow_gid)),
            _ => (0o644, (0, 0)),
        };
        write_database(&etc.join(name), text.as_bytes(), mode, owner)?;
    }
    for (name, user) in &declared.users {
        allocations.users.entry(name.clone()).or_insert(user.uid);
    }
    for (name, group) in &declared.groups {
        allocations.groups.entry(name.clone()).or_insert(group.gid);
    }
    allocations.managed_users = declared.users.keys().cloned().collect();
    allocations.managed_groups = declared.groups.keys().cloned().collect();
    allocations.save(root)?;
    for (name, text) in &rendered {
        copies.finish(name, Blake3Hash::of(text.as_bytes()));
    }
    copies.save(root)?;
    drop(guard);

    let mut created_homes = Vec::new();
    for user in declared.users.values() {
        if user.create_home && create_home(root.path(), &user.home, (user.uid, user.gid))? {
            created_homes.push(user.home.clone());
        }
    }
    Ok(AccountsReport {
        removed_users: merged.removed_users,
        removed_groups: merged.removed_groups,
        created_homes,
        warnings: merged.warnings,
    })
}

/// Takes the write lock on `/etc/.pwd.lock` that `lckpwdf(3)` takes,
/// waiting up to [`LOCK_TIMEOUT`]. The lock is released when the returned
/// file is closed.
fn lock_databases(etc: &Path) -> Result<File> {
    fs::create_dir_all(etc).map_err(io(etc))?;
    let path = etc.join(".pwd.lock");
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(io(&path))?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match rustix::fs::fcntl_lock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => return Ok(file),
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::ACCESS) => {
                if Instant::now() >= deadline {
                    return Err(Error::LockTimeout(path));
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(io(&path)(e.into())),
        }
    }
}

/// The password hash of every user that declares one, reading password
/// files below `root`.
fn read_hashes(root: &Root, declared: &Declared) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    for (name, user) in &declared.users {
        let hash = match &user.password {
            Password::Unset => continue,
            Password::Hash { hash } => hash.clone(),
            Password::File { path } => read_secret(&fsx::under(root.path(), path))?,
        };
        hashes.insert(name.clone(), hash);
    }
    Ok(hashes)
}

/// Reads a password hash from the secret file `path`, which must belong to
/// the user running Laft, root on a real system, and be inaccessible to
/// its group and others. One trailing line feed is removed.
fn read_secret(path: &Path) -> Result<String> {
    let secret = |reason| Error::Secret {
        path: path.to_path_buf(),
        reason,
    };
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(secret("the password file does not exist"));
        }
        Err(e) => return Err(io(path)(e)),
    };
    let metadata = file.metadata().map_err(io(path))?;
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(secret("the password file is not owned by root"));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(secret(
            "the password file is accessible by its group or others",
        ));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|_| Error::InvalidHash(path.to_path_buf()))?;
    let hash = text.strip_suffix('\n').unwrap_or(&text);
    if !valid_hash(hash) {
        return Err(Error::InvalidHash(path.to_path_buf()));
    }
    Ok(hash.to_owned())
}

/// Replaces the database `path`: the new content goes to `<path>+` and is
/// synced, the old content is kept as `<path>-`, then `<path>+` is renamed
/// to `path`.
fn write_database(path: &Path, bytes: &[u8], mode: u32, owner: fsx::Owner) -> Result<()> {
    let staged = fsx::sibling(path, "+");
    fsx::stage(&staged, bytes, mode, owner)?;
    match fs::read(path) {
        Ok(old) => fsx::write_atomic(&fsx::sibling(path, "-"), &old, mode, owner)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(path)(e)),
    }
    fsx::commit(&staged, path)
}

/// Creates the home directory `home` below `root` with mode `0o700` and
/// `owner`, unless something exists there. Returns whether it was created.
fn create_home(root: &Path, home: &str, owner: fsx::Owner) -> Result<bool> {
    let path = fsx::under(root, home);
    if fsx::lstat(&path)?.is_some() {
        return Ok(false);
    }
    let parent = fsx::parent(&path);
    fs::create_dir_all(parent).map_err(io(parent))?;
    DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(io(&path))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(io(&path))?;
    if fsx::is_root() {
        chown(&path, Some(owner.0), Some(owner.1)).map_err(io(&path))?;
    }
    fsx::sync_dir(parent)?;
    Ok(true)
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
