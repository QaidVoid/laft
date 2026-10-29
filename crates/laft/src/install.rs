//! `laft install`, `laft hardware`, and `laft rescue` with its alias
//! `laft chroot`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use laft_resolve::Upgrade;
use rustix::mount::{MountFlags, MountPropagationFlags};
use serde_json::json;

use crate::apply;
use crate::cli::ApplyArgs;
use crate::env::Env;
use crate::failure::{Failure, exit};

/// The configuration `--generate-config` writes.
const TEMPLATE: &str = "\
# The configuration of this machine. `laft options` lists every setting,
# and `laft apply` makes the system match it.
system {
  include \"./hardware.stav\"

  hostname = \"laft\"
  timezone = \"UTC\"

  packages = []

  # repo core {
  #   url = \"https://...\"
  #   key = \"RW...\"
  # }
}
";

/// The top-level symlinks of a Laft root.
const LINKS: [(&str, &str); 5] = [
    ("bin", "usr/bin"),
    ("sbin", "usr/bin"),
    ("lib", "usr/lib"),
    ("lib64", "usr/lib"),
    ("usr", "/run/laft/system/usr"),
];

/// The top-level directories of a Laft root and their modes.
const DIRS: [(&str, u32); 14] = [
    ("boot", 0o755),
    ("dev", 0o755),
    ("etc", 0o755),
    ("home", 0o755),
    ("laft", 0o755),
    ("media", 0o755),
    ("mnt", 0o755),
    ("opt", 0o755),
    ("proc", 0o555),
    ("root", 0o700),
    ("run", 0o755),
    ("srv", 0o755),
    ("sys", 0o555),
    ("tmp", 0o1777),
];

fn require_root_dir(env: &Env, command: &str) -> Result<(), Failure> {
    if env.offline_root {
        Ok(())
    } else {
        Err(Failure::new(
            exit::USAGE,
            format!("laft {command} needs the target's mountpoint in --root"),
        ))
    }
}

/// Writes `text` to the new file `path`, refusing to replace one.
fn write_new(path: &Path, text: &str) -> Result<(), Failure> {
    crate::fsx::create_new(path, text.as_bytes()).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => Failure::general(format!(
            "{} exists, and laft never overwrites it",
            path.display()
        )),
        _ => Failure::general(format!("{}: {e}", path.display())),
    })
}

/// Fails unless the target holds nothing but `boot/`, `lost+found/` and
/// `etc/laft/`.
fn check_empty(root: &Path) -> Result<(), Failure> {
    let mut extra = Vec::new();
    for item in fs::read_dir(root)? {
        let name = item?.file_name().to_string_lossy().into_owned();
        match name.as_str() {
            "boot" | "lost+found" => {}
            "etc" => {
                for inner in fs::read_dir(root.join("etc"))? {
                    let inner = inner?.file_name().to_string_lossy().into_owned();
                    if inner != "laft" {
                        extra.push(format!("etc/{inner}"));
                    }
                }
            }
            _ => extra.push(name),
        }
    }
    if extra.is_empty() {
        return Ok(());
    }
    Err(Failure::general(format!("{} is not empty", root.display()))
        .context(format!("found: {}", extra.join(", ")))
        .hint("install onto freshly made filesystems"))
}

