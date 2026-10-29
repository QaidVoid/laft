//! The initramfs of a generation and its boot record.
//!
//! The initramfs holds `laft-boot` as `/init`, its configuration
//! `laft-boot.json`, the kernel modules and firmware needed to reach the
//! root filesystem, and the programs of the initrd packages. It contains
//! nothing specific to one generation, so generations with the same boot
//! configuration produce byte identical files and share one copy on the
//! EFI system partition.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use laft_core::{Blake3Hash, LuksOption, PkgRef, StoreId, Variant};
use laft_store::{Generation, Store};
use serde::{Deserialize, Serialize};

use crate::config::{BootConfig, Fs, Microcode};
use crate::error::{Error, Result, Warning, corrupt, io};
use crate::{fsx, tree};

mod cpio;
mod modules;

pub(crate) use modules::normalize;

use cpio::{Archive, Kind};
use modules::{CRYPTO_DIRS, GENERIC_DIRS, KEYBOARD, ModuleIndex, modinfo_values};

/// The version of [`BootRecord`] and of `laft-boot.json`.
const VERSION: u32 = 1;

/// The zstd level of the main archive.
const ZSTD_LEVEL: i32 = 6;

/// The largest decompressed kernel module, in bytes.
const MAX_MODULE_SIZE: u64 = 256 << 20;

/// The busybox applets that get a symlink in `/bin`.
const APPLETS: [&str; 23] = [
    "sh", "ash", "cat", "cp", "dmesg", "echo", "find", "grep", "less", "ln", "ls", "mkdir",
    "mknod", "mount", "mv", "ps", "reboot", "poweroff", "rm", "sleep", "umount", "vi", "cttyhack",
];

/// The record of a generation's kernel and initramfs, stored as
/// `boot/boot.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootRecord {
    /// The format version, 1.
    pub version: u32,
    /// The kernel command line, starting with `laft.gen=<N>`.
    pub cmdline: Vec<String>,
    /// The firmware files in the initramfs, as absolute paths in it.
    pub firmware: Vec<String>,
    /// The content hash that names the initramfs on the EFI system
    /// partition: the first 32 characters of the base32 BLAKE3 hash.
    pub initramfs: String,
    /// The store id of the kernel package.
    pub kernel_id: StoreId,
    /// The kernel release, such as `6.17.5-laft1`.
    pub kernel_release: String,
    /// The kernel modules in the initramfs, in load order.
    pub modules: Vec<String>,
}

impl BootRecord {
    /// Reads the boot record of the generation directory `generation_dir`.
    ///
    /// # Errors
    ///
    /// Fails when `boot/boot.json` cannot be read or is not a supported
    /// version.
    pub fn read(generation_dir: &Path) -> Result<Self> {
        let path = generation_dir.join("boot/boot.json");
        let bytes = fs::read(&path).map_err(io(&path))?;
        let record: Self = serde_json::from_slice(&bytes).map_err(|e| corrupt(&path, e))?;
        if record.version != VERSION {
            return Err(corrupt(&path, "unsupported format version"));
        }
        Ok(record)
    }
}

/// A package the initramfs needs from outside the system view, and the
/// setting that needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitrdPackage {
    /// The package reference, such as `busybox:static`.
    pub pkg: PkgRef,
    /// The setting that needs it, such as `boot.initrd.shell`.
    pub origin: String,
}

