//! The boot sequence of PID 1 in the initramfs.
//!
//! Each step that can fail drops to the emergency shell and is retried once
//! the user leaves it. The decisions the steps take are made by the pure
//! functions of the other modules. This module only carries them out.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use laft_core::{BootState, Timestamp};
use rustix::io::Errno;
use rustix::mount::{MountFlags, UnmountFlags};

use crate::cmdline::Cmdline;
use crate::config::{Config, Filesystem};
use crate::counting::{self, Decision};
use crate::device::{DeviceSpec, Devices};
use crate::generation::{self, Generation};
use crate::log::{Log, console};
use crate::plan::{self, Op};
use crate::programs::{self, FsckOutcome};
use crate::{files, options, shell};

const ROOT: &str = "/";
const NEWROOT: &str = "/newroot";
const CONFIG: &str = "/laft-boot.json";
const ESP: &str = "/esp";
const POLL: Duration = Duration::from_millis(100);
const RAMFS_MAGIC: u64 = 0x8584_58f6;
const TMPFS_MAGIC: u64 = 0x0102_1994;

/// The state the boot carries from step to step.
struct Boot {
    log: Log,
    cmdline: Cmdline,
    release: String,
    shell: bool,
    devices: Devices,
}

/// Runs the whole boot and executes the generation's init. It never
/// returns, because PID 1 must not exit.
pub(crate) fn run() -> ! {
    let mut boot = Boot {
        log: Log::default(),
        cmdline: Cmdline::default(),
        release: rustix::system::uname()
            .release()
            .to_string_lossy()
            .into_owned(),
        shell: true,
        devices: Devices::new("/sys/class/block", "/dev"),
    };
    boot.mount_kernel_filesystems();
    boot.read_cmdline();
    let config = boot.retry(Boot::read_config);
    boot.shell = config.shell;
    if boot.cmdline.break_premount {
        boot.emergency("break point premount reached (laft.break=premount)");
    }
    boot.load_modules(&config);
    boot.retry(|b| b.unlock(&config));
    let root = boot.retry(|b| b.mount_root(&config));
    let generation = boot.retry(Boot::select);
    let generation = boot.count(generation, &config);
    boot.retry(|_| {
        plan::execute(&plan::activate(
            Path::new(ROOT),
            Path::new(NEWROOT),
            generation.number,
            &generation.backend,
        ))
    });
    boot.retry(|_| files::check_layout(Path::new(NEWROOT)));
    if boot.cmdline.break_preswitch {
        boot.emergency("break point preswitch reached (laft.break=preswitch)");
    }
    boot.log.info(&format!(
        "switching to generation {} on {}",
        generation.number,
        root.display()
    ));
    boot.retry(|_| {
        plan::execute(&plan::move_kernel_filesystems(
            Path::new(ROOT),
            Path::new(NEWROOT),
        ))
    });
    boot.retry(|_| switch_root());
    boot.reopen_console();
    boot.exec_init(&generation)
}

impl Boot {
    /// Runs `step` until it succeeds, with the emergency shell after every
    /// failure.
    fn retry<T>(&mut self, mut step: impl FnMut(&mut Self) -> Result<T, String>) -> T {
        loop {
            match step(self) {
                Ok(value) => return value,
                Err(message) => self.emergency(&message),
            }
        }
    }

    fn emergency(&mut self, message: &str) {
        shell::emergency(&mut self.log, message, self.shell);
    }

    /// Without the kernel filesystems there is no shell and no log, so this
    /// step retries on its own and reports on the console.
    fn mount_kernel_filesystems(&mut self) {
        while let Err(e) = plan::execute(&plan::kernel_filesystems(Path::new(ROOT))) {
            console(&format!("{e}; retrying in 5 seconds"));
            thread::sleep(Duration::from_secs(5));
        }
        self.log
            .open(Path::new("/dev/kmsg"), Path::new("/run/laft/boot.log"));
    }

    fn read_cmdline(&mut self) {
        match fs::read_to_string("/proc/cmdline") {
            Ok(text) => self.cmdline = Cmdline::parse(&text),
            Err(e) => self
                .log
                .warn(&format!("cannot read the kernel command line: {e}")),
        }
        self.log.set_debug(self.cmdline.debug);
        for warning in std::mem::take(&mut self.cmdline.warnings) {
            self.log.warn(&warning);
        }
    }

