use std::fs::File;
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use rustix::io::Errno;
use rustix::mount::{FsMountFlags, FsOpenFlags, MountAttrFlags, MountPropagationFlags};
use rustix::pipe::PipeFlags;
use rustix::process::WaitOptions;
use rustix::thread::UnshareFlags;

use crate::error::Step;
use crate::sys::{self, Fork};

const PASSED: i32 = 0;
const FAILED: i32 = 1;
const SKIPPED: i32 = 2;

/// Runs `body` in a forked child that is root of its own new user and mount
/// namespace, so it may mount freely without touching the test runner's
/// namespace. A failure message from `body` fails the test. When this
/// environment cannot mount in a user namespace, the test is skipped with a
/// printed reason.
pub(crate) fn in_user_namespace(body: impl FnOnce() -> Result<(), String>) {
    if let Some(reason) = mounting_refused() {
        println!("skipping: {reason}");
        return;
    }
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    let (status, message) = in_child(move || match enter_user_namespace(uid, gid) {
        Err(errno) => (FAILED, format!("entering a user namespace: {errno}")),
        Ok(()) => match catch_unwind(AssertUnwindSafe(body)) {
            Ok(Ok(())) => (PASSED, String::new()),
            Ok(Err(message)) => (FAILED, message),
            Err(_) => (FAILED, "the test body panicked".to_owned()),
        },
    });
    if status != Some(PASSED) {
        panic!("{message} ({status:?})");
    }
}

/// Why a sandbox error means this environment cannot run sandboxes rather
/// than a bug, or `None` when it does not.
///
/// The kernel or its policy may refuse the namespaces with `EPERM`, `EACCES`
/// or `ENOSPC`. A policy may also allow user namespaces but refuse to mount
/// in them, which a separate probe confirms.
pub(crate) fn sandbox_refused(error: &crate::Error) -> Option<String> {
    let errno = error.io_error().and_then(Errno::from_io_error)?;
    if matches!(error.step(), Step::Unshare | Step::MapIds) && refusal(errno) {
        return Some(format!("user namespaces are unavailable: {error}"));
    }
    if matches!(errno, Errno::PERM | Errno::ACCESS) {
        return mounting_refused();
    }
    None
}

fn refusal(errno: Errno) -> bool {
    matches!(errno, Errno::PERM | Errno::ACCESS | Errno::NOSPC)
}

/// Why this environment cannot mount inside a new user namespace, or `None`
/// when it can. The probe runs once, in a forked child.
///
/// # Panics
///
/// Panics when the probe fails for a reason other than a refusal.
fn mounting_refused() -> Option<String> {
    static REASON: OnceLock<Option<String>> = OnceLock::new();
    REASON.get_or_init(probe).clone()
}

fn probe() -> Option<String> {
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    let (status, message) = in_child(move || {
        let skip = |what: &str, errno: Errno| (SKIPPED, format!("{what}: {errno}"));
        match enter_user_namespace(uid, gid) {
            Err(errno) if refusal(errno) => return skip("user namespaces are unavailable", errno),
            Err(errno) => return (FAILED, format!("entering a user namespace: {errno}")),
            Ok(()) => {}
        }
        match mount_in_namespace() {
            Ok(()) => (PASSED, String::new()),
            Err(errno @ (Errno::PERM | Errno::ACCESS)) => {
                skip("mounting is refused in user namespaces", errno)
            }
            Err(errno @ Errno::NOSYS) => skip("the kernel lacks the mount API", errno),
            Err(errno) => (FAILED, format!("mounting in a user namespace: {errno}")),
        }
    });
    match status {
        Some(PASSED) => None,
        Some(SKIPPED) => Some(message),
        _ => panic!("{message} ({status:?})"),
    }
}

/// Makes a detached tmpfs mount and changes the propagation of `/`, the two
/// kinds of mount call a sandbox makes. Both stay in the probe's own mount
/// namespace.
fn mount_in_namespace() -> Result<(), Errno> {
    let context = rustix::mount::fsopen(c"tmpfs", FsOpenFlags::FSOPEN_CLOEXEC)?;
    rustix::mount::fsconfig_create(&context)?;
    rustix::mount::fsmount(
        &context,
        FsMountFlags::FSMOUNT_CLOEXEC,
        MountAttrFlags::empty(),
    )?;
    rustix::mount::mount_change(
        c"/",
        MountPropagationFlags::PRIVATE | MountPropagationFlags::REC,
    )
}

/// Runs `body` in a forked child and returns the exit code and message it
/// produced.
fn in_child(body: impl FnOnce() -> (i32, String)) -> (Option<i32>, String) {
    let (reader, writer) = rustix::pipe::pipe_with(PipeFlags::CLOEXEC).expect("pipes can be made");
    // SAFETY: the child only allocates and makes system calls. The C library
    // keeps its allocator consistent across `fork`, and the child never
    // prints, reads the environment or returns into the test harness.
    match unsafe { sys::fork() }.expect("fork succeeds") {
        Fork::Child => {
            drop(reader);
            let (code, message) = body();
            let _ = File::from(writer).write_all(message.as_bytes());
            sys::exit(code)
        }
        Fork::Parent(pid) => {
            drop(writer);
            let mut message = String::new();
            File::from(reader)
                .read_to_string(&mut message)
                .expect("the child's message is readable");
            let (_, status) = rustix::process::waitpid(Some(pid), WaitOptions::empty())
                .expect("the child can be waited for")
                .expect("the child has exited");
            (status.exit_status(), message)
        }
    }
}

fn enter_user_namespace(uid: u32, gid: u32) -> Result<(), Errno> {
    sys::unshare(UnshareFlags::NEWUSER | UnshareFlags::NEWNS)?;
    let write = |path: &str, data: String| {
        std::fs::write(path, data).map_err(|e| Errno::from_io_error(&e).unwrap_or(Errno::IO))
    };
    write("/proc/self/setgroups", "deny".to_owned())?;
    write("/proc/self/uid_map", format!("0 {uid} 1\n"))?;
    write("/proc/self/gid_map", format!("0 {gid} 1\n"))
}