/// Creates the top-level layout of a Laft root below `root`.
fn create_layout(root: &Path) -> Result<(), Failure> {
    for (dir, mode) in DIRS {
        let path = root.join(dir);
        fs::create_dir_all(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
    }
    for (link, target) in LINKS
        .iter()
        .chain(&[("etc/static", "/run/laft/system/etc")])
    {
        let path = root.join(link);
        if fs::symlink_metadata(&path).is_err() {
            symlink(target, &path)?;
        }
    }
    Ok(())
}

/// Copies the `.stav` files and directories of `from` into `to`.
fn copy_dir(from: &Path, to: &Path) -> Result<(), Failure> {
    fs::create_dir_all(to)?;
    for item in fs::read_dir(from)? {
        let item = item?;
        let target = to.join(item.file_name());
        if item.file_type()?.is_dir() {
            copy_dir(&item.path(), &target)?;
        } else {
            fs::copy(item.path(), &target)?;
        }
    }
    Ok(())
}

/// `laft install`.
pub(crate) fn install(env: &Env, generate_config: bool) -> Result<u8, Failure> {
    require_root_dir(env, "install")?;
    let root = env.root.path().to_path_buf();
    let config_dir = root.join("etc/laft");
    if generate_config {
        write_new(&config_dir.join("system.stav"), TEMPLATE)?;
        write_new(
            &config_dir.join("hardware.stav"),
            &hardware_stav(&probe(env)),
        )?;
        env.ui.out(format!(
            "wrote {} and {}",
            config_dir.join("system.stav").display(),
            config_dir.join("hardware.stav").display()
        ));
        return Ok(0);
    }
    check_empty(&root)?;
    // A configuration that does not load fails the install below with
    // its diagnostics, so the precheck falls back to the default ESP.
    let esp = env
        .load(&|_| Ok(()))
        .map_or_else(|_| env.esp(None), |loaded| env.esp(Some(&loaded.config)));
    laft_activate::check_esp(&esp)?;
    create_layout(&root)?;
    let source = env.config_dir();
    let installed_config = fs::canonicalize(&config_dir).unwrap_or(config_dir.clone());
    if source != installed_config {
        copy_dir(&source, &config_dir)?;
    }
    let target = Env {
        config: config_dir.join("system.stav"),
        ..env.clone()
    };
    let code = apply::apply(
        &target,
        &apply::Request {
            command: "install",
            args: ApplyArgs::default(),
            upgrade: Upgrade::None,
            rebuild: BTreeSet::new(),
        },
    )?;
    let dir = root.display();
    env.ui.note(format!(
        "next steps:\n  laft rescue --root {dir} -- passwd root\n  umount -R {dir}\n  reboot"
    ));
    Ok(code)
}

/// A filesystem of the machine, as `hardware.stav` declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbedFs {
    /// The mountpoint inside the system.
    pub(crate) mountpoint: String,
    /// The device, as `UUID=...` when it has one.
    pub(crate) device: String,
    /// The filesystem type.
    pub(crate) fstype: String,
}

/// What `laft hardware` found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Probe {
    /// The filesystems, by mountpoint.
    pub(crate) filesystems: Vec<ProbedFs>,
    /// The swap devices.
    pub(crate) swap: Vec<String>,
    /// The LUKS devices, by name, with their underlying device.
    pub(crate) luks: BTreeMap<String, String>,
    /// The microcode to carry: `intel`, `amd`, or `None`.
    pub(crate) microcode: Option<&'static str>,
    /// The modules the boot devices need.
    pub(crate) modules: BTreeSet<String>,
}

/// The UUID tag of every device that has one, by canonical device path.
fn uuids() -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    if let Ok(items) = fs::read_dir("/dev/disk/by-uuid") {
        for item in items.flatten() {
            if let Ok(device) = fs::canonicalize(item.path()) {
                out.insert(
                    device,
                    format!("UUID={}", item.file_name().to_string_lossy()),
                );
            }
        }
    }
    out
}

/// The canonical path of a device, following `/dev/mapper` links.
fn canonical(device: &str) -> PathBuf {
    fs::canonicalize(device).unwrap_or_else(|_| PathBuf::from(device))
}

/// The LUKS mapping behind `/dev/dm-N`, as its name and the underlying
/// device, when it is one.
fn luks_of(device: &Path) -> Option<(String, PathBuf)> {
    let dm = device.file_name()?.to_str()?;
    let sys = Path::new("/sys/block").join(dm);
    let uuid = fs::read_to_string(sys.join("dm/uuid")).ok()?;
    if !uuid.starts_with("CRYPT-LUKS") {
        return None;
    }
    let name = fs::read_to_string(sys.join("dm/name"))
        .ok()?
        .trim()
        .to_owned();
    // A LUKS mapping has one device below it. Sorting keeps the result
    // independent of the directory order should there be more.
    let mut slaves: Vec<_> = fs::read_dir(sys.join("slaves"))
        .ok()?
        .flatten()
        .map(|s| s.file_name())
        .collect();
    slaves.sort();
    Some((name, Path::new("/dev").join(slaves.first()?)))
}

