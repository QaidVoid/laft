//! Process state that a setuid program inherits from its caller and must
//! not trust: descriptors, signal dispositions and credentials.

use crate::error::{Error, Step};
use crate::sys;

/// The highest signal number the kernel defines.
const LAST_SIGNAL: i32 = 64;

/// Marks every descriptor from 3 upward close-on-exec, so that none that
/// the caller passed in reaches a program executed later.
///
/// The descriptors stay open until that `exec`, because closing a
/// descriptor that some live handle in the process may still own would
/// invalidate it. A program that never reads a descriptor by number is not
/// influenced by the inherited ones. Descriptors 0 to 2 are left alone.
///
/// Requires Linux 5.11 or newer for `CLOSE_RANGE_CLOEXEC`.
///
/// # Errors
///
/// Fails when the kernel rejects `close_range`, for example with `ENOSYS`
/// or `EINVAL` on an older kernel.
pub fn close_inherited_on_exec() -> Result<(), Error> {
    sys::cloexec_from(3).map_err(|e| Error::new(Step::Descriptors, e))
}

/// Restores the default disposition of every signal, so that signals the
/// caller ignored behave normally in this process and in programs it
/// executes. `SIGKILL` and `SIGSTOP` cannot be changed and are skipped, and
/// so are the signals the C library reserves for itself, which it refuses
/// to change.
pub fn reset_signal_dispositions() {
    for signal in 1..=LAST_SIGNAL {
        if signal != libc::SIGKILL && signal != libc::SIGSTOP {
            sys::default_signal(signal);
        }
    }
}

/// The real, effective and saved user ids of the calling thread, in that
/// order.
///
/// # Errors
///
/// Fails only when the kernel rejects `getresuid`, which it does not do for
/// valid arguments.
pub fn user_ids() -> Result<[u32; 3], Error> {
    sys::getresuid().map_err(|e| Error::new(Step::ReadIds, e))
}

/// The real, effective and saved group ids of the calling thread, in that
/// order.
///
/// # Errors
///
/// Fails only when the kernel rejects `getresgid`, which it does not do for
/// valid arguments.
pub fn group_ids() -> Result<[u32; 3], Error> {
    sys::getresgid().map_err(|e| Error::new(Step::ReadIds, e))
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
