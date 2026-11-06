//! Running a program as another user.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Output};

use rustix::process::{Gid, Uid, getgid, getuid};

/// Runs `command` as `uid` and `gid`, without supplementary groups, and
/// waits for it. The ids must be nonzero, and only root may switch to
/// another user: a caller that is not root fails when it asks for other
/// ids than its own. The child runs with `no_new_privs`, so setuid and
/// setgid bits on the program it execs are ignored.
///
/// The ids are switched in the child between `fork` and `exec`, so the
/// caller keeps its privileges.
///
/// # Errors
///
/// Fails when the ids are zero, when a caller that is not root asks for
/// other ids than its own, when the program cannot be spawned, or, in
/// the child, when `no_new_privs` cannot be set or the ids cannot be
/// switched.
pub fn run_as(command: &mut Command, uid: u32, gid: u32) -> io::Result<ExitStatus> {
    // SAFETY: the hook runs in the forked child before `exec`. It only
    // makes system calls and never allocates, takes a lock or touches
    // state shared with other threads of the parent, which is what a
    // child of a multithreaded process may do.
    unsafe { command.pre_exec(ids_hook(uid, gid)?) };
    command.status()
}

/// Runs `command` as `uid` and `gid`, as [`run_as`] does, and captures
/// its output. Standard error keeps the configuration of `command`, which
/// is inherited when it is not set.
///
/// # Errors
///
/// Fails as [`run_as`] does.
pub fn run_as_output(command: &mut Command, uid: u32, gid: u32) -> io::Result<Output> {
    // SAFETY: as in `run_as`.
    unsafe { command.pre_exec(ids_hook(uid, gid)?) };
    command.output()
}

/// Checks the ids and returns the hook that switches them in the child.
fn ids_hook(uid: u32, gid: u32) -> io::Result<impl FnMut() -> io::Result<()>> {
    if uid == 0 || gid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the target ids must be nonzero",
        ));
    }
    let switch = getuid().is_root();
    if !switch && (getuid().as_raw(), getgid().as_raw()) != (uid, gid) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "only root may run a program as another user",
        ));
    }
    Ok(move || -> io::Result<()> {
        rustix::thread::set_no_new_privs(true)
            .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
        if switch {
            rustix::thread::set_thread_groups(&[])
                .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
            let (gid, uid) = (Gid::from_raw(gid), Uid::from_raw(uid));
            rustix::thread::set_thread_res_gid(gid, gid, gid)
                .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
            rustix::thread::set_thread_res_uid(uid, uid, uid)
                .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
        }
        Ok(())
    })
}

#[cfg(test)]
#[path = "as_user_tests.rs"]
mod tests;
