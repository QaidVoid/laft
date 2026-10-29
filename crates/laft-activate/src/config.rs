//! The parts of the system configuration that shape `/etc` and the account
//! databases, as plain values.
//!
//! Every type deserializes from the evaluated configuration. Fields that the
//! configuration leaves out take the defaults documented on them. Other
//! fields of the system configuration are ignored, so the whole evaluated
//! system value can be read into [`EtcConfig`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use laft_core::LuksOption;
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
    /// The mounted filesystems, by mountpoint, for `/etc/fstab`.
    pub fs: BTreeMap<String, Fs>,
    /// The swap devices, by device, for `/etc/fstab`.
    pub swap: BTreeMap<String, Swap>,
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
            fs: BTreeMap::new(),
            swap: BTreeMap::new(),
        }
    }
}

/// The declared accounts and how they combine with accounts created outside
/// Laft.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

/// The parts of the system configuration that shape the initramfs, the
/// boot files and the boot menu.
///
/// Like [`EtcConfig`], it deserializes from the whole evaluated system
/// value and ignores the fields of other subsystems.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BootConfig {
    /// The boot loader, kernel and boot counting settings.
    pub boot: Boot,
    /// The mounted filesystems, by mountpoint.
    pub fs: BTreeMap<String, Fs>,
    /// The LUKS devices unlocked in the initramfs, by device-mapper name.
    pub luks: BTreeMap<String, Luks>,
}

/// The boot loader, kernel and boot counting settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Boot {
    /// The boot loader.
    pub loader: Loader,
    /// The mountpoint of the EFI system partition. Default `/boot`.
    pub esp: String,
    /// The package that provides the kernel. Default `linux`.
    pub kernel: String,
    /// The kernel command line after `laft.gen=<N>`. Default `quiet` and
    /// `loglevel=3`.
    pub cmdline: Vec<String>,
    /// The menu timeout in seconds, or `None` to wait forever. Default 3.
    pub timeout: Option<u32>,
    /// Whether menu entries may be edited at boot. Default `false`.
    pub editor: bool,
    /// Boot attempts before falling back to the last good generation. 0
    /// disables boot counting. Default 3.
    pub tries: u32,
    /// Which CPU microcode the initramfs carries. Default `auto`.
    pub microcode: Microcode,
    /// The initramfs settings.
    pub initrd: Initrd,
}

impl Default for Boot {
    fn default() -> Self {
        Self {
            loader: Loader::Limine,
            esp: "/boot".into(),
            kernel: "linux".into(),
            cmdline: vec!["quiet".into(), "loglevel=3".into()],
            timeout: Some(3),
            editor: false,
            tries: 3,
            microcode: Microcode::Auto,
            initrd: Initrd::default(),
        }
    }
}

/// A boot loader Laft can install.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    /// Limine, the only one.
    #[default]
    Limine,
}

/// Which CPU microcode the initramfs carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Microcode {
    /// The vendor of the machine running the assembly when the initramfs is
    /// host only, otherwise both.
    #[default]
    Auto,
    /// Intel microcode.
    Intel,
    /// AMD microcode.
    Amd,
    /// Intel and AMD microcode.
    Both,
    /// None.
    Off,
}

/// The initramfs settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Initrd {
    /// Extra kernel modules, by name.
    pub modules: BTreeSet<String>,
    /// Whether to include only the modules the boot devices of the
    /// assembling machine need, instead of every storage and input driver.
    /// Default `true`.
    pub hostonly: bool,
    /// Whether to include the emergency shell. Default `true`.
    pub shell: bool,
    /// Seconds to wait for the boot devices. Default 30.
    pub root_wait: u32,
}

impl Default for Initrd {
    fn default() -> Self {
        Self {
            modules: BTreeSet::new(),
            hostonly: true,
            shell: true,
            root_wait: 30,
        }
    }
}

/// A mounted filesystem.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fs {
    /// The device, such as `UUID=...` or `/dev/mapper/cryptroot`.
    pub device: Option<String>,
    /// The filesystem type, such as `ext4` or `vfat`. The configuration's
    /// `other(name = "zfs")` reads as its name.
    #[serde(deserialize_with = "fs_type")]
    pub fstype: String,
    /// The mount options.
    pub options: Vec<String>,
    /// Whether the filesystem is checked before it is mounted. With `None`,
    /// true for ext2, ext3, ext4, f2fs and vfat.
    pub fsck: Option<bool>,
}

/// Reads a filesystem type: a name, or the variant `other` with its
/// `name`.
fn fs_type<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FsType {
        Name(String),
        Other { other: OtherFs },
    }
    #[derive(Deserialize)]
    struct OtherFs {
        name: String,
    }
    Ok(match FsType::deserialize(deserializer)? {
        FsType::Name(name) => name,
        FsType::Other { other } => other.name,
    })
}

/// A swap device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Swap {
    /// The swap priority, or `None` for the kernel's choice.
    pub priority: Option<i64>,
}

/// A LUKS device unlocked in the initramfs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Luks {
    /// The encrypted device, `UUID=...` or `/dev/...`.
    pub device: String,
    /// Options for the unlocked device. An unknown option fails reading the
    /// configuration, because the initramfs would refuse it at boot.
    pub options: BTreeSet<LuksOption>,
}

/// The security settings, the `security` field of the system
/// configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Security {
    /// The files that keep setuid, setgid or file capabilities in the
    /// system generation, by absolute path.
    pub privileged: BTreeMap<String, Privileged>,
}

/// A file that keeps setuid, setgid or file capabilities in the system
/// generation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Privileged {
    /// Whether the entry applies. Default `true`.
    pub enable: bool,
    /// The owning user. Default `root`.
    pub owner: String,
    /// The owning group. Default `root`.
    pub group: String,
    /// The mode, at most `0o7777`. Default `0o4755`.
    pub mode: u32,
    /// File capabilities in the text syntax of `cap_from_text(3)`, such as
    /// `cap_setuid=ep`.
    pub caps: Option<String>,
}

impl Default for Privileged {
    fn default() -> Self {
        Self {
            enable: true,
            owner: "root".into(),
            group: "root".into(),
            mode: 0o4755,
            caps: None,
        }
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