/// Probes the filesystems mounted below the root, the swap devices and
/// the processor.
pub(crate) fn probe(env: &Env) -> Probe {
    let root = env.root.path();
    let uuids = uuids();
    let tag = |device: &Path| {
        uuids
            .get(device)
            .cloned()
            .unwrap_or_else(|| device.display().to_string())
    };
    let mut probe = Probe::default();
    let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut seen = BTreeSet::new();
    for line in mountinfo.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let point = left
            .split(' ')
            .nth(4)
            .unwrap_or_default()
            .replace("\\040", " ");
        let mut right = right.split(' ');
        let (Some(fstype), Some(source)) = (right.next(), right.next()) else {
            continue;
        };
        if !source.starts_with("/dev/") {
            continue;
        }
        let inside = match Path::new(&point).strip_prefix(root) {
            Ok(rel) if rel.as_os_str().is_empty() => "/".to_owned(),
            Ok(rel) => format!("/{}", rel.display()),
            Err(_) => continue,
        };
        let below = |dir: &str| inside == dir || inside.starts_with(&format!("{dir}/"));
        if below("/run") || below("/proc") || below("/sys") {
            continue;
        }
        let device = canonical(source);
        if let Some((name, under)) = luks_of(&device) {
            probe.luks.insert(name.clone(), tag(&under));
        }
        let device = if device.starts_with("/dev/dm-") {
            luks_of(&device).map_or_else(|| tag(&device), |(name, _)| format!("/dev/mapper/{name}"))
        } else {
            tag(&device)
        };
        if seen.insert(inside.clone()) {
            probe.filesystems.push(ProbedFs {
                mountpoint: inside,
                device,
                fstype: fstype.to_owned(),
            });
        }
    }
    probe
        .filesystems
        .sort_by(|a, b| a.mountpoint.cmp(&b.mountpoint));
    if root == Path::new("/") {
        let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
        for line in swaps.lines().skip(1) {
            if let Some(device) = line.split_whitespace().next()
                && device.starts_with("/dev/")
            {
                probe.swap.push(tag(&canonical(device)));
            }
        }
        probe.modules = laft_activate::Host::detect(&laft_activate::BootConfig::default()).modules;
    }
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    probe.microcode = if cpuinfo.contains("GenuineIntel") {
        Some("intel")
    } else if cpuinfo.contains("AuthenticAMD") {
        Some("amd")
    } else {
        None
    };
    probe
}

/// The filesystem type as the configuration writes it.
fn fstype(name: &str) -> String {
    match name {
        "ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "f2fs" | "vfat" | "tmpfs" | "nfs" | "nfs4" => {
            name.to_owned()
        }
        other => format!("other(name = {})", quoted(other)),
    }
}

/// `text` as a Stav string literal, with the characters that would end it
/// or start an interpolation escaped.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '#' => out.push_str("\\#"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The text of `hardware.stav` for `probe`.
pub(crate) fn hardware_stav(probe: &Probe) -> String {
    let mut out = String::from(
        "# Generated by `laft hardware`. It belongs to you now: laft never\n# overwrites it.\n",
    );
    for fs in &probe.filesystems {
        let _ = write!(
            out,
            "\nfs {} {{\n  device = {}\n  fstype = {}\n}}\n",
            quoted(&fs.mountpoint),
            quoted(&fs.device),
            fstype(&fs.fstype)
        );
    }
    for device in &probe.swap {
        let _ = write!(out, "\nswap {} {{}}\n", quoted(device));
    }
    for (name, device) in &probe.luks {
        let _ = write!(
            out,
            "\nluks {} {{ device = {} }}\n",
            quoted(name),
            quoted(device)
        );
    }
    let _ = write!(out, "\nboot {{\n");
    if let Some(microcode) = probe.microcode {
        let _ = writeln!(out, "  microcode = {microcode}");
    }
    let modules: Vec<String> = probe.modules.iter().map(|m| quoted(m)).collect();
    let _ = writeln!(out, "  initrd {{ modules = [{}] }}", modules.join(", "));
    out.push_str("}\n");
    out
}

