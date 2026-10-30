//! Running QEMU with its serial console on standard input and output, and
//! waiting for text on that console.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{Result, fail};

/// The memory of every machine, in MiB.
const MEMORY_MIB: u64 = 1024;

/// The memory the host must have available before a machine starts, in
/// MiB: the machine's memory and room for QEMU itself.
const HOST_MIB: u64 = MEMORY_MIB + 512;

/// The UEFI firmware code, which QEMU ships.
const FIRMWARE_CODE: &str = "/usr/share/qemu/edk2-x86_64-code.fd";

/// The UEFI variable store template that matches [`FIRMWARE_CODE`].
const FIRMWARE_VARS: &str = "/usr/share/qemu/edk2-i386-vars.fd";

/// How a machine is accelerated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Accel {
    /// Hardware virtualization through `/dev/kvm`.
    Kvm,
    /// QEMU's own emulation, when KVM is unavailable.
    Tcg,
}

impl Accel {
    /// KVM when `/dev/kvm` can be opened for reading and writing, TCG
    /// otherwise.
    pub(crate) fn detect() -> Self {
        let kvm = OpenOptions::new().read(true).write(true).open("/dev/kvm");
        if kvm.is_ok() { Self::Kvm } else { Self::Tcg }
    }

    /// How much longer than with KVM everything takes.
    pub(crate) fn slowdown(self) -> u32 {
        match self {
            Self::Kvm => 1,
            Self::Tcg => 10,
        }
    }
}

/// How the machine starts.
pub(crate) enum Boot {
    /// The kernel and initramfs directly, with this command line.
    Kernel {
        /// The kernel image.
        kernel: PathBuf,
        /// The initramfs.
        initrd: PathBuf,
        /// The kernel command line.
        cmdline: String,
    },
    /// UEFI firmware, with this writable copy of the variable store.
    Uefi {
        /// The variable store, which keeps the firmware's boot entries
        /// between boots.
        vars: PathBuf,
    },
}

/// Copies the firmware's variable store template to `vars`, unless a copy
/// is there already.
///
/// # Errors
///
/// Fails when the firmware is not installed or the copy fails.
pub(crate) fn prepare_vars(vars: &Path) -> Result<()> {
    for file in [FIRMWARE_CODE, FIRMWARE_VARS] {
        if !Path::new(file).exists() {
            return Err(fail(format!("the UEFI firmware {file} is not installed")));
        }
    }
    if !vars.exists() {
        fs::copy(FIRMWARE_VARS, vars).map_err(|e| fail(format!("copying {FIRMWARE_VARS}: {e}")))?;
    }
    Ok(())
}

/// Fails unless the host has [`HOST_MIB`] of memory available.
fn check_memory() -> Result<()> {
    let text = fs::read_to_string("/proc/meminfo")
        .map_err(|e| fail(format!("reading /proc/meminfo: {e}")))?;
    let available = text
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .ok_or_else(|| fail("/proc/meminfo has no MemAvailable"))?;
    if available / 1024 < HOST_MIB {
        return Err(fail(format!(
            "only {} MiB of memory are available, and a machine needs {HOST_MIB} MiB",
            available / 1024
        )));
    }
    Ok(())
}

/// What the console printed so far without its escape sequences, and
/// whether it was closed.
#[derive(Default)]
struct Console {
    text: Vec<u8>,
    escape: Escape,
    closed: bool,
}

/// Where the console output is in an escape sequence.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    /// Outside one.
    #[default]
    None,
    /// After the escape character.
    Started,
    /// Inside a control sequence, which ends with a byte from `@` to `~`.
    Control,
}

impl Console {
    /// Appends `bytes` without escape sequences. The boot menu and the
    /// firmware place every character with a cursor movement, so only
    /// the text without them reads as it appears on a screen.
    fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.escape = match (self.escape, byte) {
                (Escape::None, 0x1b) => Escape::Started,
                (Escape::None, _) => {
                    self.text.push(byte);
                    Escape::None
                }
                (Escape::Started, b'[') => Escape::Control,
                (Escape::Started, _) => Escape::None,
                (Escape::Control, 0x40..=0x7e) => Escape::None,
                (Escape::Control, _) => Escape::Control,
            };
        }
    }
}

type Shared = Arc<(Mutex<Console>, Condvar)>;

/// A running machine.
pub(crate) struct Machine {
    child: Child,
    stdin: Option<ChildStdin>,
    console: Shared,
    reader: Option<JoinHandle<()>>,
    seen: usize,
    deadline: Instant,
}