/// The packages the initramfs of `config` takes programs and microcode
/// from, on the machine `host`. They are resolved as a closure of the
/// generation that belongs to no view.
///
/// | Package | When |
/// |---|---|
/// | `busybox:static` | `boot.initrd.shell` is set |
/// | `cryptsetup:static` | a LUKS device is declared |
/// | `e2fsprogs:static` | `/` or `/laft` is ext2, ext3 or ext4 and is checked |
/// | `f2fs-tools:static` | `/` or `/laft` is f2fs and is checked |
/// | `intel-ucode` | Intel microcode is selected by `boot.microcode` |
/// | `linux-firmware` | AMD microcode is selected by `boot.microcode` |
///
/// A filesystem is checked when its `fsck` is set, and by default for
/// ext2, ext3, ext4, f2fs and vfat. The initramfs has no checker for vfat,
/// so a vfat filesystem adds nothing. When `/` and `/laft` need the same
/// package, its origin names both.
///
/// # Panics
///
/// Panics if a package name of the table is not a valid name, which the
/// literals in this function rule out.
///
/// ```
/// use laft_activate::{BootConfig, Host, initrd_packages};
///
/// let packages = initrd_packages(&BootConfig::default(), &Host::default());
/// assert!(packages.iter().any(|p| p.pkg.to_string() == "busybox:static"));
/// ```
pub fn initrd_packages(config: &BootConfig, host: &Host) -> Vec<InitrdPackage> {
    let mut out = Vec::new();
    let mut add = |name: &str, variant: Variant, origin: String| {
        let pkg = PkgRef::new(
            name.parse()
                .expect("the package names of the table are valid names"),
            None,
            variant,
        );
        match out.iter_mut().find(|p: &&mut InitrdPackage| p.pkg == pkg) {
            Some(known) if known.origin != origin => {
                known.origin = format!("{}, {origin}", known.origin);
            }
            Some(_) => {}
            None => out.push(InitrdPackage { pkg, origin }),
        }
    };
    if config.boot.initrd.shell {
        add("busybox", Variant::Static, "boot.initrd.shell".into());
    }
    if !config.luks.is_empty() {
        add("cryptsetup", Variant::Static, "luks".into());
    }
    for mountpoint in ["/", "/laft"] {
        let Some(fs) = config.fs.get(mountpoint) else {
            continue;
        };
        match fsck_program(fs).as_deref() {
            Some("/usr/bin/e2fsck") => {
                add("e2fsprogs", Variant::Static, format!("fs {mountpoint:?}"));
            }
            Some("/usr/bin/fsck.f2fs") => {
                add("f2fs-tools", Variant::Static, format!("fs {mountpoint:?}"));
            }
            _ => {}
        }
    }
    for vendor in microcode_vendors(config, host) {
        let name = if vendor == INTEL {
            "intel-ucode"
        } else {
            "linux-firmware"
        };
        add(name, Variant::Dynamic, "boot.microcode".into());
    }
    out
}

/// What assembly needs to know about the machine it runs on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Host {
    /// The release of the running kernel. A generation that does not ship
    /// it keeps its modules, so that modules can still be loaded until the
    /// next reboot.
    pub kernel: Option<String>,
    /// The kernel modules the boot devices need, for a host only
    /// initramfs, by name.
    pub modules: BTreeSet<String>,
    /// The CPU vendor id, such as `GenuineIntel`, for host only microcode.
    pub cpu_vendor: Option<String>,
}

impl Host {
    /// Looks at the running machine: the kernel release, the CPU vendor,
    /// and the drivers of the block devices that hold `/`, `/laft`, the
    /// EFI system partition and every LUKS device of `config`.
    ///
    /// Anything that cannot be read is left out, so the result may be
    /// empty, for example inside a container.
    pub fn detect(config: &BootConfig) -> Self {
        let kernel = rustix::system::uname()
            .release()
            .to_str()
            .ok()
            .map(str::to_owned);
        let cpu_vendor = fs::read_to_string("/proc/cpuinfo").ok().and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("vendor_id")?.split_once(':'))
                .map(|(_, vendor)| vendor.trim().to_owned())
        });
        let mut devices = Vec::new();
        let mut mountpoints = vec!["/", config.boot.esp.as_str()];
        if config.fs.contains_key("/laft") {
            mountpoints.push("/laft");
        }
        for mountpoint in mountpoints {
            if let Ok(stat) = rustix::fs::stat(mountpoint) {
                devices.push(split_dev(stat.st_dev));
            }
        }
        for luks in config.luks.values() {
            let path = match luks.device.strip_prefix("UUID=") {
                Some(uuid) => PathBuf::from("/dev/disk/by-uuid").join(uuid),
                None => PathBuf::from(&luks.device),
            };
            if let Ok(stat) = rustix::fs::stat(&path) {
                devices.push(split_dev(stat.st_rdev));
            }
        }
        Self {
            kernel,
            modules: device_modules(Path::new("/sys"), &devices),
            cpu_vendor,
        }
    }
}

