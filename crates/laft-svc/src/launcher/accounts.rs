//! Accounts, read from `/etc/passwd` and `/etc/group` directly, because
//! the launcher never loads name service modules.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::error::{Error, corrupt, io};
use crate::spec::{Spec, SpecDir};

/// A user account.
#[derive(Debug, Clone, PartialEq, Eq)]
struct User {
    uid: u32,
    gid: u32,
    home: String,
    shell: String,
}

/// The account databases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Accounts {
    users: BTreeMap<String, User>,
    groups: BTreeMap<String, u32>,
}

/// Who a service runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Credentials {
    pub(crate) user: String,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    /// The supplementary groups, including the primary group, sorted.
    pub(crate) groups: Vec<u32>,
    pub(crate) home: String,
    pub(crate) shell: String,
    /// Whether the service declares a group or supplementary groups. A
    /// service running as root gets its groups changed only then.
    pub(crate) declares_groups: bool,
}

impl Accounts {
    /// Reads `etc/passwd` and `etc/group` below `root`.
    pub(crate) fn read(root: &Path) -> Result<Self, Error> {
        let passwd = root.join("etc/passwd");
        let group = root.join("etc/group");
        let passwd_text = fs::read_to_string(&passwd).map_err(io(&passwd))?;
        let group_text = fs::read_to_string(&group).map_err(io(&group))?;
        Self::parse(&passwd_text, &passwd, &group_text, &group)
    }

    pub(crate) fn parse(
        passwd: &str,
        passwd_path: &Path,
        group: &str,
        group_path: &Path,
    ) -> Result<Self, Error> {
        let mut accounts = Self::default();
        for (number, line) in lines(passwd) {
            let fields: Vec<&str> = line.split(':').collect();
            let [name, _, uid, gid, _, home, shell] = fields[..] else {
                return Err(corrupt(
                    passwd_path,
                    format!("line {number}: expected 7 fields"),
                ));
            };
            let user = User {
                uid: id(uid, passwd_path, number)?,
                gid: id(gid, passwd_path, number)?,
                home: home.to_owned(),
                shell: shell.to_owned(),
            };
            accounts.users.entry(name.to_owned()).or_insert(user);
        }
        for (number, line) in lines(group) {
            let fields: Vec<&str> = line.split(':').collect();
            let [name, _, gid, _] = fields[..] else {
                return Err(corrupt(
                    group_path,
                    format!("line {number}: expected 4 fields"),
                ));
            };
            let gid = id(gid, group_path, number)?;
            accounts.groups.entry(name.to_owned()).or_insert(gid);
        }
        Ok(accounts)
    }

    /// The credentials of the service: its user, its group or the user's
    /// primary group, and its supplementary groups plus that group.
    pub(crate) fn credentials(&self, spec: &Spec) -> Result<Credentials, Error> {
        let user = self.user(spec, &spec.user)?;
        let gid = match &spec.group {
            Some(group) => self.group(spec, group)?,
            None => user.gid,
        };
        let mut groups = vec![gid];
        for group in &spec.groups {
            groups.push(self.group(spec, group)?);
        }
        groups.sort_unstable();
        groups.dedup();
        Ok(Credentials {
            user: spec.user.clone(),
            uid: user.uid,
            gid,
            groups,
            home: user.home.clone(),
            shell: user.shell.clone(),
            declares_groups: spec.group.is_some() || !spec.groups.is_empty(),
        })
    }

    /// The owner and group of a declared directory. The owner defaults to
    /// the service user. The group defaults to the primary group of the
    /// declared owner, or to the service's group when no owner is declared.
    pub(crate) fn dir_owner(
        &self,
        spec: &Spec,
        credentials: &Credentials,
        dir: &SpecDir,
    ) -> Result<(u32, u32), Error> {
        let (uid, owner_gid) = match &dir.owner {
            Some(owner) => {
                let user = self.user(spec, owner)?;
                (user.uid, user.gid)
            }
            None => (credentials.uid, credentials.gid),
        };
        let gid = match &dir.group {
            Some(group) => self.group(spec, group)?,
            None => owner_gid,
        };
        Ok((uid, gid))
    }

    fn user(&self, spec: &Spec, name: &str) -> Result<&User, Error> {
        self.users.get(name).ok_or_else(|| Error::UnknownAccount {
            name: spec.name.clone(),
            kind: "user",
            account: name.to_owned(),
        })
    }

    fn group(&self, spec: &Spec, name: &str) -> Result<u32, Error> {
        self.groups
            .get(name)
            .copied()
            .ok_or_else(|| Error::UnknownAccount {
                name: spec.name.clone(),
                kind: "group",
                account: name.to_owned(),
            })
    }
}

/// The non-empty lines of an account database with their 1-based numbers.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line))
        .filter(|(_, line)| !line.is_empty())
}

fn id(text: &str, path: &Path, number: usize) -> Result<u32, Error> {
    text.parse()
        .map_err(|_| corrupt(path, format!("line {number}: invalid id {text:?}")))
}

#[cfg(test)]
#[path = "accounts_tests.rs"]
mod tests;
