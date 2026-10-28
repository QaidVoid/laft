//! Installing the boot files of the generations on the EFI system
//! partition and rendering the boot menu.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use laft_core::{Blake3Hash, Timestamp};
use laft_store::Root;
use serde::{Deserialize, Serialize};

use crate::config::{Boot, BootConfig};
use crate::error::{Error, Result, corrupt, io};
use crate::{BootRecord, fault};

/// The version of `entries.json`.
const VERSION: u32 = 1;

/// The magic number of a FAT filesystem in `statfs`.
const MSDOS_SUPER_MAGIC: u32 = 0x4d44;

/// Where Limine lives in a generation, and where firmware looks for it.
const LIMINE: &str = "usr/share/limine/BOOTX64.EFI";
const LIMINE_ON_ESP: &str = "EFI/BOOT/BOOTX64.EFI";

/// What installing the boot files did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootInstall {
    /// The generations in the menu, newest first.
    pub entries: Vec<u64>,
    /// The generation the menu boots by default, if any is the default.
    pub default: Option<u64>,
    /// The files copied to the EFI system partition, relative to it.
    pub copied: Vec<String>,
    /// The files removed from it because no menu entry needs them.
    pub removed: Vec<String>,
}

/// Fails unless `esp` is a mounted FAT filesystem, as the EFI system
/// partition is.
///
/// # Errors
///
/// Fails when `esp` cannot be examined or is not FAT.
pub fn check_esp(esp: &Path) -> Result<()> {
    let dir = File::open(esp).map_err(io(esp))?;
    let stat = rustix::fs::fstatfs(&dir).map_err(|e| io(esp)(e.into()))?;
    if stat.f_type as u32 != MSDOS_SUPER_MAGIC {
        return Err(Error::Invalid(format!(
            "{} is not a mounted FAT filesystem (ESP)",
            esp.display()
        )));
    }
    Ok(())
}

/// One generation in the boot menu.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MenuEntry {
    generation: u64,
    created: Timestamp,
    kernel: String,
    boot: BootRecord,
    failed: bool,
}

impl MenuEntry {
    fn kernel_file(&self) -> String {
        format!("{}.vmlinuz", self.boot.kernel_id)
    }

    fn initramfs_file(&self) -> String {
        format!("{}.initramfs", self.boot.initramfs)
    }
}