fn split_dev(dev: u64) -> (u32, u32) {
    (rustix::fs::major(dev), rustix::fs::minor(dev))
}

/// The modules of the drivers of every device in `devices`, given as major
/// and minor numbers, and of the devices above them, read from the sysfs
/// tree at `sysfs`.
pub(crate) fn device_modules(sysfs: &Path, devices: &[(u32, u32)]) -> BTreeSet<String> {
    let top = sysfs.join("devices");
    let mut found = BTreeSet::new();
    for (major, minor) in devices {
        let link = sysfs.join(format!("dev/block/{major}:{minor}"));
        let Ok(mut node) = fs::canonicalize(&link) else {
            continue;
        };
        let top = fs::canonicalize(&top).unwrap_or_else(|_| top.clone());
        while node.starts_with(&top) {
            if let Ok(module) = fs::read_link(node.join("driver/module"))
                && let Some(name) = module.file_name()
            {
                found.insert(normalize(&name.to_string_lossy()));
            }
            if !node.pop() {
                break;
            }
        }
    }
    found
}

/// What the initramfs of a generation is built from.
pub(crate) struct Inputs<'a> {
    /// The generation tree under construction, after its triggers ran.
    pub(crate) tree: &'a Path,
    /// The store.
    pub(crate) store: &'a Store,
    /// The generation's record, with its packages and initrd packages.
    pub(crate) generation: &'a Generation,
    /// The boot configuration.
    pub(crate) config: &'a BootConfig,
    /// The machine running the assembly.
    pub(crate) host: &'a Host,
    /// The directory of the previous generation, whose initramfs is shared
    /// when it is identical.
    pub(crate) previous: Option<&'a Path>,
}

