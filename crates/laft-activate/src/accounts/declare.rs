//! Resolution of the configured accounts: defaults, implicit primary groups,
//! memberships and stable id allocation.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;

use laft_core::{system_gid, system_uid};
use laft_store::Lock;

use super::db::Databases;
use super::{
    Allocations, Declared, DeclaredGroup, DeclaredUser, Password, valid_hash, valid_name,
    valid_path,
};
use crate::config::Users;
use crate::error::{ConfigError, Error, Result};
use crate::state::{self, Record};

/// The ids allocated to system accounts that do not set one.
const SYSTEM_IDS: RangeInclusive<u32> = 101..=999;
/// The ids allocated to regular accounts that do not set one.
const REGULAR_IDS: RangeInclusive<u32> = 1000..=59999;

/// Resolves the configured accounts of `users` into declared accounts with
/// fixed ids, and records every id in `laft/state/accounts.json` before
/// returning.
///
/// An account without an explicit id keeps the id recorded for its name, or
/// the id it has in the current `/etc/passwd` or `/etc/group`. Otherwise it
/// gets the lowest free id of its range: 101 to 999 for system accounts and
/// 1000 to 59999 for others. An id is free when it is not recorded, not in
/// the current databases and not declared by another account. A user's
/// private group takes the user's uid when that gid is free. The static
/// system accounts always have their fixed ids.
///
/// A user without a primary group gets the group with its name, which is
/// created when it is not declared.
///
/// ```no_run
/// use laft_activate::{Users, declare};
/// use laft_store::Root;
///
/// let root = Root::new("/mnt");
/// let lock = root.lock("apply")?;
/// let declared = declare(&lock, &Users::default())?;
/// # Ok::<(), laft_activate::Error>(())
/// ```
///
/// # Errors
///
/// Fails with [`Error::Config`] listing every invalid account, with
/// [`Error::Malformed`] when a current database cannot be parsed, and when
/// the allocations cannot be read or written.
pub fn declare(lock: &Lock, users: &Users) -> Result<Declared> {
    let root = lock.root();
    let current = Databases::read(&state::etc_dir(root))?;
    let mut allocations = Allocations::load(root)?;
    let declared = resolve(users, &mut allocations, &current).map_err(Error::Config)?;
    allocations.save(root)?;
    Ok(declared)
}

/// What an account asks for when its id is assigned.
struct Want<'a> {
    name: &'a str,
    explicit: Option<u32>,
    system: bool,
    /// An id to take before the lowest free one, when it is free.
    prefer: Option<u32>,
}

