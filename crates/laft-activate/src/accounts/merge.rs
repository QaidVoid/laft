//! Merging the declared accounts into the current account databases.

use std::collections::{BTreeMap, BTreeSet};

use super::db::{Databases, GroupLine, Gshadow, Passwd, Shadow};
use super::{Allocations, Declared};
use crate::error::{Error, Result, Warning};

/// The new databases and what changed.
#[derive(Debug)]
pub(crate) struct Merged {
    pub(crate) databases: Databases,
    /// Users in the current databases that are gone, in their order.
    pub(crate) removed_users: Vec<String>,
    /// Groups in the current databases that are gone, in their order.
    pub(crate) removed_groups: Vec<String>,
    pub(crate) warnings: Vec<Warning>,
}

/// Merges `declared` into the `current` databases.
///
/// When the accounts are mutable, the result starts from the current
/// entries minus those Laft managed before (`managed`) and no longer
/// declares. Otherwise it starts empty. Declared entries then replace the
/// fields Laft owns in an existing entry of the same name, or are appended
/// in ascending id order. In a declared group, the memberships of users
/// Laft manages are exactly the declared ones, and memberships of other
/// users are kept. `hashes` holds the password hash of every user that
/// declares one. Other users keep their current hash, and new ones are
/// locked. When the accounts are mutable, a declared group keeps the
/// password and administrators of its `gshadow` entry. Otherwise both are
/// reset.
///
/// Fails with [`Error::IdCollision`] when a kept, undeclared entry has the
/// id of a declared one.
pub(crate) fn merge(
    declared: &Declared,
    hashes: &BTreeMap<String, String>,
    managed: &Allocations,
    current: Databases,
) -> Result<Merged> {
    let mut warnings = Vec::new();
    let keep = |name: &str, is_declared: bool, managed: &BTreeSet<String>| {
        declared.mutable && (is_declared || !managed.contains(name))
    };

    let mut passwd = Vec::new();
    let mut removed_users = Vec::new();
    for entry in current.passwd {
        let is_declared = declared.users.contains_key(&entry.name);
        if keep(&entry.name, is_declared, &managed.managed_users) {
            passwd.push(entry);
        } else if !is_declared {
            if !managed.managed_users.contains(&entry.name) {
                warnings.push(Warning::RemovedAccount {
                    kind: "user",
                    name: entry.name.clone(),
                });
            }
            removed_users.push(entry.name);
        }
    }
    for entry in &passwd {
        if declared.users.contains_key(&entry.name) {
            continue;
        }
        if let Some((name, _)) = declared.users.iter().find(|(_, u)| u.uid == entry.uid) {
            return Err(Error::IdCollision {
                kind: "user",
                declared: name.clone(),
                existing: entry.name.clone(),
                id: entry.uid,
            });
        }
    }
    let mut added = Vec::new();
    for (name, user) in &declared.users {
        match passwd.iter_mut().find(|p| &p.name == name) {
            Some(entry) => {
                entry.uid = user.uid;
                entry.gid = user.gid;
                entry.gecos.clone_from(&user.description);
                entry.home.clone_from(&user.home);
                entry.shell.clone_from(&user.shell);
            }
            None => added.push(Passwd {
                name: name.clone(),
                password: "x".into(),
                uid: user.uid,
                gid: user.gid,
                gecos: user.description.clone(),
                home: user.home.clone(),
                shell: user.shell.clone(),
            }),
        }
    }
    added.sort_by_key(|p| p.uid);
    passwd.extend(added);

    let gone: BTreeSet<&str> = removed_users.iter().map(String::as_str).collect();
    let enforced: BTreeSet<&str> = managed
        .managed_users
        .iter()
        .chain(declared.users.keys())
        .map(String::as_str)
        .collect();
    let mut group = Vec::new();
    let mut removed_groups = Vec::new();
    for mut entry in current.group {
        let is_declared = declared.groups.contains_key(&entry.name);
        if keep(&entry.name, is_declared, &managed.managed_groups) {
            entry.members.retain(|m| !gone.contains(m.as_str()));
            group.push(entry);
        } else if !is_declared {
            if !managed.managed_groups.contains(&entry.name) {
                warnings.push(Warning::RemovedAccount {
                    kind: "group",
                    name: entry.name.clone(),
                });
            }
            removed_groups.push(entry.name);
        }
    }
    for entry in &group {
        if declared.groups.contains_key(&entry.name) {
            continue;
        }
        if let Some((name, _)) = declared.groups.iter().find(|(_, g)| g.gid == entry.gid) {
            return Err(Error::IdCollision {
                kind: "group",
                declared: name.clone(),
                existing: entry.name.clone(),
                id: entry.gid,
            });
        }
    }
    let mut added = Vec::new();
    for (name, declared_group) in &declared.groups {
        match group.iter_mut().find(|g| &g.name == name) {
            Some(entry) => {
                entry.gid = declared_group.gid;
                entry.members.retain(|m| !enforced.contains(m.as_str()));
                entry.members.extend(declared_group.members.iter().cloned());
            }
            None => added.push(GroupLine {
                name: name.clone(),
                password: "x".into(),
                gid: declared_group.gid,
                members: declared_group.members.iter().cloned().collect(),
            }),
        }
    }
    added.sort_by_key(|g| g.gid);
    group.extend(added);

    let mut old_shadow: BTreeMap<String, Shadow> = current
        .shadow
        .into_iter()
        .map(|s| (s.name.clone(), s))
        .collect();
    let shadow = passwd
        .iter()
        .map(|p| {
            let existing = old_shadow.remove(&p.name);
            match (hashes.get(&p.name), existing) {
                (Some(hash), Some(mut entry)) => {
                    entry.hash.clone_from(hash);
                    entry
                }
                (Some(hash), None) => new_shadow(&p.name, hash),
                (None, Some(entry)) => entry,
                // A hash kept in `passwd` itself stays valid. A new shadow
                // entry must carry it, because the shadow hash wins.
                (None, None) if !matches!(p.password.as_str(), "x" | "") => {
                    new_shadow(&p.name, &p.password)
                }
                (None, None) => new_shadow(&p.name, "!"),
            }
        })
        .collect();

    let mut old_gshadow: BTreeMap<String, Gshadow> = current
        .gshadow
        .into_iter()
        .map(|g| (g.name.clone(), g))
        .collect();
    let gshadow = group
        .iter()
        .map(|g| {
            let fresh = || Gshadow {
                name: g.name.clone(),
                password: "!".into(),
                admins: String::new(),
                members: g.members.clone(),
            };
            match old_gshadow.remove(&g.name) {
                Some(entry) if declared.groups.contains_key(&g.name) && declared.mutable => {
                    Gshadow {
                        members: g.members.clone(),
                        ..entry
                    }
                }
                Some(_) if declared.groups.contains_key(&g.name) => fresh(),
                Some(mut entry) => {
                    entry.members.retain(|m| !gone.contains(m.as_str()));
                    entry
                }
                None => fresh(),
            }
        })
        .collect();

    Ok(Merged {
        databases: Databases {
            passwd,
            group,
            shadow,
            gshadow,
        },
        removed_users,
        removed_groups,
        warnings,
    })
}

/// A new shadow entry with `hash` and empty aging fields.
fn new_shadow(name: &str, hash: &str) -> Shadow {
    Shadow {
        name: name.to_owned(),
        hash: hash.to_owned(),
        aging: vec![String::new(); 7],
    }
}