/// Builds the initramfs into `boot/initramfs` of the tree and writes
/// `boot/boot.json`, and returns the boot record. What the user should know
/// about the module selection goes to `warnings`.
///
/// # Panics
///
/// Panics if a BLAKE3 hash does not render as `b3:` and 64 hex digits,
/// which [`Blake3Hash`] guarantees.
pub(crate) fn build(inputs: &Inputs<'_>, warnings: &mut Vec<Warning>) -> Result<BootRecord> {
    let boot = &inputs.config.boot;
    for word in &boot.cmdline {
        if word.is_empty() || word.contains(char::is_whitespace) {
            return Err(Error::Invalid(format!(
                "boot.cmdline: {word:?} must be one word without spaces"
            )));
        }
    }
    let (kernel_id, release) = kernel(inputs)?;
    let release_dir = format!("usr/lib/modules/{release}");
    let index = read_index(inputs.tree, &release_dir)?;

    let (names, dirs) = requested(inputs.config, inputs.host, &index, &release, warnings)?;
    let order = index.select(&names, &dirs);
    let mut main = Archive::default();
    for dir in ["dev", "proc", "sys", "run", "newroot"] {
        main.add(dir, 0o755, Kind::Dir);
    }
    main.add("dev/console", 0o600, Kind::CharDevice(5, 1));
    main.add("dev/null", 0o666, Kind::CharDevice(1, 3));

    let mut module_paths = Vec::new();
    let mut firmware_wanted = BTreeSet::new();
    for name in &order {
        let Some(path) = index.path(name) else {
            continue;
        };
        let bytes = read_module(inputs.tree, &format!("{release_dir}/{path}"))?;
        let modinfo = section(&bytes, ".modinfo", path)?;
        for firmware in modinfo_values(&modinfo, "firmware") {
            firmware_wanted.insert(firmware.to_owned());
        }
        let inside = format!(
            "lib/modules/{release}/{}",
            path.strip_suffix(".zst").unwrap_or(path)
        );
        module_paths.push(format!("/{inside}"));
        main.add(&inside, 0o644, Kind::File(bytes));
    }
    let mut firmware = Vec::new();
    for name in firmware_wanted {
        if name.starts_with('/') || name.split('/').any(|part| part == ".." || part.is_empty()) {
            continue;
        }
        for suffix in ["", ".zst", ".xz"] {
            let rel = format!("usr/lib/firmware/{name}{suffix}");
            if let Some(file) = tree::resolve_file(inputs.tree, &rel)? {
                let inside = format!("lib/firmware/{name}{suffix}");
                main.add(&inside, 0o644, Kind::File(read(&file)?));
                firmware.push(format!("/{inside}"));
                break;
            }
        }
    }

    let init = tree::resolve_file(inputs.tree, "usr/lib/laft/laft-boot")?.ok_or_else(|| {
        Error::Boot(
            "the system view has no usr/lib/laft/laft-boot to run as the initramfs init".into(),
        )
    })?;
    main.add("init", 0o755, Kind::File(read(&init)?));
    let programs = Programs { inputs };
    if boot.initrd.shell {
        main.add(
            "bin/busybox",
            0o755,
            Kind::File(programs.require("usr/bin/busybox", "initrd.shell")?),
        );
        for applet in APPLETS {
            main.add(
                &format!("bin/{applet}"),
                0o777,
                Kind::Symlink("busybox".into()),
            );
        }
    }
    if !inputs.config.luks.is_empty() {
        main.add(
            "usr/bin/cryptsetup",
            0o755,
            Kind::File(programs.require("usr/bin/cryptsetup", "a luks device")?),
        );
    }
    let root_fs = filesystem(inputs.config, "/")?;
    let laft_fs = inputs
        .config
        .fs
        .get("/laft")
        .map(|_| filesystem(inputs.config, "/laft"))
        .transpose()?;
    for fs in std::iter::once(&root_fs).chain(laft_fs.as_ref()) {
        if let Some(fsck) = &fs.fsck
            && !main.contains(fsck)
        {
            let reason = format!("checking the {} filesystem {}", fs.fstype, fs.device);
            main.add(
                fsck,
                0o755,
                Kind::File(programs.require(&fsck[1..], &reason)?),
            );
        }
    }
    let esp = inputs.config.fs.get(&boot.esp).ok_or_else(|| {
        Error::Invalid(format!(
            "boot.esp is {:?} but there is no fs {:?}",
            boot.esp, boot.esp
        ))
    })?;
    let config = LaftBootJson {
        esp: FsJson {
            device: device(&boot.esp, esp)?,
            fsck: None,
            options: esp.options.join(","),
            fstype: esp.fstype.clone(),
        },
        laft: laft_fs,
        luks: inputs
            .config
            .luks
            .iter()
            .map(|(name, luks)| LuksJson {
                device: luks.device.clone(),
                name: name.clone(),
                options: luks.options.iter().copied().collect(),
            })
            .collect(),
        modules: module_paths,
        release: release.clone(),
        root: root_fs,
        root_wait_secs: boot.initrd.root_wait,
        shell: boot.initrd.shell,
        tries: boot.tries,
        version: VERSION,
    };
    let json = laft_core::to_canonical_json(&config)
        .map_err(|e| Error::Invalid(format!("laft-boot.json: {e}")))?;
    main.add("laft-boot.json", 0o644, Kind::File(json));

    let early = microcode(inputs, &programs)?;
    let mut bytes = if early.is_empty() {
        Vec::new()
    } else {
        early.to_bytes()?
    };
    bytes.extend(compress(&main.to_bytes()?)?);
    let initramfs = Blake3Hash::of(&bytes)
        .to_string()
        .get(3..35)
        .expect("a hash renders as b3: and 64 hex digits")
        .to_owned();

    let boot_dir = inputs.tree.join("boot");
    let target = boot_dir.join("initramfs");
    let shared = inputs.previous.and_then(|dir| {
        let record = BootRecord::read(dir).ok()?;
        (record.initramfs == initramfs).then(|| dir.join("boot/initramfs"))
    });
    match shared {
        Some(old) if fs::hard_link(&old, &target).is_ok() => {}
        _ => fsx::write_new(&target, &bytes, 0o444)?,
    }
    let mut cmdline = vec![format!("laft.gen={}", inputs.generation.generation)];
    cmdline.extend(boot.cmdline.iter().cloned());
    let record = BootRecord {
        version: VERSION,
        cmdline,
        firmware,
        initramfs,
        kernel_id,
        kernel_release: release,
        modules: order,
    };
    let path = boot_dir.join("boot.json");
    let json = laft_core::to_canonical_json(&record)
        .map_err(|e| corrupt(&path, format!("not canonical JSON: {e}")))?;
    fsx::write_new(&path, &json, 0o444)?;
    Ok(record)
}

