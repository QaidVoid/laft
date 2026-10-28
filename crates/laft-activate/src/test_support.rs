//! Roots, accounts and generations shared by the unit tests. Everything
//! happens in a temporary directory.

use std::fs;
use std::path::{Path, PathBuf};

use laft_store::{Lock, Root};
use tempfile::TempDir;

use crate::{Group, User, Users};

/// A temporary root.
pub(crate) struct Env {
    _dir: TempDir,
    pub(crate) root: Root,
}

impl Env {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("can create a temporary directory");
        let root = Root::new(dir.path());
        fs::create_dir_all(dir.path().join("etc")).expect("can create etc");
        Self { _dir: dir, root }
    }

    pub(crate) fn lock(&self) -> Lock {
        self.root.lock("test").expect("the lock is free")
    }

    /// The path of `relative` below the root.
    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    /// The text of `relative` below the root.
    pub(crate) fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path(relative)).expect("the file is readable")
    }

    /// Writes `text` to `relative` below the root, creating directories.
    pub(crate) fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().expect("has a parent")).expect("can create dirs");
        fs::write(path, text).expect("can write");
    }
}

/// `root`, `wheel`, `shadow`, `audio`, `users`, `sshd`, `nobody` and
/// `nogroup`, as the base part declares them.
pub(crate) fn base_users() -> Users {
    let mut users = Users::default();
    users.accounts.insert(
        "root".into(),
        User {
            uid: Some(0),
            home: Some("/root".into()),
            create_home: Some(false),
            ..User::default()
        },
    );
    users.accounts.insert("sshd".into(), system_user());
    users.accounts.insert(
        "nobody".into(),
        User {
            group: Some("nogroup".into()),
            ..system_user()
        },
    );
    for name in [
        "root", "wheel", "shadow", "audio", "users", "sshd", "nogroup",
    ] {
        users.groups.insert(
            name.into(),
            Group {
                system: name != "root",
                ..Group::default()
            },
        );
    }
    users
}

fn system_user() -> User {
    User {
        system: true,
        ..User::default()
    }
}

/// The base accounts plus `extra` regular users.
pub(crate) fn users_with(extra: &[(&str, User)]) -> Users {
    let mut users = base_users();
    for (name, user) in extra {
        users.accounts.insert((*name).to_owned(), user.clone());
    }
    users
}

/// Whether some line of `text` starts with `name:`.
pub(crate) fn has_entry(text: &str, name: &str) -> bool {
    text.lines().any(|l| l.starts_with(&format!("{name}:")))
}

/// The permission bits of `path`.
pub(crate) fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path)
        .expect("exists")
        .permissions()
        .mode()
        & 0o7777
}

/// Declares `users` for generation `number`, as assembly does, and stores
/// the declared accounts in the generation directory.
pub(crate) fn declare_generation(lock: &Lock, number: u64, users: &Users) -> crate::Declared {
    let declared = crate::declare(lock, users).expect("the accounts are valid");
    let dir = crate::state::gen_dir(lock.root(), number);
    fs::create_dir_all(&dir).expect("can create the generation");
    declared.write(&dir).expect("can write the accounts");
    declared
}