/// Resolves `users` against the recorded `allocations` and the `current`
/// databases, and records every resolved id in `allocations`.
pub(crate) fn resolve(
    users: &Users,
    allocations: &mut Allocations,
    current: &Databases,
) -> Result<Declared, Vec<ConfigError>> {
    let mut errors = Vec::new();
    let groups = effective_groups(users, &mut errors);
    check_users(users, &groups, &mut errors);
    for (name, group) in &users.groups {
        for member in &group.members {
            if !users.accounts.contains_key(member) {
                errors.push(ConfigError::UnknownAccount {
                    kind: "user",
                    name: member.clone(),
                    context: format!("group {name:?}"),
                });
            }
        }
    }

    let user_wants: Vec<Want<'_>> = users
        .accounts
        .iter()
        .map(|(name, user)| Want {
            name,
            explicit: user.uid,
            system: user.system,
            prefer: None,
        })
        .collect();
    let current_uids = current
        .passwd
        .iter()
        .map(|p| (p.name.clone(), p.uid))
        .collect();
    let uids = assign(
        "user",
        &user_wants,
        system_uid,
        &allocations.users,
        &current_uids,
        &mut errors,
    );

    let group_wants: Vec<Want<'_>> = groups
        .iter()
        .map(|(name, group)| Want {
            name,
            explicit: group.gid,
            system: group.system,
            prefer: group.private_of.and_then(|user| uids.get(user).copied()),
        })
        .collect();
    let current_gids = current
        .group
        .iter()
        .map(|g| (g.name.clone(), g.gid))
        .collect();
    let gids = assign(
        "group",
        &group_wants,
        system_gid,
        &allocations.groups,
        &current_gids,
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut declared = Declared {
        version: state::FORMAT,
        mutable: users.mutable,
        users: BTreeMap::new(),
        groups: BTreeMap::new(),
    };
    for (name, user) in &users.accounts {
        let primary = user.group.as_deref().unwrap_or(name);
        let password = match (&user.password_hash, &user.password_file) {
            (Some(hash), _) => Password::Hash { hash: hash.clone() },
            (None, Some(path)) => Password::File { path: path.clone() },
            (None, None) => Password::Unset,
        };
        let (home, shell) = if user.system {
            ("/var/empty".to_owned(), "/usr/bin/nologin")
        } else {
            (format!("/home/{name}"), "/usr/bin/bash")
        };
        declared.users.insert(
            name.clone(),
            DeclaredUser {
                uid: uids[name],
                gid: gids[primary],
                description: user.description.clone(),
                home: user.home.clone().unwrap_or(home),
                create_home: user.create_home.unwrap_or(!user.system),
                shell: user.shell.clone().unwrap_or_else(|| shell.to_owned()),
                password,
            },
        );
    }
    for (name, group) in &groups {
        let mut members: BTreeSet<String> = group.members.clone();
        for (user_name, user) in &users.accounts {
            if user.groups.contains(name) {
                members.insert(user_name.clone());
            }
        }
        declared.groups.insert(
            name.clone(),
            DeclaredGroup {
                gid: gids[name],
                members,
            },
        );
    }
    allocations.users.extend(uids);
    allocations.groups.extend(gids);
    Ok(declared)
}

/// A group as declared or implied by a user without a primary group.
struct EffectiveGroup<'a> {
    gid: Option<u32>,
    system: bool,
    members: BTreeSet<String>,
    /// The user whose primary group this is because it has the user's
    /// name.
    private_of: Option<&'a str>,
}

/// The declared groups plus the implicit primary groups.
fn effective_groups<'a>(
    users: &'a Users,
    errors: &mut Vec<ConfigError>,
) -> BTreeMap<String, EffectiveGroup<'a>> {
    let mut groups: BTreeMap<String, EffectiveGroup<'a>> = users
        .groups
        .iter()
        .map(|(name, group)| {
            let group = EffectiveGroup {
                gid: group.gid,
                system: group.system || system_gid(name).is_some(),
                members: group.members.clone(),
                private_of: None,
            };
            (name.clone(), group)
        })
        .collect();
    for (name, user) in &users.accounts {
        if user.group.is_some() {
            continue;
        }
        match groups.get_mut(name) {
            Some(group) => {
                let regular = !user.system && system_uid(name).is_none();
                if group.system && regular {
                    errors.push(ConfigError::SystemPrimaryGroup(name.clone()));
                }
                group.private_of = Some(name);
            }
            None => {
                let group = EffectiveGroup {
                    gid: None,
                    system: user.system,
                    members: BTreeSet::new(),
                    private_of: Some(name),
                };
                groups.insert(name.clone(), group);
            }
        }
    }
    for name in groups.keys() {
        if !valid_name(name) {
            errors.push(ConfigError::InvalidName {
                kind: "group",
                name: name.clone(),
            });
        }
    }
    groups
}

/// Checks every user's name, fields and references.
fn check_users(
    users: &Users,
    groups: &BTreeMap<String, EffectiveGroup<'_>>,
    errors: &mut Vec<ConfigError>,
) {
    for (name, user) in &users.accounts {
        if !valid_name(name) {
            errors.push(ConfigError::InvalidName {
                kind: "user",
                name: name.clone(),
            });
        }
        if user.password_hash.is_some() && user.password_file.is_some() {
            errors.push(ConfigError::PasswordSources(name.clone()));
        }
        let invalid = |field: &str, reason| ConfigError::InvalidValue {
            context: format!("user {name:?}: {field}"),
            reason,
        };
        if user
            .password_hash
            .as_deref()
            .is_some_and(|h| !valid_hash(h))
        {
            errors.push(invalid(
                "password_hash",
                "expected a crypt(3) hash of a supported method, `!` or `*`",
            ));
        }
        if user.description.contains([':', '\n']) {
            errors.push(invalid(
                "description",
                "a colon or a line break cannot be stored",
            ));
        }
        for (field, value) in [
            ("home", &user.home),
            ("shell", &user.shell),
            ("password_file", &user.password_file),
        ] {
            if value.as_deref().is_some_and(|v| !valid_path(v)) {
                errors.push(invalid(
                    field,
                    "expected a normalized absolute path without a colon or a line break",
                ));
            }
        }
        let referenced = user.group.iter().chain(&user.groups);
        for group in referenced {
            if !groups.contains_key(group) {
                errors.push(ConfigError::UnknownAccount {
                    kind: "group",
                    name: group.clone(),
                    context: format!("user {name:?}"),
                });
            }
        }
    }
}

