//! `laft-vmtest`: installs a minimal Laft system onto a disk image and
//! boots it under UEFI in QEMU, checking every stage on the serial
//! console.
//!
//! Run it with `tests/vm/run`, which builds the static Laft binaries
//! first. Everything it downloads, builds and boots lives in
//! `~/.cache/laft-vm`, and nothing needs root:
//!
//! 1. `laft-build` builds busybox, runit, the kernel and Limine from the
//!    recipes in `tests/vm/recipes` in bootstrap mode, and the Laft
//!    binaries are packaged as `laft`. `laft-repo` publishes them all as a
//!    signed `file://` repository.
//! 2. A GPT disk image gets an empty FAT32 EFI system partition and an
//!    empty ext4 root, made with `sfdisk`, `mkfs.vfat`, `mkfs.ext4` and
//!    `dd`.
//! 3. An installer machine boots the kernel directly with an initramfs
//!    that mounts both filesystems and runs `laft hardware --write` and
//!    `laft install` against them, with the repository in the initramfs.
//! 4. The disk boots through the UEFI firmware and Limine into
//!    `laft-boot`, runit, the services and a getty on the serial console.
//!    The test logs in, checks the booted system and powers it off, then
//!    boots it a second time to check that the boot was recorded as good.
//!
//! The console of each machine is saved next to the disk image.

#![forbid(unsafe_code)]

mod cpio;
mod disk;
mod installer;
mod packages;
mod qemu;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use crate::qemu::{Accel, Boot, Machine};

/// The result of a step: an error message for the report.
type Result<T> = std::result::Result<T, String>;

/// An error with `message`.
fn fail(message: impl Into<String>) -> String {
    message.into()
}

/// The directories the test works with.
pub(crate) struct Paths {
    /// `tests/vm` in the workspace.
    source: PathBuf,
    /// The static release binaries of the workspace.
    bin: PathBuf,
    /// `~/.cache/laft-vm`, which keeps the build root between runs.
    cache: PathBuf,
    /// The files of this run.
    work: PathBuf,
}

impl Paths {
    fn new() -> Result<Self> {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = source
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| fail("tests/vm is not inside a workspace"))?;
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map_or_else(|| workspace.join("target"), PathBuf::from);
        let home = std::env::var_os("HOME").ok_or_else(|| fail("HOME is not set"))?;
        let cache = PathBuf::from(home).join(".cache/laft-vm");
        let work = cache.join("work");
        fs::create_dir_all(&work).map_err(|e| fail(format!("{}: {e}", work.display())))?;
        Ok(Self {
            bin: target.join("x86_64-unknown-linux-musl/release"),
            source,
            cache,
            work,
        })
    }

    /// The published repository.
    fn www(&self) -> PathBuf {
        self.work.join("www")
    }
}

/// Runs `command` and fails unless it exits successfully.
fn run(command: &mut Command) -> Result<()> {
    eprintln!("laft-vmtest: running {command:?}");
    let status = command
        .status()
        .map_err(|e| fail(format!("{:?}: {e}", command.get_program())))?;
    if status.success() {
        Ok(())
    } else {
        Err(fail(format!("{command:?} failed with {status}")))
    }
}

/// Runs `command` with `input` on its standard input and fails unless it
/// exits successfully.
fn run_with_input(command: &mut Command, input: &[u8]) -> Result<()> {
    eprintln!("laft-vmtest: running {command:?}");
    let mut child = command
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| fail(format!("{:?}: {e}", command.get_program())))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .map_err(|e| fail(format!("{:?}: {e}", command.get_program())))?;
    }
    let status = child
        .wait()
        .map_err(|e| fail(format!("{:?}: {e}", command.get_program())))?;
    if status.success() {
        Ok(())
    } else {
        Err(fail(format!("{command:?} failed with {status}")))
    }
}

fn main() -> ExitCode {
    let keep = std::env::args().any(|a| a == "--keep");
    match test(keep) {
        Ok(()) => {
            eprintln!("laft-vmtest: passed");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("laft-vmtest: FAILED: {message}");
            ExitCode::FAILURE
        }
    }
}

fn test(keep: bool) -> Result<()> {
    let paths = Paths::new()?;
    let accel = Accel::detect();
    if accel == Accel::Tcg {
        eprintln!("laft-vmtest: /dev/kvm is not usable, so the machines are emulated");
    }
    let key = packages::build(&paths)?;
    let disk = paths.work.join("disk.img");
    disk::create(&disk)?;
    install(&paths, accel, &key, &disk)?;
    let vars = paths.work.join("vars.fd");
    let _ = fs::remove_file(&vars);
    qemu::prepare_vars(&vars)?;
    first_boot(&paths, accel, &disk, &vars)?;
    second_boot(&paths, accel, &disk, &vars)?;
    if !keep {
        for file in [disk, vars] {
            let _ = fs::remove_file(file);
        }
    }
    Ok(())
}

/// Installs the system onto `disk` from the installer machine.
fn install(paths: &Paths, accel: Accel, key: &str, disk: &Path) -> Result<()> {
    let (kernel, initrd) = installer::prepare(paths, key)?;
    let boot = Boot::Kernel {
        kernel,
        initrd: initrd.clone(),
        cmdline: "console=ttyS0 panic=-1 quiet".into(),
    };
    let timeout = Duration::from_secs(300 * u64::from(accel.slowdown()));
    let mut machine = Machine::start(accel, &boot, disk, &paths.work.join("install.log"), timeout)?;
    machine.expect("the installer finishes", installer::DONE)?;
    machine.wait()?;
    let _ = fs::remove_file(initrd);
    let transcript = machine.transcript();
    let status = transcript
        .split(installer::DONE)
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_default();
    if status != "0" {
        return Err(fail(format!(
            "laft install exited with status {status}, see {}",
            paths.work.join("install.log").display()
        )));
    }
    eprintln!("laft-vmtest: ok: laft install succeeded");
    Ok(())
}