/// `laft hardware`.
pub(crate) fn hardware(env: &Env, write: bool) -> Result<u8, Failure> {
    let probe = probe(env);
    let text = hardware_stav(&probe);
    let written = if write {
        env.require_write("hardware --write")?;
        let path = env.path("/etc/laft/hardware.stav");
        write_new(&path, &text)?;
        Some(path)
    } else {
        None
    };
    if env.ui.json {
        let filesystems: Vec<_> = probe
            .filesystems
            .iter()
            .map(|f| json!({"mountpoint": f.mountpoint, "device": f.device, "fstype": f.fstype}))
            .collect();
        env.ui.doc(
            json!({"filesystems": filesystems, "swap": probe.swap, "luks": probe.luks,
            "microcode": probe.microcode, "modules": probe.modules,
            "written": written.as_ref().map(|p| p.display().to_string())}),
        );
    } else if let Some(path) = written {
        env.ui.note(format!("wrote {}", path.display()));
    } else {
        env.ui.out(text.trim_end());
    }
    Ok(0)
}

fn mount_error(what: &str) -> impl Fn(rustix::io::Errno) -> Failure + '_ {
    move |e| Failure::general(format!("{what}: {e}"))
}

/// The login shell of root in the `passwd` file of `root`.
fn root_shell(root: &Path) -> String {
    fs::read_to_string(root.join("etc/passwd"))
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|l| l.starts_with("root:"))
                .and_then(|l| l.rsplit(':').next().map(str::to_owned))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into())
}

/// `laft rescue` and `laft chroot`: enters the Laft system mounted at the
/// root in a private mount namespace.
pub(crate) fn rescue(
    env: &Env,
    generation: Option<u64>,
    command: &[String],
) -> Result<u8, Failure> {
    require_root_dir(env, "rescue")?;
    if !rustix::process::geteuid().is_root() {
        return Err(Failure::needs_root("rescue"));
    }
    let root = env.root.path().to_path_buf();
    let number = match generation {
        Some(n) => n,
        None => env
            .root
            .default_generation()?
            .ok_or_else(|| Failure::general("the target has no default generation"))?,
    };
    let record = env.generation(number)?;
    laft_sandbox::private_mount_namespace()
        .map_err(|e| Failure::general(format!("entering a private mount namespace: {e}")))?;
    let run = root.join("run");
    rustix::mount::mount(
        "tmpfs",
        &run,
        "tmpfs",
        MountFlags::NOSUID | MountFlags::NODEV,
        Some(c"mode=755"),
    )
    .map_err(mount_error("mounting a tmpfs on /run"))?;
    let mounted = run.join(format!("laft/mounts/{number}"));
    fs::create_dir_all(&mounted)?;
    rustix::mount::mount_bind(root.join(format!("laft/gens/{number}")), &mounted)
        .map_err(mount_error("binding the generation"))?;
    rustix::mount::mount_remount(
        &mounted,
        MountFlags::BIND | MountFlags::RDONLY | MountFlags::NODEV,
        "",
    )
    .map_err(mount_error("making the generation read-only"))?;
    for link in ["system", "booted"] {
        symlink(format!("mounts/{number}"), run.join("laft").join(link))?;
    }
    fs::write(
        run.join("laft/init-backend"),
        format!("{}\n", record.init.backend),
    )?;
    for dir in ["dev", "proc", "sys"] {
        let target = root.join(dir);
        fs::create_dir_all(&target)?;
        rustix::mount::mount_bind_recursive(Path::new("/").join(dir), &target)
            .map_err(mount_error("binding the kernel filesystems"))?;
        rustix::mount::mount_change(
            &target,
            MountPropagationFlags::DOWNSTREAM | MountPropagationFlags::REC,
        )
        .map_err(mount_error("binding the kernel filesystems"))?;
    }
    let shell = root_shell(&root);
    rustix::process::chroot(&root).map_err(mount_error("changing the root"))?;
    std::env::set_current_dir("/")?;
    let (program, args) = match command.split_first() {
        Some((program, args)) => (program.clone(), args.to_vec()),
        None => (shell, Vec::new()),
    };
    let error = Command::new(&program)
        .args(&args)
        .env("PATH", "/usr/bin")
        .env("HOME", "/root")
        .exec();
    let code = if error.kind() == io::ErrorKind::NotFound {
        127
    } else {
        126
    };
    Err(Failure::new(code, format!("{program}: {error}")))
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
