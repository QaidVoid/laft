//! The steps of the early boot graph that Laft runs itself, as
//! `laft svc builtin <step> <up|down>`.
//!
//! The base part of the system configuration declares one oneshot for each
//! step, and the step does exactly this:
//!
//! | Step | `up` | `down` |
//! |---|---|---|
//! | `kernel-fs` | mounts `devpts` on `/dev/pts`, `tmpfs` on `/dev/shm`, `cgroup2` on `/sys/fs/cgroup`, `securityfs` on `/sys/kernel/security`, and `efivarfs` on `/sys/firmware/efi/efivars` when the firmware is EFI, each unless something is mounted there | nothing |
//! | `kmod` | loads the modules that `/etc/modules-load.d/*.conf` and `/usr/lib/modules-load.d/*.conf` list with `modprobe -a -b` | nothing |
//! | `udev-trigger` | replays the kernel's subsystem and device events with `udevadm trigger` and waits with `udevadm settle` | nothing |
//! | `hostname` | sets the host name from the first line of `/etc/hostname` | nothing |
//! | `mount-fs` | like `mount -a`: checks every filesystem of `/etc/fstab` with a pass, then mounts every one that is not `noauto`, swap or mounted already, in file order | unmounts those filesystems in reverse order, except `/` |
//! | `random-seed` | feeds `/var/lib/laft/random-seed` to `/dev/urandom` and replaces it with a new seed | saves a new seed |
//! | `console` | loads `KEYMAP` with `loadkeys` and `FONT` with `setfont` from `/etc/vconsole.conf` | nothing |
//!
//! A step for which the base part declares no `down` does nothing on
//! `down`. Swap, kernel parameters and the clock are not built in: the
//! base part runs `swapon -a` on the swap lines of `/etc/fstab`,
//! `sysctl -p /etc/sysctl.d/laft.conf` and `hwclock`.
//!
//! Every step works below a root directory and runs programs from its
//! `usr/bin`, so tests point it at a temporary tree with stand-in programs.
//! Only the host name is set without a program, by writing
//! `proc/sys/kernel/hostname` below the root.

pub mod fstab;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use laft_core::DeviceSpec;

use self::fstab::{Entry, mountpoints, parse_fstab};

/// The file that carries the random seed across boots, below the root.
pub const RANDOM_SEED: &str = "var/lib/laft/random-seed";

/// The size of a saved random seed, in bytes.
const SEED_BYTES: usize = 512;

/// A built-in step of the early boot graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The kernel's pseudo filesystems that the initramfs leaves out.
    KernelFs,
    /// The configured kernel modules.
    Kmod,
    /// The replay of kernel device events.
    UdevTrigger,
    /// The host name.
    Hostname,
    /// The filesystems of `/etc/fstab`.
    MountFs,
    /// The random seed.
    RandomSeed,
    /// The console keymap and font.
    Console,
}

impl FromStr for Step {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Ok(match text {
            "kernel-fs" => Self::KernelFs,
            "kmod" => Self::Kmod,
            "udev-trigger" => Self::UdevTrigger,
            "hostname" => Self::Hostname,
            "mount-fs" => Self::MountFs,
            "random-seed" => Self::RandomSeed,
            "console" => Self::Console,
            _ => {
                return Err(format!(
                    "{text} is not a built-in step: kernel-fs, kmod, udev-trigger, hostname, mount-fs, random-seed or console"
                ));
            }
        })
    }
}

/// Whether a step starts or stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The service's `up`.
    Up,
    /// The service's `down`.
    Down,
}

impl FromStr for Action {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        match text {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            _ => Err(format!("{text} is neither up nor down")),
        }
    }
}