/// The kernel package's store id and its release, the one directory
/// `usr/lib/modules/<release>` that holds a `vmlinuz`.
fn kernel(inputs: &Inputs<'_>) -> Result<(StoreId, String)> {
    let name = &inputs.config.boot.kernel;
    let package = inputs
        .generation
        .packages
        .iter()
        .find(|p| p.name.as_str() == name)
        .ok_or_else(|| {
            Error::Boot(format!(
                "the kernel package {name} is not in the system view"
            ))
        })?;
    let manifest = inputs.store.manifest(&package.dir)?;
    let releases: Vec<&str> = manifest
        .entries()
        .iter()
        .filter(|e| matches!(e, laft_pkg::Entry::File { .. }))
        .filter_map(|e| {
            let rest = e.path().strip_prefix("usr/lib/modules/")?;
            let (release, file) = rest.split_once('/')?;
            (file == "vmlinuz").then_some(release)
        })
        .collect();
    match releases.as_slice() {
        [release] => Ok((package.id, (*release).to_owned())),
        _ => Err(Error::Boot(format!(
            "the kernel package {name} must ship exactly one usr/lib/modules/<release>/vmlinuz, it ships {}",
            releases.len()
        ))),
    }
}

/// Reads the module index of the release directory `release_dir`.
fn read_index(tree: &Path, release_dir: &str) -> Result<ModuleIndex> {
    let text = |name: &str, required: bool| -> Result<String> {
        let path = tree.join(release_dir).join(name);
        match fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => Ok(String::new()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::Boot(format!(
                "/{release_dir} has no {name}, so the depmod trigger did not index it"
            ))),
            Err(e) => Err(io(&path)(e)),
        }
    };
    ModuleIndex::parse(
        &text("modules.dep", true)?,
        &text("modules.builtin", false)?,
        &text("modules.softdep", false)?,
        &text("modules.order", false)?,
    )
    .map_err(|e| Error::Boot(format!("/{release_dir}/{e}")))
}