/// Starts the installed system from `disk` through the firmware.
fn start(paths: &Paths, accel: Accel, disk: &Path, vars: &Path, log: &str) -> Result<Machine> {
    let timeout = Duration::from_secs(240 * u64::from(accel.slowdown()));
    Machine::start(
        accel,
        &Boot::Uefi {
            vars: vars.to_path_buf(),
        },
        disk,
        &paths.work.join(log),
        timeout,
    )
}

/// Logs in as root on the serial console.
fn login(machine: &mut Machine) -> Result<()> {
    machine.expect("the getty prompt", "laft-vm login:")?;
    machine.send("root\r")?;
    machine.expect("the password prompt", "Password:")?;
    // login discards what was typed before it turned echo off.
    std::thread::sleep(Duration::from_secs(1));
    machine.send("laft\r")?;
    shell(machine, "true").map(|_| ())
}

/// Runs `command` in the root shell and returns what it printed.
fn shell(machine: &mut Machine, command: &str) -> Result<String> {
    // The quotes keep the echo of the typed line from matching the marker.
    machine.send(&format!("echo @@be''gin; {command}; echo @@e''nd\r"))?;
    machine.expect(&format!("the output of `{command}`"), "@@begin")?;
    let before = machine.transcript().len();
    machine.expect(&format!("the end of `{command}`"), "@@end")?;
    let text = machine.transcript();
    let end = text[before..]
        .find("@@end")
        .map_or(text.len(), |i| before + i);
    Ok(text[before..end].to_owned())
}

/// Powers the machine off from the root shell and waits until it is off.
fn power_off(machine: &mut Machine) -> Result<()> {
    machine.send("runit-init 0\r")?;
    machine.expect("runit stage 3", "- runit: leave stage: /etc/runit/2")?;
    machine.expect("the power off", "- runit: power off...")?;
    let status = machine.wait()?;
    if !status.success() {
        return Err(fail(format!("QEMU exited with {status}")));
    }
    eprintln!("laft-vmtest: ok: the machine powered off");
    Ok(())
}

/// Fails unless `output` contains `needle`.
fn check(what: &str, output: &str, needle: &str) -> Result<()> {
    if output.contains(needle) {
        eprintln!("laft-vmtest: ok: {what}");
        Ok(())
    } else {
        Err(fail(format!("{what}: expected {needle:?} in:\n{output}")))
    }
}

/// Boots the new system and checks every stage, then the booted system.
fn first_boot(paths: &Paths, accel: Accel, disk: &Path, vars: &Path) -> Result<()> {
    let mut machine = start(paths, accel, disk, vars, "boot-1.log")?;
    machine.expect("Limine loads", "Booting automatically")?;
    machine.expect("Limine loads the kernel", "linux: Loading kernel")?;
    machine.expect(
        "laft-boot selects the generation",
        "laft-boot: switching to generation 1",
    )?;
    machine.expect("the switch to the new root", "- runit: $Id")?;
    machine.expect("runit stage 1 starts", "- runit: enter stage: /etc/runit/1")?;
    machine.expect(
        "the early steps activate the generation",
        "activated   generation 1 at boot",
    )?;
    machine.expect("runit stage 2 starts", "- runit: enter stage: /etc/runit/2")?;
    machine.expect(
        "the marker service runs",
        "laft-vmtest: marker service is running",
    )?;
    login(&mut machine)?;
    // boot-complete and the getty start together, so the note of
    // `laft boot mark-good` may come before or after the prompt.
    check(
        "boot mark-good ran",
        &machine.transcript(),
        "generation 1 booted completely",
    )?;
    let state = shell(&mut machine, "cat /laft/state/boot.json")?;
    check(
        "generation 1 is the last good one",
        &state,
        "\"last_good\":1",
    )?;
    check("the trial ended", &state, "\"trial\":null")?;
    let links = shell(
        &mut machine,
        "readlink /run/laft/system; readlink /run/laft/booted",
    )?;
    check(
        "/run/laft/system and booted point at generation 1",
        &links,
        "mounts/1\r\nmounts/1",
    )?;
    let mounts = shell(
        &mut machine,
        "grep -E ' /laft | /run/laft/mounts/1 ' /proc/self/mountinfo",
    )?;
    check(
        "/laft is read-only, nosuid and nodev",
        &mounts,
        " /laft ro,nosuid,nodev",
    )?;
    check(
        "the generation is mounted read-only and nodev",
        &mounts,
        " /run/laft/mounts/1 ro,nodev",
    )?;
    power_off(&mut machine)
}

/// Boots the system again and checks that the first boot was recorded as
/// good.
fn second_boot(paths: &Paths, accel: Accel, disk: &Path, vars: &Path) -> Result<()> {
    let mut machine = start(paths, accel, disk, vars, "boot-2.log")?;
    machine.expect(
        "laft-boot selects the generation again",
        "laft-boot: switching to generation 1",
    )?;
    login(&mut machine)?;
    let state = shell(&mut machine, "cat /laft/state/boot.json")?;
    check("generation 1 is still good", &state, "\"good\":[1]")?;
    check("nothing is bad", &state, "\"bad\":[]")?;
    power_off(&mut machine)
}