/// Why a built-in step failed.
#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed.
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A file the step reads is malformed.
    Malformed {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// A program the step runs could not start or failed.
    Program {
        /// The program and its arguments.
        command: String,
        /// Why it failed.
        reason: String,
    },
    /// Some filesystems were not checked or mounted, each with why. The
    /// others were mounted.
    Filesystems(Vec<(String, String)>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Malformed { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::Program { command, reason } => write!(f, "{command}: {reason}"),
            Self::Filesystems(failed) => {
                let list: Vec<String> = failed
                    .iter()
                    .map(|(m, why)| format!("{m}: {why}"))
                    .collect();
                write!(f, "filesystems not mounted: {}", list.join("; "))
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

/// Runs `step` for `action` on the system below `root`, which is `/` on a
/// booting machine, and returns warnings about what it left out.
///
/// # Errors
///
/// Fails as the step's function does.
pub fn run(root: &Path, step: Step, action: Action) -> Result<Vec<String>, Error> {
    match (step, action) {
        (Step::KernelFs, Action::Up) => kernel_fs(root).map(|()| Vec::new()),
        (Step::Kmod, Action::Up) => kmod(root).map(|()| Vec::new()),
        (Step::UdevTrigger, Action::Up) => udev_trigger(root).map(|()| Vec::new()),
        (Step::Hostname, Action::Up) => hostname(root).map(|()| Vec::new()),
        (Step::MountFs, Action::Up) => mount_fs(root),
        (Step::MountFs, Action::Down) => unmount_fs(root).map(|()| Vec::new()),
        (Step::RandomSeed, Action::Up) => seed_up(root).map(|()| Vec::new()),
        (Step::RandomSeed, Action::Down) => save_seed(root).map(|()| Vec::new()),
        (Step::Console, Action::Up) => console(root).map(|()| Vec::new()),
        (_, Action::Down) => Ok(Vec::new()),
    }
}

/// A mount that a step makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelMount {
    /// The filesystem type, which is also the source.
    pub fstype: &'static str,
    /// The absolute mountpoint.
    pub target: &'static str,
    /// The mount options.
    pub options: &'static str,
}

/// The kernel filesystems that `kernel-fs` mounts on the system below
/// `root`, given its mount table `mountinfo`: those whose mountpoint has
/// nothing mounted yet, with `efivarfs` only when `sys/firmware/efi`
/// exists.
pub fn kernel_mounts(root: &Path, mountinfo: &str) -> Vec<KernelMount> {
    let mounted = mountpoints(mountinfo);
    let efi = root.join("sys/firmware/efi").is_dir();
    [
        (
            "devpts",
            "/dev/pts",
            "nosuid,noexec,gid=5,mode=0620,ptmxmode=0666",
        ),
        ("tmpfs", "/dev/shm", "nosuid,nodev,mode=1777"),
        (
            "cgroup2",
            "/sys/fs/cgroup",
            "nosuid,nodev,noexec,nsdelegate",
        ),
        ("securityfs", "/sys/kernel/security", "nosuid,nodev,noexec"),
        (
            "efivarfs",
            "/sys/firmware/efi/efivars",
            "nosuid,nodev,noexec",
        ),
    ]
    .into_iter()
    .filter(|(fstype, target, _)| !mounted.contains(*target) && (*fstype != "efivarfs" || efi))
    .map(|(fstype, target, options)| KernelMount {
        fstype,
        target,
        options,
    })
    .collect()
}

/// Mounts the kernel filesystems that [`kernel_mounts`] lists, creating
/// their mountpoints, with `usr/bin/mount`.
///
/// # Errors
///
/// Fails when the mount table cannot be read, a mountpoint cannot be
/// created or a mount fails.
pub fn kernel_fs(root: &Path) -> Result<(), Error> {
    let table = root.join("proc/self/mountinfo");
    let mountinfo = fs::read_to_string(&table).map_err(io(&table))?;
    for mount in kernel_mounts(root, &mountinfo) {
        let target = below(root, mount.target);
        fs::create_dir_all(&target).map_err(io(&target))?;
        program(root, "mount")
            .args(["-t", mount.fstype, "-o", mount.options, mount.fstype])
            .arg(&target)
            .run()?;
    }
    Ok(())
}

/// The modules that the `.conf` files of `etc/modules-load.d` and
/// `usr/lib/modules-load.d` below `root` list, in the order of the file
/// names and then of the lines, each once. A file in `etc` hides the file
/// of the same name in `usr/lib`. Blank lines and lines that start with
/// `#` or `;` are skipped.
///
/// # Errors
///
/// Fails when a directory or a file cannot be read.
pub fn modules(root: &Path) -> Result<Vec<String>, Error> {
    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    for dir in ["usr/lib/modules-load.d", "etc/modules-load.d"] {
        let dir = root.join(dir);
        let items = match fs::read_dir(&dir) {
            Ok(items) => items,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io(&dir)(e)),
        };
        for item in items {
            let path = item.map_err(io(&dir))?.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.ends_with(".conf") {
                files.insert(name.to_owned(), path);
            }
        }
    }
    let mut modules: Vec<String> = Vec::new();
    for path in files.values() {
        let text = fs::read_to_string(path).map_err(io(path))?;
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if !modules.iter().any(|m| m == line) {
                modules.push(line.to_owned());
            }
        }
    }
    Ok(modules)
}

/// Loads the modules that [`modules`] lists with `usr/bin/modprobe -a -b`,
/// which leaves out blacklisted ones.
///
/// # Errors
///
/// Fails when the lists cannot be read or `modprobe` fails.
pub fn kmod(root: &Path) -> Result<(), Error> {
    let modules = modules(root)?;
    if modules.is_empty() {
        return Ok(());
    }
    program(root, "modprobe")
        .args(["-a", "-b"])
        .args(&modules)
        .run()
}

/// Replays the kernel's subsystem and device events and waits until udev
/// handled them, with `usr/bin/udevadm`.
///
/// # Errors
///
/// Fails when `udevadm` fails.
pub fn udev_trigger(root: &Path) -> Result<(), Error> {
    for kind in ["subsystems", "devices"] {
        program(root, "udevadm")
            .args(["trigger", "--action=add"])
            .arg(format!("--type={kind}"))
            .run()?;
    }
    program(root, "udevadm").arg("settle").run()
}

/// Reads the host name from the first line of `etc/hostname` below
/// `root`: 1 to 64 letters, digits, `-` and `.`.
///
/// # Errors
///
/// Fails when the file cannot be read or holds no valid host name.
pub fn read_hostname(root: &Path) -> Result<String, Error> {
    let path = root.join("etc/hostname");
    let text = fs::read_to_string(&path).map_err(io(&path))?;
    let name = text.lines().next().unwrap_or_default().trim();
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    if !valid {
        return Err(Error::Malformed {
            path,
            reason: format!("{name:?} is not a host name"),
        });
    }
    Ok(name.to_owned())
}

/// Sets the host name to [`read_hostname`] by writing it to
/// `proc/sys/kernel/hostname` below `root`.
///
/// # Errors
///
/// Fails as [`read_hostname`] does, and when the name cannot be written.
pub fn hostname(root: &Path) -> Result<(), Error> {
    let name = read_hostname(root)?;
    let path = root.join("proc/sys/kernel/hostname");
    fs::write(&path, name).map_err(io(&path))
}

/// The program that checks a filesystem of type `fstype` and its
/// arguments for an automatic repair, or `None` for types without one.
pub fn checker(fstype: &str) -> Option<(&'static str, &'static str)> {
    match fstype {
        "ext2" | "ext3" | "ext4" => Some(("e2fsck", "-p")),
        "f2fs" => Some(("fsck.f2fs", "-a")),
        "vfat" => Some(("fsck.vfat", "-a")),
        _ => None,
    }
}

/// The device file that the fstab device `device` names: a tag such as
/// `UUID=` becomes its link below `/dev/disk`, which udev creates, and
/// anything else stays as it is.
pub fn device_path(device: &str) -> String {
    match device.parse::<DeviceSpec>() {
        Ok(DeviceSpec::Uuid(id)) => format!("/dev/disk/by-uuid/{id}"),
        Ok(DeviceSpec::Label(label)) => format!("/dev/disk/by-label/{label}"),
        Ok(DeviceSpec::PartUuid(id)) => format!("/dev/disk/by-partuuid/{id}"),
        Ok(DeviceSpec::PartLabel(label)) => format!("/dev/disk/by-partlabel/{label}"),
        _ => device.to_owned(),
    }
}

/// The filesystems of the fstab `entries` that `mount-fs up` mounts, given
/// the mountpoints that are mounted already: every one that is not swap,
/// `noauto` or mounted, in file order.
pub fn to_mount<'e>(entries: &'e [Entry], mounted: &BTreeSet<String>) -> Vec<&'e Entry> {
    entries
        .iter()
        .filter(|e| !e.is_swap() && !e.is_noauto() && !mounted.contains(&e.mountpoint))
        .collect()
}

