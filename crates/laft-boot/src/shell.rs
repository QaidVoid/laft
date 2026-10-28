//! The emergency shell and the last resorts of PID 1.

use std::panic::PanicHookInfo;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use rustix::process::WaitOptions;
use rustix::system::RebootCommand;

use crate::log::{Log, console};

/// The static busybox of the initramfs, present when the emergency shell is
/// included.
const BUSYBOX: &str = "/bin/busybox";

/// Reports `message` and runs the emergency shell on the console until the
/// user leaves it, so that the caller can retry what failed.
///
/// Without a shell, because `allowed` is false or busybox is missing, the
/// machine reboots after 30 seconds instead.
pub(crate) fn emergency(log: &mut Log, message: &str, allowed: bool) {
    log.record_error(message);
    console(message);
    if !allowed || !Path::new(BUSYBOX).exists() {
        console("no emergency shell available; rebooting in 30 seconds");
        thread::sleep(Duration::from_secs(30));
        reboot();
    }
    console(
        "emergency shell. Boot log: /run/laft/boot.log\n\
         type \"exit\" to retry the failed step, or \"reboot -f\" to reboot.",
    );
    // The shell needs its own session for `cttyhack` to give it the console
    // as controlling terminal, so that Ctrl-C works.
    let shell = Command::new(BUSYBOX)
        .args(["setsid", BUSYBOX, "cttyhack", "/bin/sh"])
        .env("PS1", "(laft-boot) # ")
        .env("PATH", "/bin:/usr/bin")
        .status();
    if let Err(e) = shell {
        log.error(&format!("cannot start the emergency shell: {e}"));
        thread::sleep(Duration::from_secs(5));
    }
    reap();
}

/// Waits for every child that has already exited, including processes the
/// shell left behind, which PID 1 inherits.
pub(crate) fn reap() {
    while let Ok(Some(_)) = rustix::process::waitpid(None, WaitOptions::NOHANG) {}
}

/// Flushes every filesystem and restarts the machine.
pub(crate) fn reboot() -> ! {
    rustix::fs::sync();
    let _ = rustix::system::reboot(RebootCommand::Restart);
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

/// Handles a panic of PID 1, which must never exit: the panic is reported,
/// the emergency shell runs, and then the machine reboots, because the
/// failed step cannot be resumed.
pub(crate) fn on_panic(info: &PanicHookInfo<'_>) {
    let mut log = Log::default();
    log.open(Path::new("/dev/kmsg"), Path::new("/run/laft/boot.log"));
    emergency(&mut log, &format!("laft-boot {info}"), true);
    console("rebooting after the panic");
    reboot();
}
