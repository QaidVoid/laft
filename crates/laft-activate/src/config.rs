//! The parts of the system configuration that shape `/etc` and the account
//! databases, as plain values.
//!
//! Every type deserializes from the evaluated configuration. Fields that the
//! configuration leaves out take the defaults documented on them. Other
//! fields of the system configuration are ignored, so the whole evaluated
//! system value can be read into [`EtcConfig`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

/// Everything that `/etc` and the account databases are rendered from.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct EtcConfig {
    /// The host name. Default `laft`.
    pub hostname: String,
    /// The IANA time zone name. Default `UTC`.
    pub timezone: String,
    /// The value of `LANG` for logins and services. Default `C.UTF-8`.
    pub locale: String,
    /// The console keymap, if any.
    pub keymap: Option<String>,
    /// The console font, if any.
    pub console_font: Option<String>,
    /// The declared accounts.
    pub users: Users,
    /// The declared files in `/etc`, by absolute path.
    pub files: BTreeMap<String, File>,
    /// Kernel modules to load, blacklist and configure.
    pub kernel: Kernel,
    /// Kernel parameters, by key such as `vm.swappiness`.
    pub sysctl: BTreeMap<String, Value>,
}

impl Default for EtcConfig {
    fn default() -> Self {
        Self {
            hostname: "laft".into(),
            timezone: "UTC".into(),
            locale: "C.UTF-8".into(),
            keymap: None,
            console_font: None,
            users: Users::default(),
            files: BTreeMap::new(),
            kernel: Kernel::default(),
            sysctl: BTreeMap::new(),
        }
    }
}

/// The declared accounts and how they combine with accounts created outside
/// Laft.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Users {
    /// With `true`, accounts created outside Laft are kept. With `false`,
    /// the account databases contain exactly the declared accounts. Default
    /// `true`.
    pub mutable: bool,
    /// The declared users, by name.
    pub accounts: BTreeMap<String, User>,
    /// The declared groups, by name.
    pub groups: BTreeMap<String, Group>,
}

impl Default for Users {
    fn default() -> Self {
        Self {
            mutable: true,
            accounts: BTreeMap::new(),
            groups: BTreeMap::new(),
        }
    }
}

/// A declared user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct User {
    /// The uid. Allocated when `None`.
    pub uid: Option<u32>,
    /// The primary group. With `None`, the group with the user's name, which
    /// is created when it is not declared.
    pub group: Option<String>,
    /// Supplementary groups.
    pub groups: BTreeSet<String>,
    /// The description, the comment field of `/etc/passwd`.
    pub description: String,
    /// The home directory. With `None`, `/home/<name>`, or `/var/empty` for
    /// system accounts.
    pub home: Option<String>,
    /// Whether activation creates the home directory when it is missing.
    /// With `None`, true for regular accounts and false for system accounts.
    pub create_home: Option<bool>,
    /// The login shell. With `None`, `/usr/bin/bash`, or `/usr/bin/nologin`
    /// for system accounts.
    pub shell: Option<String>,
    /// Whether this is a system account, which takes its ids from the
    /// system range.
    pub system: bool,
    /// A crypt(3) hash, or `!` to lock the account.
    pub password_hash: Option<String>,
    /// The absolute path of a file holding the hash, read at activation.
    pub password_file: Option<String>,
    /// OpenSSH public keys that may log in as the user.
    pub ssh_keys: Vec<String>,
}

/// A declared group.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Group {
    /// The gid. Allocated when `None`.
    pub gid: Option<u32>,
    /// Members in addition to the users that list this group in their
    /// `groups`.
    pub members: BTreeSet<String>,
    /// Whether this is a system group, which takes its id from the system
    /// range.
    pub system: bool,
}

/// A declared file in `/etc`.
///
/// When enabled, exactly one content field is set.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct File {
    /// The content, verbatim.
    pub text: Option<String>,
    /// A file whose bytes are the content, read during assembly.
    pub source: Option<PathBuf>,
    /// Makes the entry a symlink with this target, stored verbatim.
    pub link: Option<String>,
    /// A value rendered as JSON, pretty printed with two spaces.
    pub json: Option<Value>,
    /// A table rendered as TOML.
    pub toml: Option<Value>,
    /// A value rendered as YAML.
    pub yaml: Option<Value>,
    /// Sections of `key=value` lines, by section name. The section named
    /// `""` is written first, without a header.
    pub ini: Option<BTreeMap<String, BTreeMap<String, Value>>>,
    /// `KEY=value` lines readable by POSIX shells.
    pub env: Option<BTreeMap<String, String>>,
    /// The permission bits, at most `0o777`. Default `0o644`.
    pub mode: u32,
    /// The owning user. Default `root`.
    pub owner: String,
    /// The owning group. Default `root`.
    pub group: String,
    /// Copies the file into `/etc` instead of linking it through
    /// `/etc/static`. Default `false`.
    pub copy: bool,
    /// `false` removes the entry, including a package default or a derived
    /// file at the same path. Default `true`.
    pub enable: bool,
}

impl Default for File {
    fn default() -> Self {
        Self {
            text: None,
            source: None,
            link: None,
            json: None,
            toml: None,
            yaml: None,
            ini: None,
            env: None,
            mode: 0o644,
            owner: "root".into(),
            group: "root".into(),
            copy: false,
            enable: true,
        }
    }
}

/// Kernel modules to load, blacklist and configure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Kernel {
    /// Modules loaded during boot, in this order.
    pub modules: Vec<String>,
    /// Modules never loaded automatically.
    pub blacklist: BTreeSet<String>,
    /// Module parameters by module, for example `power_save=1`.
    pub options: BTreeMap<String, String>,
}