/// Reads `etc/fstab` below `root`.
fn read_fstab(root: &Path) -> Result<Vec<Entry>, Error> {
    let path = root.join("etc/fstab");
    let text = fs::read_to_string(&path).map_err(io(&path))?;
    parse_fstab(&text).map_err(|(line, reason)| Error::Malformed {
        path,
        reason: format!("line {line}: {reason}"),
    })
}

/// Reads the mountpoints of `proc/self/mountinfo` below `root`.
fn mounted(root: &Path) -> Result<BTreeSet<String>, Error> {
    let table = root.join("proc/self/mountinfo");
    Ok(mountpoints(
        &fs::read_to_string(&table).map_err(io(&table))?,
    ))
}

/// Checks and mounts the filesystems of `etc/fstab` below `root` like
/// `mount -a`, as [`to_mount`] selects them. A filesystem with a pass is
/// checked first by its [`checker`]. Exit status 0 or 1 of the checker
/// mounts it, anything else leaves it unmounted and is reported. A
/// filesystem whose checker is missing from `usr/bin` is mounted unchecked
/// with a warning. Every filesystem is mounted with `usr/bin/mount
/// <mountpoint>`, which reads its line from `/etc/fstab`.
///
/// # Errors
///
/// Fails when the fstab or the mount table cannot be read, and with
/// [`Error::Filesystems`] when a filesystem was not mounted.
pub fn mount_fs(root: &Path) -> Result<Vec<String>, Error> {
    let entries = read_fstab(root)?;
    let mounted = mounted(root)?;
    let mut warnings = Vec::new();
    let mut failed = Vec::new();
    for entry in to_mount(&entries, &mounted) {
        if entry.pass > 0
            && let Some((name, flag)) = checker(&entry.fstype)
        {
            if root.join("usr/bin").join(name).exists() {
                let device = device_path(&entry.device);
                let status = program(root, name).args([flag, &device]).status();
                match status {
                    Ok(code) if code <= 1 => {}
                    Ok(code) => {
                        failed.push((
                            entry.mountpoint.clone(),
                            format!("{name} exited with status {code}"),
                        ));
                        continue;
                    }
                    Err(e) => {
                        failed.push((entry.mountpoint.clone(), e.to_string()));
                        continue;
                    }
                }
            } else {
                warnings.push(format!(
                    "{} is mounted unchecked, because {name} is not installed",
                    entry.mountpoint
                ));
            }
        }
        let target = below(root, &entry.mountpoint);
        if let Err(e) = fs::create_dir_all(&target) {
            failed.push((entry.mountpoint.clone(), e.to_string()));
            continue;
        }
        if let Err(e) = program(root, "mount").arg(&target).run() {
            failed.push((entry.mountpoint.clone(), e.to_string()));
        }
    }
    if failed.is_empty() {
        Ok(warnings)
    } else {
        Err(Error::Filesystems(failed))
    }
}