    fn read_config(&mut self) -> Result<Config, String> {
        fs::read(CONFIG)
            .map_err(|e| e.to_string())
            .and_then(|bytes| Config::parse(&bytes, &self.release))
            .map_err(|e| format!("laft-boot.json is missing or invalid: {e}"))
    }

    /// Loads every module in order. A module that fails to load is only a
    /// warning, because it may be optional for this machine.
    fn load_modules(&mut self, config: &Config) {
        for path in &config.modules {
            let loaded = File::open(path)
                .map_err(|e| e.to_string())
                .and_then(|file| match rustix::system::finit_module(&file, c"", 0) {
                    Ok(()) | Err(Errno::EXIST) => Ok(()),
                    Err(e) => Err(e.to_string()),
                });
            match loaded {
                Ok(()) => self.log.info(&format!("loaded {path}")),
                Err(e) => self.log.warn(&format!("cannot load module {path}: {e}")),
            }
        }
    }

    fn root_wait(&self, config: &Config) -> Duration {
        Duration::from_secs(self.cmdline.rootwait.unwrap_or(config.root_wait_secs))
    }

    /// Waits for the boot devices and unlocks every LUKS device that is not
    /// unlocked yet.
    fn unlock(&mut self, config: &Config) -> Result<(), String> {
        let mut specs = Vec::new();
        for luks in &config.luks {
            specs.push(DeviceSpec::parse(&luks.device)?);
        }
        for fs in [Some(&config.root), config.laft.as_ref()]
            .into_iter()
            .flatten()
        {
            match DeviceSpec::parse(&fs.device)? {
                DeviceSpec::Mapper(_) => {}
                spec => specs.push(spec),
            }
        }
        let timeout = self.root_wait(config);
        let found = self.devices.wait(&specs, timeout, POLL)?;
        for (luks, device) in config.luks.iter().zip(found) {
            let mapper = DeviceSpec::Mapper(luks.name.clone());
            if self.devices.find(&mapper).is_some() {
                continue;
            }
            self.log
                .info(&format!("unlocking {} as {mapper}", device.display()));
            let mut tries = 0;
            loop {
                tries += 1;
                let status = Command::new(programs::CRYPTSETUP)
                    .args(programs::cryptsetup_args(&device, luks))
                    .env("DM_DISABLE_UDEV", "1")
                    .status()
                    .map_err(|e| format!("cannot run {}: {e}", programs::CRYPTSETUP))?;
                match status.code() {
                    Some(0) => break,
                    Some(programs::WRONG_PASSPHRASE) if tries < programs::PASSPHRASE_TRIES => {
                        console("wrong passphrase, try again");
                    }
                    _ => {
                        return Err(format!(
                            "cannot unlock {} as {mapper}: cryptsetup failed ({status})",
                            device.display()
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks and mounts the root filesystem at the new root, and the
    /// separate `/laft` filesystem if there is one. Mounts that an earlier
    /// attempt made are kept.
    fn mount_root(&mut self, config: &Config) -> Result<PathBuf, String> {
        let newroot = Path::new(NEWROOT);
        let timeout = self.root_wait(config);
        let root = self.resolve(&config.root, timeout)?;
        if !is_mount_point(newroot) {
            self.fsck(&config.root, &root)?;
            plan::execute(&[mount_op(&root, newroot, &config.root)])?;
        }
        if let Some(laft) = &config.laft {
            let target = newroot.join("laft");
            if !is_mount_point(&target) {
                let device = self.resolve(laft, timeout)?;
                self.fsck(laft, &device)?;
                plan::execute(&[mount_op(&device, &target, laft)])?;
            }
        }
        Ok(root)
    }

    fn resolve(&mut self, fs: &Filesystem, timeout: Duration) -> Result<PathBuf, String> {
        let spec = DeviceSpec::parse(&fs.device)?;
        let found = self.devices.wait(&[spec], timeout, POLL)?;
        found
            .into_iter()
            .next()
            .ok_or_else(|| format!("{} was not found", fs.device))
    }

    fn fsck(&mut self, fs: &Filesystem, device: &Path) -> Result<(), String> {
        let Some(fsck) = &fs.fsck else {
            return Ok(());
        };
        self.log.info(&format!("checking {}", device.display()));
        let status = Command::new(fsck)
            .arg("-p")
            .arg(device)
            .status()
            .map_err(|e| format!("cannot run {fsck}: {e}"))?;
        match programs::fsck_outcome(status.code()) {
            FsckOutcome::Continue => Ok(()),
            FsckOutcome::Reboot => {
                self.log.warn(&format!(
                    "{fsck} repaired {} and requires a reboot",
                    device.display()
                ));
                shell::reboot()
            }
            FsckOutcome::Repair => Err(format!(
                "{fsck} found errors on {device} ({status})\n\
                 run \"{fsck} {device}\" to repair the filesystem manually, then exit",
                device = device.display()
            )),
        }
    }

    fn select(&mut self) -> Result<Generation, String> {
        let gens = gens_dir();
        let default = generation::read_link(&gens.join("default")).unwrap_or_else(|e| {
            self.log.warn(&e);
            None
        });
        let selection = generation::select(self.cmdline.generation, default, &self.release, |n| {
            generation::read(&gens, n)
        })?;
        for warning in &selection.warnings {
            self.log.warn(warning);
        }
        Ok(selection.generation)
    }

    /// Counts this boot against the trial of `generation` and falls back to
    /// the last good generation when the trial is used up. Falling back to
    /// another kernel reboots through the boot loader.
    fn count(&mut self, generation: Generation, config: &Config) -> Generation {
        let path = state_path();
        let state = match fs::read(&path).map(|bytes| BootState::parse(&bytes)) {
            Ok(Ok(state)) => state,
            Ok(Err(e)) => {
                self.log.warn(&format!("ignoring {}: {e}", path.display()));
                return generation;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return generation,
            Err(e) => {
                self.log
                    .warn(&format!("cannot read {}: {e}", path.display()));
                return generation;
            }
        };
        let gens = gens_dir();
        let number = generation.number;
        let tries = config.tries;
        let decision = counting::decide(
            state,
            number,
            tries,
            self.cmdline.notries,
            |n| generation::read(&gens, n).is_ok(),
            Timestamp::now(),
        );
        match decision {
            Decision::Unchanged => generation,
            Decision::Counted(state) => {
                self.save(state);
                generation
            }
            Decision::Exhausted(state) => {
                self.log.error(&format!(
                    "generation {number} failed {tries} boots and there is no known good \
                     generation; booting it anyway"
                ));
                self.save(state);
                generation
            }
            Decision::Fallback { state, to } => {
                let Ok(target) = generation::read(&gens, to) else {
                    self.save(state);
                    return generation;
                };
                let link = gens.join("default");
                let tmp = Path::new(NEWROOT).join("laft/tmp/default.laft-boot");
                if let Err(e) = files::replace_symlink(&link, &to.to_string(), &tmp) {
                    self.log
                        .error(&format!("cannot point {} at {to}: {e}", link.display()));
                }
                self.save(state);
                if target.kernel_id == generation.kernel_id
                    && target.initramfs == generation.initramfs
                {
                    self.log.warn(&format!(
                        "generation {number} failed {tries} boots; continuing with generation {to}"
                    ));
                    return target;
                }
                match self.select_boot_entry(to, config) {
                    Ok(()) => {
                        self.log.warn(&format!(
                            "generation {number} failed {tries} boots; rebooting into \
                             generation {to}"
                        ));
                        shell::reboot()
                    }
                    Err(e) => {
                        self.log.error(&format!(
                            "cannot make the boot loader start generation {to}: {e}; \
                             continuing with generation {number}"
                        ));
                        generation
                    }
                }
            }
        }
    }

    fn save(&mut self, mut state: BootState) {
        state.prune(&existing_generations(&gens_dir()));
        let path = state_path();
        let written = state
            .to_json()
            .map_err(|e| e.to_string())
            .and_then(|bytes| files::write_atomic(&path, &bytes).map_err(|e| e.to_string()));
        if let Err(e) = written {
            self.log
                .error(&format!("cannot write {}: {e}", path.display()));
        }
    }

    /// Makes the boot menu entry of generation `to` the default one on the
    /// EFI system partition.
    fn select_boot_entry(&mut self, to: u64, config: &Config) -> Result<(), String> {
        let esp = Path::new(ESP);
        let device = self.resolve(&config.esp, self.root_wait(config))?;
        fs::create_dir_all(esp).map_err(|e| format!("cannot create {ESP}: {e}"))?;
        let mut mount = mount_op(&device, esp, &config.esp);
        if let Op::Mount { flags, .. } = &mut mount {
            flags.remove(MountFlags::RDONLY);
        }
        plan::execute(&[mount])?;
        let result = (|| {
            let entries = fs::read(esp.join("laft/entries.json"))
                .map_err(|e| format!("cannot read the boot menu index: {e}"))?;
            let index = counting::menu_index(&entries, to)?
                .ok_or_else(|| format!("the boot menu has no entry for generation {to}"))?;
            let conf_path = esp.join("limine.conf");
            let conf = fs::read_to_string(&conf_path)
                .map_err(|e| format!("cannot read {}: {e}", conf_path.display()))?;
            let conf = counting::set_default_entry(&conf, index)
                .ok_or_else(|| format!("{} has no default_entry line", conf_path.display()))?;
            files::write_atomic(&conf_path, conf.as_bytes())
                .map_err(|e| format!("cannot write {}: {e}", conf_path.display()))
        })();
        if let Err(e) = rustix::mount::unmount(esp, UnmountFlags::empty()) {
            self.log.warn(&format!("cannot unmount {ESP}: {e}"));
        }
        rustix::fs::sync();
        result
    }

    /// Makes the console the standard input, output and error of the init.
    fn reopen_console(&mut self) {
        let opened = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/console")
            .map_err(|e| e.to_string());
        let reopened = opened.and_then(|console| {
            rustix::stdio::dup2_stdin(&console)
                .and_then(|()| rustix::stdio::dup2_stdout(&console))
                .and_then(|()| rustix::stdio::dup2_stderr(&console))
                .map_err(|e| e.to_string())
        });
        if let Err(e) = reopened {
            self.log
                .warn(&format!("cannot reopen /dev/console for the init: {e}"));
        }
    }

    /// Executes the init of `generation`, or the program given with
    /// `laft.init`, and falls back to a shell when that fails. The kernel
    /// log and the boot log are closed on exec.
    fn exec_init(&mut self, generation: &Generation) -> ! {
        let command = match &self.cmdline.init {
            Some(init) => vec![init.clone()],
            None => generation.exec.clone(),
        };
        for argv in [command, vec!["/usr/bin/sh".to_owned()]] {
            let Some((program, args)) = argv.split_first() else {
                continue;
            };
            let e = Command::new(program)
                .args(args)
                .env_clear()
                .env("HOME", "/")
                .env("TERM", "linux")
                .env("PATH", "/usr/bin")
                .exec();
            self.log.error(&format!("cannot execute {program}: {e}"));
        }
        self.log.error("no init could be executed");
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }
}

fn mount_op(device: &Path, target: &Path, fs: &Filesystem) -> Op {
    let (flags, data) = options::parse(&fs.options);
    Op::Mount {
        source: device.to_string_lossy().into_owned(),
        target: target.to_owned(),
        fstype: fs.fstype.clone(),
        flags,
        data,
    }
}

fn gens_dir() -> PathBuf {
    Path::new(NEWROOT).join("laft/gens")
}

fn state_path() -> PathBuf {
    Path::new(NEWROOT).join("laft/state/boot.json")
}

/// The numbers of the generation directories in `gens`.
fn existing_generations(gens: &Path) -> BTreeSet<u64> {
    fs::read_dir(gens)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .collect()
}

/// Whether `path` is the root of a mount, because it lies on another
/// filesystem than its parent.
fn is_mount_point(path: &Path) -> bool {
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).ok();
    match (dev(path), path.parent().and_then(dev)) {
        (Some(own), Some(parent)) => own != parent,
        _ => false,
    }
}

/// Empties the initramfs to free its memory and makes the new root the
/// root of the mount namespace.
fn switch_root() -> Result<(), String> {
    let fail = |e: io::Error| format!("cannot enter the new root: {e}");
    std::env::set_current_dir(NEWROOT).map_err(fail)?;
    // Only an initramfs may be emptied. Anything else means the program does
    // not run where it should.
    let root_type = rustix::fs::statfs(ROOT)
        .map_err(|e| format!("cannot inspect the initramfs: {e}"))?
        .f_type as u64;
    if root_type == RAMFS_MAGIC || root_type == TMPFS_MAGIC {
        files::clear(Path::new(ROOT), Path::new(NEWROOT));
    }
    rustix::mount::mount_move(".", ROOT)
        .map_err(|e| format!("cannot move the new root to /: {e}"))?;
    rustix::process::chroot(".").map_err(|e| format!("cannot enter the new root: {e}"))?;
    std::env::set_current_dir(ROOT).map_err(fail)
}
