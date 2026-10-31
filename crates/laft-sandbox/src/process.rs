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
/// executes. `SIGKILL` and `SIGSTOP` cannot be changed and are skipped.
/// Every other signal up to 64 is reset, except the few the C library
/// reserves for its own threads. It refuses to change those, and they keep
/// their disposition.
pub fn reset_signal_dispositions() {
    for signal in 1..=LAST_SIGNAL {
        if signal != libc::SIGKILL && signal != libc::SIGSTOP {
            sys::default_signal(signal);
        }
    }
}

/// Whether the process was asked to terminate since
/// [`catch_termination`].
///
/// ```
/// assert!(!laft_sandbox::terminated());
/// laft_sandbox::catch_termination();
/// assert!(!laft_sandbox::terminated());
/// ```
pub fn terminated() -> bool {
    ASKED_TO_STOP.load(std::sync::atomic::Ordering::Relaxed)
}

/// Handles SIGINT and SIGTERM by marking [`terminated`], instead of
/// ending the process. A caller checks the flag where it can stop without
/// breaking anything and kills what it runs.
///
/// The handler writes one atomic, so it is async signal safe.
pub fn catch_termination() {
    // SAFETY: installing a handler that writes one atomic touches no
    // memory but that atomic and registers no code that could unwind.
    unsafe {
        let handler = ask_to_stop as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

static ASKED_TO_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn ask_to_stop(_: libc::c_int) {
    ASKED_TO_STOP.store(true, std::sync::atomic::Ordering::Relaxed);
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