/// Unmounts the filesystems of `etc/fstab` below `root` that are mounted,
/// except `/` and swap, in reverse file order with `usr/bin/umount`.
///
/// # Errors
///
/// Fails when the fstab or the mount table cannot be read, and with
/// [`Error::Filesystems`] when a filesystem stays mounted.
pub fn unmount_fs(root: &Path) -> Result<(), Error> {
    let entries = read_fstab(root)?;
    let mounted = mounted(root)?;
    let mut failed = Vec::new();
    for entry in entries.iter().rev() {
        if entry.is_swap() || entry.mountpoint == "/" || !mounted.contains(&entry.mountpoint) {
            continue;
        }
        if let Err(e) = program(root, "umount")
            .arg(below(root, &entry.mountpoint))
            .run()
        {
            failed.push((entry.mountpoint.clone(), e.to_string()));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::Filesystems(failed))
    }
}

/// Feeds the saved seed below `root` to `dev/urandom`, which mixes it into
/// the kernel's pool without crediting entropy, then replaces it with a
/// new one, so that the same seed is never used twice. Without a saved
/// seed it only saves one.
///
/// # Errors
///
/// Fails when the seed cannot be read, fed or replaced.
pub fn seed_up(root: &Path) -> Result<(), Error> {
    let path = root.join(RANDOM_SEED);
    match fs::read(&path) {
        Ok(seed) => {
            let urandom = root.join("dev/urandom");
            fs::OpenOptions::new()
                .write(true)
                .open(&urandom)
                .and_then(|mut f| io::Write::write_all(&mut f, &seed))
                .map_err(io(&urandom))?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(&path)(e)),
    }
    save_seed(root)
}

