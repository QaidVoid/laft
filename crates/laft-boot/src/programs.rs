//! The command lines of the external programs in the initramfs and the
//! meaning of their exit codes.

use std::ffi::OsString;
use std::path::Path;

use laft_core::LuksOption;

use crate::config::Luks;

/// The static `cryptsetup` in the initramfs.
pub(crate) const CRYPTSETUP: &str = "/usr/bin/cryptsetup";

/// The exit code of `cryptsetup` for a wrong passphrase.
pub(crate) const WRONG_PASSPHRASE: i32 = 2;

/// How many passphrases are tried for one LUKS device.
pub(crate) const PASSPHRASE_TRIES: u32 = 3;

/// The arguments of `cryptsetup` that unlock `luks`, found at `device`.
pub(crate) fn cryptsetup_args(device: &Path, luks: &Luks) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["open".into(), "--type".into(), "luks".into()];
    for option in &luks.options {
        args.push(
            match option {
                LuksOption::Discard => "--allow-discards",
                LuksOption::NoReadWorkqueue => "--perf-no_read_workqueue",
                LuksOption::NoWriteWorkqueue => "--perf-no_write_workqueue",
            }
            .into(),
        );
    }
    args.push(device.into());
    args.push(luks.name.clone().into());
    args
}

/// What to do after fsck finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FsckOutcome {
    /// The filesystem is clean or was repaired.
    Continue,
    /// The filesystem was repaired but the system must reboot.
    Reboot,
    /// Errors are left that need a person to repair them.
    Repair,
}

/// Interprets the exit code of fsck, a set of bit flags: 1 for corrected
/// errors, 2 when a reboot is required, 4 and above for uncorrected errors
/// and failures. `None` means fsck was killed by a signal.
pub(crate) fn fsck_outcome(code: Option<i32>) -> FsckOutcome {
    match code {
        Some(code @ 0..=3) if code & 2 != 0 => FsckOutcome::Reboot,
        Some(0 | 1) => FsckOutcome::Continue,
        _ => FsckOutcome::Repair,
    }
}

#[cfg(test)]
#[path = "programs_tests.rs"]
mod tests;