/// Installs the kernel and initramfs of every complete generation of
/// `root` into `laft/` of the EFI system partition mounted at `esp`,
/// renders `limine.conf` and `laft/entries.json` with one entry per
/// generation, newest first, and removes boot files no entry needs.
///
/// Files are content addressed: the kernel is named by the store id of its
/// package and the initramfs by its content hash, so generations share
/// identical files. Every file is written to a temporary name, synced and
/// renamed. New boot files are in place before the menu that names them,
/// and old ones are removed only after it, so a crash at any point leaves a
/// menu whose files all exist. Limine itself is updated from the newest
/// generation before the menu, so a new menu is never read by an old
/// Limine.
///
/// The menu settings come from the configuration of the default
/// generation, or of the newest one when none is the default. Generations
/// that failed to boot are marked in the menu.
///
/// The partition is not checked here. Call [`check_esp`] first on a real
/// system.
///
/// ```no_run
/// use std::path::Path;
///
/// use laft_activate::{check_esp, install_boot};
/// use laft_store::Root;
///
/// check_esp(Path::new("/boot"))?;
/// let installed = install_boot(&Root::default(), Path::new("/boot"))?;
/// println!("boot menu: {:?}", installed.entries);
/// # Ok::<(), laft_activate::Error>(())
/// ```
///
/// # Errors
///
/// Fails when a generation's boot files cannot be read or a file on the
/// partition cannot be written.
pub fn install_boot(root: &Root, esp: &Path) -> Result<BootInstall> {
    let mut entries = Vec::new();
    let failed = failed_generations(root)?;
    for number in root.generations()?.into_iter().rev() {
        let Ok(generation) = root.generation(number) else {
            continue;
        };
        let dir = gen_dir(root, number);
        let Ok(boot) = BootRecord::read(&dir) else {
            continue;
        };
        let kernel = generation
            .packages
            .iter()
            .find(|p| p.id == boot.kernel_id)
            .map_or_else(|| "kernel".to_owned(), |p| p.name.to_string());
        entries.push(MenuEntry {
            generation: number,
            created: generation.created,
            kernel,
            boot,
            failed: failed.contains(&number),
        });
    }
    let default = root.default_generation()?;
    let settings = menu_settings(root, default.or(entries.first().map(|e| e.generation)))?;

    let laft = esp.join("laft");
    fs::create_dir_all(&laft).map_err(io(&laft))?;
    let mut install = BootInstall {
        entries: entries.iter().map(|e| e.generation).collect(),
        default: default.filter(|d| entries.iter().any(|e| e.generation == *d)),
        ..BootInstall::default()
    };
    let mut wanted = BTreeSet::new();
    for entry in &entries {
        let dir = gen_dir(root, entry.generation);
        let kernel = dir.join(format!(
            "usr/lib/modules/{}/vmlinuz",
            entry.boot.kernel_release
        ));
        for (name, source) in [
            (entry.kernel_file(), kernel),
            (entry.initramfs_file(), dir.join("boot/initramfs")),
        ] {
            if wanted.insert(name.clone()) && !laft.join(&name).exists() {
                copy_in(&source, &laft.join(&name))?;
                install.copied.push(format!("laft/{name}"));
            }
        }
    }
    if let Some(newest) = entries.first() {
        let limine = gen_dir(root, newest.generation).join(LIMINE);
        if limine.exists() && hash_of(&limine)? != hash_of(&esp.join(LIMINE_ON_ESP)).ok().flatten()
        {
            let target = esp.join(LIMINE_ON_ESP);
            let parent = target.parent().unwrap_or(esp);
            fs::create_dir_all(parent).map_err(io(parent))?;
            copy_in(&limine, &target)?;
            install.copied.push(LIMINE_ON_ESP.to_owned());
        }
    }
    fault::point("esp.files")?;

    let default_index = install
        .default
        .and_then(|d| entries.iter().position(|e| e.generation == d))
        .map_or(1, |i| i + 1);
    write_in(
        esp,
        "limine.conf",
        render_limine(&entries, default_index, &settings).as_bytes(),
    )?;
    let index = EntriesJson {
        default_gen: install.default,
        entries: entries
            .iter()
            .enumerate()
            .map(|(i, e)| IndexEntry {
                generation: e.generation,
                index: i + 1,
            })
            .collect(),
        version: VERSION,
    };
    let json =
        laft_core::to_canonical_json(&index).map_err(|e| corrupt(&laft.join("entries.json"), e))?;
    write_in(&laft, "entries.json", &json)?;
    sync_dir(&laft)?;
    sync_dir(esp)?;
    fault::point("esp.config")?;

    for item in fs::read_dir(&laft).map_err(io(&laft))? {
        let name = item
            .map_err(io(&laft))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        let boot_file = name.ends_with(".vmlinuz") || name.ends_with(".initramfs");
        if (boot_file && !wanted.contains(&name)) || name.starts_with(".tmp-") {
            let path = laft.join(&name);
            fs::remove_file(&path).map_err(io(&path))?;
            install.removed.push(format!("laft/{name}"));
        }
    }
    sync_dir(&laft)?;
    Ok(install)
}

fn gen_dir(root: &Root, number: u64) -> std::path::PathBuf {
    root.path().join("laft/gens").join(number.to_string())
}

/// The boot settings of generation `number`, from its configuration, or
/// the defaults when there is no generation.
fn menu_settings(root: &Root, number: Option<u64>) -> Result<Boot> {
    let Some(number) = number else {
        return Ok(Boot::default());
    };
    let path = gen_dir(root, number).join("config.json");
    let bytes = fs::read(&path).map_err(io(&path))?;
    let config: BootConfig = serde_json::from_slice(&bytes).map_err(|e| corrupt(&path, e))?;
    Ok(config.boot)
}