/// Writes a new seed of 512 random bytes from the kernel to the seed file
/// below `root`, readable only by root, with a crash safe replacement.
///
/// # Errors
///
/// Fails when no random bytes are available or the file cannot be written.
pub fn save_seed(root: &Path) -> Result<(), Error> {
    let path = root.join(RANDOM_SEED);
    let mut seed = vec![0u8; SEED_BYTES];
    let mut filled = 0;
    while filled < seed.len() {
        filled +=
            rustix::rand::getrandom(&mut seed[filled..], rustix::rand::GetRandomFlags::empty())
                .map_err(|e| io(&path)(e.into()))?;
    }
    let dir = path.parent().unwrap_or(root);
    fs::create_dir_all(dir).map_err(io(dir))?;
    let tmp = dir.join(".random-seed.tmp");
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        io::Write::write_all(&mut file, &seed)?;
        file.sync_all()?;
        fs::rename(&tmp, &path)?;
        fs::File::open(dir)?.sync_all()
    })();
    written.map_err(io(&path))
}

/// The console settings of `etc/vconsole.conf` below `root`: its `KEYMAP`
/// and `FONT`, each `None` when unset or empty. A missing file sets
/// nothing. Values may be quoted with `"`.
///
/// # Errors
///
/// Fails when the file exists but cannot be read.
pub fn vconsole(root: &Path) -> Result<(Option<String>, Option<String>), Error> {
    let path = root.join("etc/vconsole.conf");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((None, None)),
        Err(e) => return Err(io(&path)(e)),
    };
    let mut keymap = None;
    let mut font = None;
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let value = (!value.is_empty()).then(|| value.to_owned());
        match key.trim() {
            "KEYMAP" => keymap = value,
            "FONT" => font = value,
            _ => {}
        }
    }
    Ok((keymap, font))
}

/// Loads the console keymap with `usr/bin/loadkeys` and the font with
/// `usr/bin/setfont`, as [`vconsole`] reads them.
///
/// # Errors
///
/// Fails when the settings cannot be read or a program fails.
pub fn console(root: &Path) -> Result<(), Error> {
    let (keymap, font) = vconsole(root)?;
    if let Some(keymap) = keymap {
        program(root, "loadkeys").args(["-q", &keymap]).run()?;
    }
    if let Some(font) = font {
        program(root, "setfont").arg(font).run()?;
    }
    Ok(())
}

/// The absolute path `path` below `root`.
fn below(root: &Path, path: &str) -> PathBuf {
    root.join(path.trim_start_matches('/'))
}

/// A program of `usr/bin` below `root`, with only `PATH` in its
/// environment.
fn program(root: &Path, name: &str) -> Program {
    let path = root.join("usr/bin").join(name);
    let mut command = Command::new(&path);
    command.env_clear().env("PATH", root.join("usr/bin"));
    Program { command }
}

/// A command that a step runs.
struct Program {
    command: Command,
}

impl Program {
    fn arg(mut self, arg: impl AsRef<std::ffi::OsStr>) -> Self {
        self.command.arg(arg);
        self
    }

    fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.command.args(args);
        self
    }

    fn describe(&self) -> String {
        std::iter::once(self.command.get_program())
            .chain(self.command.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Runs the command and returns its exit status.
    fn status(mut self) -> Result<i32, Error> {
        let status = self.command.status().map_err(|e| Error::Program {
            command: self.describe(),
            reason: e.to_string(),
        })?;
        status.code().ok_or_else(|| Error::Program {
            command: self.describe(),
            reason: format!("killed by {status}"),
        })
    }

    /// Runs the command and fails unless it exits with status 0.
    fn run(self) -> Result<(), Error> {
        let command = self.describe();
        match self.status()? {
            0 => Ok(()),
            code => Err(Error::Program {
                command,
                reason: format!("exited with status {code}"),
            }),
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