impl Machine {
    /// Starts a machine with the virtio disk `disk`, booting as `boot`
    /// says, and copies its console to `log`. Every wait fails once
    /// `timeout` has passed since the start.
    ///
    /// # Errors
    ///
    /// Fails when the host lacks the memory, or QEMU or the log cannot be
    /// started.
    pub(crate) fn start(
        accel: Accel,
        boot: &Boot,
        disk: &Path,
        log: &Path,
        timeout: Duration,
    ) -> Result<Self> {
        check_memory()?;
        let mut command = Command::new("qemu-system-x86_64");
        // Under UEFI firmware dated 2015 or later, the kernel's CMOS clock
        // driver takes its alarm from the ACPI RTC event, which this
        // machine never raises, while the HPET in legacy replacement mode
        // takes over the clock's own IRQ 8. The clock would then deliver no
        // interrupt, and hwclock, which waits for one, would time out.
        command.args(["-machine", "q35,hpet=off", "-smp", "2", "-m"]);
        command.arg(MEMORY_MIB.to_string());
        match accel {
            Accel::Kvm => command.args(["-accel", "kvm", "-cpu", "host"]),
            Accel::Tcg => command.args(["-accel", "tcg", "-cpu", "max"]),
        };
        command.args(["-nographic", "-vga", "none", "-nic", "none", "-no-reboot"]);
        command.arg("-drive").arg(format!(
            "file={},format=raw,if=virtio,cache=unsafe",
            disk.display()
        ));
        match boot {
            Boot::Kernel {
                kernel,
                initrd,
                cmdline,
            } => {
                command.arg("-kernel").arg(kernel);
                command.arg("-initrd").arg(initrd);
                command.arg("-append").arg(cmdline);
            }
            Boot::Uefi { vars } => {
                command
                    .arg("-drive")
                    .arg(format!(
                        "if=pflash,format=raw,unit=0,readonly=on,file={FIRMWARE_CODE}"
                    ))
                    .arg("-drive")
                    .arg(format!(
                        "if=pflash,format=raw,unit=1,file={}",
                        vars.display()
                    ));
            }
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command
            .spawn()
            .map_err(|e| fail(format!("starting qemu-system-x86_64: {e}")))?;
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut file = File::create(log).map_err(|e| fail(format!("{}: {e}", log.display())))?;
        let console: Shared = Arc::default();
        let shared = Arc::clone(&console);
        let reader = thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                let n = stdout.read(&mut buffer).unwrap_or(0);
                let (lock, ready) = &*shared;
                let mut console = lock.lock().unwrap_or_else(|e| e.into_inner());
                if n == 0 {
                    console.closed = true;
                    ready.notify_all();
                    return;
                }
                let _ = file.write_all(&buffer[..n]);
                console.push(&buffer[..n]);
                ready.notify_all();
            }
        });
        Ok(Self {
            child,
            stdin,
            console,
            reader: Some(reader),
            seen: 0,
            deadline: Instant::now() + timeout,
        })
    }

    /// Waits until the console prints `needle` after everything an earlier
    /// wait matched, and reports `what` once it did.
    ///
    /// # Errors
    ///
    /// Fails when the machine stops or the time is up first.
    pub(crate) fn expect(&mut self, what: &str, needle: &str) -> Result<()> {
        let (lock, ready) = &*self.console;
        let mut console = lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(at) = find(&console.text[self.seen..], needle.as_bytes()) {
                self.seen += at + needle.len();
                eprintln!("laft-vmtest: ok: {what}");
                return Ok(());
            }
            if console.closed {
                return Err(fail(format!(
                    "the machine stopped before {what} (waited for {needle:?})"
                )));
            }
            let now = Instant::now();
            if now >= self.deadline {
                return Err(fail(format!(
                    "timed out waiting for {what} (waited for {needle:?})"
                )));
            }
            console = ready
                .wait_timeout(console, self.deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The position in [`Self::transcript`] just after the text the last
    /// wait matched.
    pub(crate) fn position(&self) -> usize {
        self.seen
    }

    /// Types `text` on the console.
    ///
    /// # Errors
    ///
    /// Fails when the machine no longer reads its console.
    pub(crate) fn send(&mut self, text: &str) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| fail("the console is closed"))?;
        stdin
            .write_all(text.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| fail(format!("typing on the console: {e}")))
    }

    /// Waits until QEMU exits and returns its status.
    ///
    /// # Errors
    ///
    /// Fails when the time is up first.
    pub(crate) fn wait(&mut self) -> Result<ExitStatus> {
        self.stdin = None;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|e| fail(format!("waiting for QEMU: {e}")))?
            {
                if let Some(reader) = self.reader.take() {
                    let _ = reader.join();
                }
                return Ok(status);
            }
            if Instant::now() >= self.deadline {
                return Err(fail("timed out waiting for the machine to power off"));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// What the console printed between the positions `start` and `end`.
    pub(crate) fn printed(&self, start: usize, end: usize) -> String {
        let (lock, _) = &*self.console;
        let console = lock.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&console.text[start..end]).into_owned()
    }

    /// Everything the console printed.
    pub(crate) fn transcript(&self) -> String {
        let (lock, _) = &*self.console;
        let console = lock.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&console.text).into_owned()
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