/// The requested module names and module directories.
///
/// The configured modules, the drivers of the file systems the initramfs
/// mounts and the device mapper modules for LUKS are required. Without them
/// the system cannot boot, so each must be a module or built into the
/// kernel. Keyboard and host modules are wanted when the kernel has them.
/// A host only initramfs for a host whose boot device drivers are unknown
/// gets the generic drivers instead, with a warning.
fn requested(
    config: &BootConfig,
    host: &Host,
    index: &ModuleIndex,
    release: &str,
    warnings: &mut Vec<Warning>,
) -> Result<(BTreeSet<String>, Vec<&'static str>)> {
    let initrd = &config.boot.initrd;
    let mut required = Vec::new();
    let mut dirs = Vec::new();
    for module in &initrd.modules {
        required.push(("boot.initrd.modules".to_owned(), normalize(module)));
    }
    let mut mounts = vec!["/".to_owned(), config.boot.esp.clone()];
    if config.fs.contains_key("/laft") {
        mounts.push("/laft".into());
    }
    for mount in mounts {
        let Some(fs) = config.fs.get(&mount) else {
            continue;
        };
        let modules = match fs.fstype.as_str() {
            "ext2" | "ext3" | "ext4" => vec!["ext4".to_owned()],
            "vfat" => ["vfat", "nls_cp437", "nls_iso8859_1"]
                .map(String::from)
                .to_vec(),
            other => vec![normalize(other)],
        };
        let context = format!("the {} file system of {mount}", fs.fstype);
        required.extend(modules.into_iter().map(|m| (context.clone(), m)));
    }
    if !config.luks.is_empty() {
        for module in ["dm_mod", "dm_crypt"] {
            required.push(("boot.luks".to_owned(), module.to_owned()));
        }
        dirs.extend(CRYPTO_DIRS);
    }
    let mut names = BTreeSet::new();
    for (context, name) in required {
        if index.path(&name).is_none() && !index.is_builtin(&name) {
            return Err(Error::Boot(format!(
                "{context}: kernel {release} has no module {name}"
            )));
        }
        names.insert(name);
    }
    names.extend(KEYBOARD.map(String::from));
    if initrd.hostonly && host.modules.is_empty() {
        warnings.push(Warning::NoHostModules);
        dirs.extend(GENERIC_DIRS);
    } else if initrd.hostonly {
        names.extend(host.modules.iter().map(|m| normalize(m)));
    } else {
        dirs.extend(GENERIC_DIRS);
    }
    Ok((names, dirs))
}

/// The content of the module `rel` of `tree`, decompressed.
fn read_module(tree: &Path, rel: &str) -> Result<Vec<u8>> {
    let path = tree::resolve_file(tree, rel)?
        .ok_or_else(|| Error::Boot(format!("module /{rel} is not a regular file")))?;
    let bytes = read(&path)?;
    if rel.ends_with(".ko") {
        return Ok(bytes);
    }
    if !rel.ends_with(".ko.zst") {
        return Err(Error::Boot(format!(
            "module /{rel} uses a compression other than zstd"
        )));
    }
    let mut decoder = zstd::stream::read::Decoder::new(bytes.as_slice()).map_err(io(&path))?;
    decoder.window_log_max(27).map_err(io(&path))?;
    let mut out = Vec::new();
    decoder
        .take(MAX_MODULE_SIZE + 1)
        .read_to_end(&mut out)
        .map_err(io(&path))?;
    if out.len() as u64 > MAX_MODULE_SIZE {
        return Err(Error::Boot(format!("module /{rel} is too large")));
    }
    Ok(out)
}

/// The content of the ELF section `name` of `bytes`, or an empty one when
/// the object has no such section.
fn section(bytes: &[u8], name: &str, path: &str) -> Result<Vec<u8>> {
    use object::{Object, ObjectSection};

    let file = object::File::parse(bytes)
        .map_err(|e| Error::Boot(format!("module {path} is not an ELF object: {e}")))?;
    match file.section_by_name(name) {
        Some(section) => section
            .data()
            .map(<[u8]>::to_vec)
            .map_err(|e| Error::Boot(format!("module {path}: {name}: {e}"))),
        None => Ok(Vec::new()),
    }
}

/// Finds programs in the initrd packages.
struct Programs<'a, 'b> {
    inputs: &'a Inputs<'b>,
}

impl Programs<'_, '_> {
    /// The content of the file `rel` of the first initrd package that has
    /// it.
    fn find(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        for package in &self.inputs.generation.initrd_packages {
            let entry = self.inputs.store.path(&package.dir);
            if let Some(file) = tree::resolve_file(&entry, rel)? {
                return read(&file).map(Some);
            }
        }
        Ok(None)
    }

    /// Like [`Self::find`], but fails naming `reason` when no initrd
    /// package has the file.
    fn require(&self, rel: &str, reason: &str) -> Result<Vec<u8>> {
        self.find(rel)?.ok_or_else(|| {
            Error::Boot(format!(
                "no initrd package provides /{rel}, which {reason} needs in the initramfs"
            ))
        })
    }
}

/// The CPU vendor id of Intel processors.
const INTEL: &str = "GenuineIntel";
/// The CPU vendor id of AMD processors.
const AMD: &str = "AuthenticAMD";