/// The generations that `laft/state/boot.json` marks as failed to boot.
fn failed_generations(root: &Root) -> Result<BTreeSet<u64>> {
    #[derive(Deserialize)]
    struct BootState {
        #[serde(default)]
        bad: BTreeSet<u64>,
    }
    let path = root.path().join("laft/state/boot.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<BootState>(&bytes)
            .map(|state| state.bad)
            .map_err(|e| corrupt(&path, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(io(&path)(e)),
    }
}

/// `limine.conf` for `entries`, newest first, booting entry
/// `default_index` (1-based) by default.
fn render_limine(entries: &[MenuEntry], default_index: usize, settings: &Boot) -> String {
    let mut out = String::from("# Rendered by laft. Changes are overwritten on the next apply.\n");
    match settings.timeout {
        Some(seconds) => {
            let _ = writeln!(out, "timeout: {seconds}");
        }
        None => out.push_str("timeout: no\n"),
    }
    let _ = writeln!(out, "default_entry: {default_index}");
    out.push_str("remember_last_entry: no\n");
    let editor = if settings.editor { "yes" } else { "no" };
    let _ = writeln!(out, "editor_enabled: {editor}");
    out.push_str("interface_branding: Laft\n");
    for entry in entries {
        let failed = if entry.failed {
            " (failed to boot)"
        } else {
            ""
        };
        let created = entry.created.to_string();
        let when = format!("{} {} UTC", &created[..10], &created[11..16]);
        let _ = write!(
            out,
            "\n/Laft {}{failed}\n    comment: {when}, {} {}\n    protocol: linux\n    path: boot():/laft/{}\n    module_path: boot():/laft/{}\n    cmdline: {}\n",
            entry.generation,
            entry.kernel,
            entry.boot.kernel_release,
            entry.kernel_file(),
            entry.initramfs_file(),
            entry.boot.cmdline.join(" "),
        );
    }
    out
}

/// `laft/entries.json`, which maps menu positions to generations.
#[derive(Serialize)]
struct EntriesJson {
    default_gen: Option<u64>,
    entries: Vec<IndexEntry>,
    version: u32,
}

#[derive(Serialize)]
struct IndexEntry {
    #[serde(rename = "gen")]
    generation: u64,
    index: usize,
}

/// A name for a temporary file that no other writer in this process uses.
fn tmp_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!(".tmp-{}-{n}", std::process::id())
}

/// Copies `source` to `target` through a synced temporary file in the
/// same directory.
fn copy_in(source: &Path, target: &Path) -> Result<()> {
    let mut from = File::open(source).map_err(io(source))?;
    let dir = target.parent().unwrap_or(target);
    let tmp = dir.join(tmp_name());
    let mut to = create(&tmp)?;
    io::copy(&mut from, &mut to).map_err(io(&tmp))?;
    to.sync_all().map_err(io(&tmp))?;
    fs::rename(&tmp, target).map_err(io(target))
}

/// Writes `bytes` to `dir/name` through a synced temporary file.
fn write_in(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let tmp = dir.join(tmp_name());
    let mut file = create(&tmp)?;
    file.write_all(bytes).map_err(io(&tmp))?;
    file.sync_all().map_err(io(&tmp))?;
    let target = dir.join(name);
    fs::rename(&tmp, &target).map_err(io(&target))
}

fn create(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .map_err(io(path))
}

fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir).and_then(|d| d.sync_all()).map_err(io(dir))
}

/// The BLAKE3 hash of the file at `path`, or `None` when it does not
/// exist.
fn hash_of(path: &Path) -> Result<Option<Blake3Hash>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(Blake3Hash::of(&bytes))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path)(e)),
    }
}

#[cfg(test)]
#[path = "esp_tests.rs"]
mod tests;
