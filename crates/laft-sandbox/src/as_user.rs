//! Running a program as another user.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus};

use rustix::process::{Gid, Uid, getuid};

/// Runs `command` as `uid` and `gid`, without supplementary groups, and
/// waits for it. Only root may switch to another user; a caller that is
/// not root keeps its own ids, which is what the tests run with.
///
/// The ids are switched in the child between `fork` and `exec`, so the
/// caller keeps its privileges.
///
/// # Errors
///
/// Fails when the program cannot be spawned, or, in the child, when the
/// ids cannot be switched.
pub fn run_as(command: &mut Command, uid: u32, gid: u32) -> io::Result<ExitStatus> {
    if !getuid().is_root() {
        return command.status();
    }
    let ids = move || -> io::Result<()> {
        rustix::thread::set_thread_groups(&[])
            .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
        let (gid, uid) = (Gid::from_raw(gid), Uid::from_raw(uid));
        rustix::thread::set_thread_res_gid(gid, gid, gid)
            .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
        rustix::thread::set_thread_res_uid(uid, uid, uid)
            .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
        Ok(())
    };
    // SAFETY: the hook runs in the forked child before `exec`. It only
    // makes system calls and never allocates, takes a lock or touches
    // state shared with other threads of the parent, which is what a
    // child of a multithreaded process may do.
    unsafe { command.pre_exec(ids) };
    command.status()
}

#[cfg(test)]
#[path = "as_user_tests.rs"]
mod tests;
