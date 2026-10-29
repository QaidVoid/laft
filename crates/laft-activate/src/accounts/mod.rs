//! Declared accounts, id allocation and the account databases.
//!
//! [`declare`] turns the configured accounts into [`Declared`] accounts with
//! fixed ids, recording every allocation in [`Allocations`] so that ids
//! never change between generations. A generation stores its declared
//! accounts, and [`apply_accounts`] writes `/etc/passwd`, `/etc/group`,
//! `/etc/shadow` and `/etc/gshadow` from them at activation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, corrupt, io};
use crate::state::{self, Record};

mod apply;
mod db;
mod declare;
mod merge;

pub use apply::{AccountsReport, apply_accounts};
pub use declare::declare;
#[cfg(test)]
pub(crate) use declare::resolve;

/// Checks `users` with the rules of [`declare`], as if no id had ever been
/// allocated and the account databases were empty.
pub(crate) fn check(users: &crate::Users) -> Vec<crate::ConfigError> {
    declare::resolve(
        users,
        &mut Allocations::default(),
        &db::Databases::default(),
    )
    .err()
    .unwrap_or_default()
}

/// The name of the declared accounts file in a generation directory.
pub(crate) const DECLARED_FILE: &str = "accounts.json";

/// The account databases, relative to `/etc`, which only the accounts step
/// writes.
pub(crate) const DATABASES: [&str; 4] = ["group", "gshadow", "passwd", "shadow"];

/// The accounts a generation declares, with every id resolved.
///
/// It is stored as `accounts.json` in the generation directory with mode
/// `0o600`, because it can contain password hashes, and activation reads it
/// again at rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declared {
    /// The format version, 1.
    pub version: u32,
    /// Whether accounts created outside Laft are kept.
    pub mutable: bool,
    /// The users, by name.
    pub users: BTreeMap<String, DeclaredUser>,
    /// The groups, by name, including implicit primary groups.
    pub groups: BTreeMap<String, DeclaredGroup>,
}

/// A user with every default applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredUser {
    /// The uid.
    pub uid: u32,
    /// The gid of the primary group.
    pub gid: u32,
    /// The comment field.
    pub description: String,
    /// The home directory.
    pub home: String,
    /// Whether activation creates the home directory when it is missing.
    pub create_home: bool,
    /// The login shell.
    pub shell: String,
    /// Where the password hash comes from.
    pub password: Password,
}

/// Where the password hash of a user comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Password {
    /// Not declared. An existing hash is kept, and a new account is locked.
    Unset,
    /// This hash.
    Hash {
        /// A crypt(3) hash, `!` or `*`.
        hash: String,
    },
    /// The hash in this file, read at activation.
    File {
        /// The absolute path of the file.
        path: String,
    },
}

/// A group with every default applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredGroup {
    /// The gid.
    pub gid: u32,
    /// The members: the users that list the group plus its declared
    /// members.
    pub members: BTreeSet<String>,
}

