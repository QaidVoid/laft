use std::fs::File;
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use rustix::io::Errno;
use rustix::pipe::PipeFlags;
use rustix::process::WaitOptions;
use rustix::thread::UnshareFlags;

use crate::sys::{self, Fork};

const PASSED: i32 = 0;
const FAILED: i32 = 1;
const SKIPPED: i32 = 2;

/// Runs `body` in a forked child that is root of its own new user and mount
/// namespace, so it may mount freely without touching the test runner's
/// namespace. A failure message from `body` fails the test. When the kernel
/// refuses the namespaces, the test is skipped with a printed reason.
pub(crate) fn in_user_namespace(body: impl FnOnce() -> Result<(), String>) {
    let (reader, writer) = rustix::pipe::pipe_with(PipeFlags::CLOEXEC).expect("pipes can be made");
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    // SAFETY: the child only allocates and makes system calls. The C library
    // keeps its allocator consistent across `fork`, and the child never
    // prints, reads the environment or returns into the test harness.
    match unsafe { sys::fork() }.expect("fork succeeds") {
        Fork::Child => {
            drop(reader);
            let (code, message) = match enter_user_namespace(uid, gid) {
                Err(errno) => (SKIPPED, format!("user namespaces are unavailable: {errno}")),
                Ok(()) => match catch_unwind(AssertUnwindSafe(body)) {
                    Ok(Ok(())) => (PASSED, String::new()),
                    Ok(Err(message)) => (FAILED, message),
                    Err(_) => (FAILED, "the test body panicked".to_owned()),
                },
            };
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
            match status.exit_status() {
                Some(PASSED) => {}
                Some(SKIPPED) => println!("skipping: {message}"),
                _ => panic!("{message} ({status:?})"),
            }
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

/// Whether a sandbox error means the kernel or its policy forbids the
/// namespaces, rather than a bug.
pub(crate) fn namespaces_refused(error: &crate::Error) -> bool {
    use crate::error::Step;
    matches!(error.step(), Step::Unshare | Step::MapIds)
        && matches!(
            Errno::from_io_error(error.io_error()),
            Some(Errno::PERM | Errno::ACCESS | Errno::NOSPC | Errno::INVAL | Errno::NOSYS)
        )
}
