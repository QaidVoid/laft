//! The step that settles modes, owners and requested privileges.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use laft_core::{system_gid, system_uid};
use laft_pkg::PrivRequest;

use crate::Outputs;
use crate::error::{BuildError, io};
use crate::fsx::{self, Kind};
use crate::glob::Glob;

/// The extended attribute that holds file capabilities.
const CAPABILITY_XATTR: &str = "security.capability";

/// What [`privileges`] decided about a payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Privileges {
    /// The privileges the recipe asked for, sorted by path, with the modes
    /// the install step produced.
    pub requested: Vec<PrivRequest>,
    /// The regular files that were writable by everyone before their write
    /// bits were cleared.
    pub world_writable: Vec<String>,
    ownership: Vec<(Glob, Option<String>, Option<String>)>,
}

impl Privileges {
    /// The owner and group of the payload path `path`, `None` for root:
    /// the first `ownership` entry, in the order of its globs, that
    /// matches.
    pub fn owner(&self, path: &str) -> (Option<String>, Option<String>) {
        self.ownership
            .iter()
            .find(|(glob, _, _)| glob.matches(path))
            .map_or((None, None), |(_, owner, group)| {
                (owner.clone(), group.clone())
            })
    }
}

/// Settles the permissions of the payload `out`.
///
/// 1. Every path belongs to root, except where the recipe's `ownership`
///    names a `user:group` from the static system account registry
///    (`B114` otherwise). Owners are recorded by name in the manifest.
/// 2. Every file loses its write bits and every directory gets mode
///    `0o755`.
/// 3. A setuid or setgid file that the recipe's `setuid` does not list is
///    `B106`.
/// 4. Every path in `setuid` or `caps` must be a regular file (`B111`). It
///    is recorded as a requested privilege with the mode the install step
///    gave it, and loses its setuid and setgid bits.
/// 5. File capabilities are removed from every file.
///
/// # Errors
///
/// Fails with `B106`, `B111` or `B114`, and when the payload cannot be read
/// or changed.
pub fn privileges(out: &Path, outputs: &Outputs) -> Result<Privileges, BuildError> {
    let mut ownership = Vec::new();
    for (glob, owner) in &outputs.ownership {
        let (user, group) = owner.split_once(':').ok_or_else(|| {
            BuildError::problem(
                "B114",
                format!("ownership of {glob} is {owner:?}, which is not user:group"),
            )
        })?;
        if system_uid(user).is_none() || system_gid(group).is_none() {
            return Err(BuildError::problem(
                "B114",
                format!("ownership of {glob} names {owner}, which is not a static system account"),
            ));
        }
        let name = |n: &str| (n != "root").then(|| n.to_owned());
        ownership.push((Glob::new(glob), name(user), name(group)));
    }
    let mut result = Privileges {
        ownership,
        ..Privileges::default()
    };

    let declared: BTreeSet<&String> = outputs.setuid.iter().collect();
    let mut files = Vec::new();
    for (rel, kind) in fsx::walk(out)? {
        let path = out.join(&rel);
        match kind {
            Kind::Dir => fsx::set_mode(&path, 0o755)?,
            Kind::File => {
                let mode = fs::symlink_metadata(&path)
                    .map_err(io(&path))?
                    .permissions()
                    .mode()
                    & 0o7777;
                if mode & 0o6000 != 0 && !declared.contains(&rel) {
                    return Err(BuildError::problem(
                        "B106",
                        format!("{rel} is setuid but not declared in setuid"),
                    ));
                }
                if mode & 0o002 != 0 {
                    result.world_writable.push(rel.clone());
                }
                files.push((rel, mode));
            }
            _ => {}
        }
    }

    let requested: BTreeSet<&String> = outputs.setuid.iter().chain(outputs.caps.keys()).collect();
    for rel in requested {
        let Some((_, mode)) = files.iter().find(|(r, _)| r == rel) else {
            return Err(BuildError::problem(
                "B111",
                format!(
                    "{rel} is listed in setuid or caps, but is not a regular file of the payload"
                ),
            ));
        };
        let (owner, group) = result.owner(rel);
        result.requested.push(PrivRequest {
            path: rel.clone(),
            mode: *mode,
            owner,
            group,
            caps: outputs.caps.get(rel).cloned(),
        });
    }

    for (rel, mode) in &files {
        let path = out.join(rel);
        fsx::set_mode(&path, mode & !0o6222)?;
        // Removing a capability needs a privilege even when the file has
        // none, so only files that carry one are touched.
        match rustix::fs::lgetxattr_query(&path, CAPABILITY_XATTR) {
            Ok(_) => rustix::fs::lremovexattr(&path, CAPABILITY_XATTR)
                .map_err(|e| io(&path)(e.into()))?,
            Err(rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP) => {}
            Err(e) => return Err(io(&path)(e.into())),
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "privileges_tests.rs"]
mod tests;