/// The CPU vendors whose microcode the initramfs of `config` carries on
/// `host`.
fn microcode_vendors(config: &BootConfig, host: &Host) -> Vec<&'static str> {
    let boot = &config.boot;
    match boot.microcode {
        Microcode::Off => vec![],
        Microcode::Intel => vec![INTEL],
        Microcode::Amd => vec![AMD],
        Microcode::Both => vec![AMD, INTEL],
        Microcode::Auto => match host.cpu_vendor.as_deref() {
            Some(INTEL) if boot.initrd.hostonly => vec![INTEL],
            Some(AMD) if boot.initrd.hostonly => vec![AMD],
            _ => vec![AMD, INTEL],
        },
    }
}

/// The uncompressed archive of CPU microcode that the kernel reads before
/// the main archive.
fn microcode(inputs: &Inputs<'_>, programs: &Programs<'_, '_>) -> Result<Archive> {
    let mut archive = Archive::default();
    for vendor in microcode_vendors(inputs.config, inputs.host) {
        if let Some(bytes) = programs.find(&format!("usr/share/laft/microcode/{vendor}.bin"))? {
            archive.add(
                &format!("kernel/x86/microcode/{vendor}.bin"),
                0o644,
                Kind::File(bytes),
            );
        }
    }
    Ok(archive)
}

fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;

    let fail = |e: std::io::Error| Error::Boot(format!("compressing the initramfs: {e}"));
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), ZSTD_LEVEL).map_err(fail)?;
    encoder.include_checksum(true).map_err(fail)?;
    encoder
        .set_pledged_src_size(Some(bytes.len() as u64))
        .map_err(fail)?;
    encoder.write_all(bytes).map_err(fail)?;
    encoder.finish().map_err(fail)
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(io(path))
}

/// `laft-boot.json`, the configuration of `laft-boot`.
#[derive(Serialize)]
struct LaftBootJson {
    esp: FsJson,
    laft: Option<FsJson>,
    luks: Vec<LuksJson>,
    modules: Vec<String>,
    release: String,
    root: FsJson,
    root_wait_secs: u32,
    shell: bool,
    tries: u32,
    version: u32,
}

/// A filesystem in `laft-boot.json`.
#[derive(Serialize)]
struct FsJson {
    device: String,
    fsck: Option<String>,
    options: String,
    #[serde(rename = "type")]
    fstype: String,
}

/// A LUKS device in `laft-boot.json`.
#[derive(Serialize)]
struct LuksJson {
    device: String,
    name: String,
    options: Vec<LuksOption>,
}

/// The filesystem mounted at `mountpoint` as `laft-boot` mounts it.
fn filesystem(config: &BootConfig, mountpoint: &str) -> Result<FsJson> {
    let fs = config.fs.get(mountpoint).ok_or_else(|| {
        Error::Invalid(format!(
            "no filesystem for {mountpoint}: declare fs {mountpoint:?}"
        ))
    })?;
    Ok(FsJson {
        device: device(mountpoint, fs)?,
        fsck: fsck_program(fs),
        options: fs.options.join(","),
        fstype: fs.fstype.clone(),
    })
}

/// The program that checks `fs` in the initramfs, if it is checked.
fn fsck_program(fs: &Fs) -> Option<String> {
    let checked = fs.fsck.unwrap_or(matches!(
        fs.fstype.as_str(),
        "ext2" | "ext3" | "ext4" | "f2fs" | "vfat"
    ));
    match fs.fstype.as_str() {
        "ext2" | "ext3" | "ext4" if checked => Some("/usr/bin/e2fsck".to_owned()),
        "f2fs" if checked => Some("/usr/bin/fsck.f2fs".to_owned()),
        _ => None,
    }
}

fn device(mountpoint: &str, fs: &Fs) -> Result<String> {
    fs.device
        .clone()
        .ok_or_else(|| Error::Invalid(format!("fs {mountpoint:?}: device is required")))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