/// The ids assigned so far.
struct Taken<'a> {
    kind: &'static str,
    ids: BTreeMap<String, u32>,
    owners: BTreeMap<u32, &'a str>,
}

impl<'a> Taken<'a> {
    fn claim(&mut self, name: &'a str, id: u32, errors: &mut Vec<ConfigError>) {
        if let Some(other) = self.owners.get(&id) {
            errors.push(ConfigError::DuplicateId {
                kind: self.kind,
                id,
                names: [(*other).to_owned(), name.to_owned()],
            });
        } else {
            self.owners.insert(id, name);
        }
        self.ids.insert(name.to_owned(), id);
    }
}

/// Assigns an id to every account in `wants`, in passes: fixed ids of the
/// static accounts and explicit ids first, then recorded ids and ids the
/// current databases give the same name, then preferred ids, then the
/// lowest free id of each account's range.
fn assign<'a>(
    kind: &'static str,
    wants: &[Want<'a>],
    registry: fn(&str) -> Option<u32>,
    recorded: &BTreeMap<String, u32>,
    current: &BTreeMap<String, u32>,
    errors: &mut Vec<ConfigError>,
) -> BTreeMap<String, u32> {
    let mut taken = Taken {
        kind,
        ids: BTreeMap::new(),
        owners: BTreeMap::new(),
    };
    for want in wants {
        let name = want.name.to_owned();
        match (registry(want.name), want.explicit) {
            (Some(fixed), Some(id)) if id != fixed => {
                errors.push(ConfigError::RegistryId {
                    kind,
                    name,
                    id,
                    fixed,
                });
            }
            (Some(id), _) => taken.claim(want.name, id, errors),
            // Ids up to 100 and 65534 belong to the static accounts.
            (None, Some(id)) if id <= 100 || id == 65534 => {
                errors.push(ConfigError::ReservedId { kind, name, id });
            }
            (None, Some(id)) => taken.claim(want.name, id, errors),
            (None, None) => {}
        }
    }
    let unassigned = |taken: &Taken<'a>| -> Vec<&Want<'a>> {
        wants
            .iter()
            .filter(|w| registry(w.name).is_none() && w.explicit.is_none())
            .filter(|w| !taken.ids.contains_key(w.name))
            .collect()
    };
    for want in unassigned(&taken) {
        let existing = current.get(want.name).filter(|&&id| {
            !recorded
                .iter()
                .any(|(other, &recorded)| recorded == id && other != want.name)
        });
        if let Some(&id) = recorded.get(want.name).or(existing) {
            taken.claim(want.name, id, errors);
        }
    }
    let recorded_ids: BTreeSet<u32> = recorded.values().copied().collect();
    let current_ids: BTreeSet<u32> = current.values().copied().collect();
    let is_free = |id: u32, taken: &Taken<'_>| {
        !taken.owners.contains_key(&id) && !recorded_ids.contains(&id) && !current_ids.contains(&id)
    };
    for want in unassigned(&taken) {
        if let Some(id) = want.prefer.filter(|&id| is_free(id, &taken)) {
            taken.claim(want.name, id, errors);
        }
    }
    for want in unassigned(&taken) {
        let mut range = if want.system { SYSTEM_IDS } else { REGULAR_IDS };
        match range.find(|&id| is_free(id, &taken)) {
            Some(id) => taken.claim(want.name, id, errors),
            None => errors.push(ConfigError::IdsExhausted {
                kind,
                name: want.name.to_owned(),
            }),
        }
    }
    taken.ids
}

#[cfg(test)]
#[path = "declare_tests.rs"]
mod tests;