impl Declared {
    /// Writes the accounts as `accounts.json` into `generation_dir`, a
    /// generation tree under construction, with mode `0o600`.
    ///
    /// The file is not synced, because a generation is published with one
    /// rename after its whole tree is synced.
    ///
    /// # Errors
    ///
    /// Fails when the file exists already or cannot be written.
    pub fn write(&self, generation_dir: &Path) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let path = generation_dir.join(DECLARED_FILE);
        let bytes = laft_core::to_canonical_json(self)
            .map_err(|e| corrupt(&path, format!("not canonical JSON: {e}")))?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut file| file.write_all(&bytes))
            .map_err(io(&path))
    }

    /// Reads the accounts from `accounts.json` in `generation_dir`.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or is not a supported version.
    pub fn read(generation_dir: &Path) -> Result<Self> {
        let path = generation_dir.join(DECLARED_FILE);
        let bytes = fs::read(&path).map_err(io(&path))?;
        let declared: Self = serde_json::from_slice(&bytes).map_err(|e| corrupt(&path, e))?;
        if declared.version != state::FORMAT {
            return Err(corrupt(&path, "unsupported format version"));
        }
        declared.check().map_err(|reason| corrupt(&path, reason))?;
        Ok(declared)
    }

    /// Checks every value that reaches the account databases or the file
    /// system against the rules [`declare`] enforces, so that a damaged
    /// record cannot produce malformed databases or paths outside the root.
    fn check(&self) -> std::result::Result<(), String> {
        for (name, user) in &self.users {
            if !valid_name(name) {
                return Err(format!("invalid user name {name:?}"));
            }
            if user.description.contains([':', '\n']) {
                return Err(format!("user {name:?}: invalid description"));
            }
            let file = match &user.password {
                Password::Unset => None,
                Password::Hash { hash } if valid_hash(hash) => None,
                Password::Hash { .. } => return Err(format!("user {name:?}: invalid hash")),
                Password::File { path } => Some(path),
            };
            for path in [&user.home, &user.shell].into_iter().chain(file) {
                if !valid_path(path) {
                    return Err(format!("user {name:?}: invalid path {path:?}"));
                }
            }
        }
        for (name, group) in &self.groups {
            if !valid_name(name) || !group.members.iter().all(|m| valid_name(m)) {
                return Err(format!("invalid group {name:?}"));
            }
        }
        Ok(())
    }
}

/// Every id Laft allocated and the accounts it manages, stored as
/// `laft/state/accounts.json`.
///
/// Recorded ids are never given to a different name. Removing an account
/// keeps its id recorded, so declaring it again restores the same id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allocations {
    /// The format version, 1.
    pub version: u32,
    /// The uid of every user Laft ever declared.
    pub users: BTreeMap<String, u32>,
    /// The gid of every group Laft ever declared.
    pub groups: BTreeMap<String, u32>,
    /// The users in the account databases Laft last wrote.
    pub managed_users: BTreeSet<String>,
    /// The groups in the account databases Laft last wrote.
    pub managed_groups: BTreeSet<String>,
}

impl Default for Allocations {
    fn default() -> Self {
        Self {
            version: state::FORMAT,
            users: BTreeMap::new(),
            groups: BTreeMap::new(),
            managed_users: BTreeSet::new(),
            managed_groups: BTreeSet::new(),
        }
    }
}

impl Record for Allocations {
    const NAME: &'static str = "accounts.json";

    fn version(&self) -> u32 {
        self.version
    }
}

impl Allocations {
    /// Reads the allocations of `root`, or returns empty ones when none are
    /// recorded.
    ///
    /// # Errors
    ///
    /// Fails when the file exists but cannot be read or parsed.
    pub fn load(root: &laft_store::Root) -> Result<Self> {
        <Self as Record>::load(root)
    }
}

/// Whether `name` is a valid user or group name: a lowercase letter or `_`,
/// then up to 31 of `a-z`, `0-9`, `_` and `-`.
pub(crate) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Whether `path` is a normalized absolute path that an account database
/// can store: `/`, or `/` followed by components that are neither empty,
/// `.` nor `..`, without a colon or a line break.
pub(crate) fn valid_path(path: &str) -> bool {
    if path.contains([':', '\n', '\r']) {
        return false;
    }
    match path.strip_prefix('/') {
        Some("") => true,
        Some(rest) => rest
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != ".."),
        None => false,
    }
}

/// Whether `hash` is a valid password field for `/etc/shadow`: `!`, `*`, or
/// an optionally locked crypt(3) hash of a supported method, without a
/// colon or a line break.
pub(crate) fn valid_hash(hash: &str) -> bool {
    if hash.contains([':', '\n', '\r']) {
        return false;
    }
    if hash == "!" || hash == "*" {
        return true;
    }
    let hash = hash.strip_prefix('!').unwrap_or(hash);
    ["$y$", "$gy$", "$6$", "$5$", "$2b$"].iter().any(|prefix| {
        hash.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty())
    })
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
