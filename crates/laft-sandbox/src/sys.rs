//! The system calls that `rustix` does not wrap safely, each behind a small
//! function whose `unsafe` block states why it is sound.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

use rustix::io::Errno;
use rustix::mount::MountAttrFlags;
use rustix::process::{Pid, WaitStatus};
use rustix::thread::UnshareFlags;

/// The kernel's `struct mount_attr` in its first version, the argument of
/// `mount_setattr(2)`.
#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

fn last_errno() -> Errno {
    Errno::from_io_error(&io::Error::last_os_error()).unwrap_or(Errno::IO)
}

/// Sets and clears per-mount attributes of the single mount `mount` refers
/// to. Unlike a bind remount, it leaves every attribute it is not asked
/// about alone, including those a user namespace has locked.
pub(crate) fn mount_setattr(
    mount: BorrowedFd<'_>,
    set: MountAttrFlags,
    clear: MountAttrFlags,
) -> Result<(), Errno> {
    let attr = MountAttr {
        attr_set: set.bits().into(),
        attr_clr: clear.bits().into(),
        propagation: 0,
        userns_fd: 0,
    };
    // SAFETY: `mount` is a live descriptor for the duration of the call, the
    // path is an empty NUL-terminated string, and `attr` is an initialized
    // `repr(C)` value whose layout and size are those of the kernel's
    // `struct mount_attr` version 0. The kernel only reads both pointers.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            mount.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            &raw const attr,
            size_of::<MountAttr>(),
        )
    };
    if ret == -1 { Err(last_errno()) } else { Ok(()) }
}

/// Moves the calling process into new namespaces of the kinds in `flags`.
///
/// No caller passes `UnshareFlags::FILES`. It is removed anyway as defence
/// in depth, because it is the flag that makes `unshare` unsafe.
pub(crate) fn unshare(flags: UnshareFlags) -> Result<(), Errno> {
    let flags = flags.difference(UnshareFlags::FILES);
    // SAFETY: `unshare` is only unsafe with `CLONE_FILES`, which could leave
    // threads with descriptors from another table. That flag was removed.
    unsafe { rustix::thread::unshare_unsafe(flags) }
}

/// Which side of a `fork` the caller is on.
pub(crate) enum Fork {
    /// The new child process.
    Child,
    /// The original process, with the child's pid.
    Parent(Pid),
}

/// Forks the calling process through the C library, which keeps its own
/// locks consistent in the child.
///
/// # Safety
///
/// If the calling process has other threads, the child may only use
/// functions that are safe after `fork` until it executes or exits.
pub(crate) unsafe fn fork() -> Result<Fork, Errno> {
    // SAFETY: the caller upholds the multithreading rule above, and `fork`
    // itself has no memory preconditions.
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => Err(last_errno()),
        0 => Ok(Fork::Child),
        pid => Pid::from_raw(pid).map(Fork::Parent).ok_or(Errno::INVAL),
    }
}

/// Marks every descriptor from `first` upward close-on-exec, so that
/// nothing the caller forgot to mark reaches the executed program.
pub(crate) fn cloexec_from(first: u32) -> Result<(), Errno> {
    // SAFETY: with `CLOSE_RANGE_CLOEXEC` the kernel only sets a flag on the
    // descriptors and closes none of them, so no owned descriptor becomes
    // dangling.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            first,
            u32::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        )
    };
    if ret == -1 { Err(last_errno()) } else { Ok(()) }
}

/// Closes every descriptor of the process.
///
/// # Safety
///
/// The caller never uses a descriptor again, including through an
/// `OwnedFd` that is still alive, and leaves only through [`exit_like`] or
/// [`exit`].
pub(crate) unsafe fn close_all() {
    // SAFETY: the caller guarantees that no descriptor is used afterwards,
    // so closing them cannot invalidate a live handle.
    unsafe { libc::syscall(libc::SYS_close_range, 0u32, u32::MAX, 0u32) };
}

/// Restores the default disposition of `signal`, so that a parent which
/// ignores it cannot change how the forked child behaves.
pub(crate) fn default_signal(signal: i32) {
    // SAFETY: installing `SIG_DFL` registers no handler code and touches no
    // memory of the process.
    unsafe { libc::signal(signal, libc::SIG_DFL) };
}

/// The real, effective and saved user ids of the calling thread.
pub(crate) fn getresuid() -> Result<[u32; 3], Errno> {
    let (mut real, mut effective, mut saved) = (0, 0, 0);
    // SAFETY: the three pointers refer to live, writable `uid_t` locals,
    // which are the only memory `getresuid` writes.
    let ret = unsafe { libc::getresuid(&mut real, &mut effective, &mut saved) };
    if ret == -1 {
        Err(last_errno())
    } else {
        Ok([real, effective, saved])
    }
}

/// The real, effective and saved group ids of the calling thread.
pub(crate) fn getresgid() -> Result<[u32; 3], Errno> {
    let (mut real, mut effective, mut saved) = (0, 0, 0);
    // SAFETY: the three pointers refer to live, writable `gid_t` locals,
    // which are the only memory `getresgid` writes.
    let ret = unsafe { libc::getresgid(&mut real, &mut effective, &mut saved) };
    if ret == -1 {
        Err(last_errno())
    } else {
        Ok([real, effective, saved])
    }
}

/// Ends the process immediately, without running exit handlers that belong
/// to the process it was forked from.
pub(crate) fn exit(code: i32) -> ! {
    // SAFETY: `_exit` has no preconditions and does not return.
    unsafe { libc::_exit(code) }
}

/// Ends the process the way `status` says its child ended: with the same
/// exit code, or killed by the same signal.
pub(crate) fn exit_like(status: WaitStatus) -> ! {
    if let Some(signal) = status.terminating_signal() {
        default_signal(signal);
        // SAFETY: sending a signal to the calling process touches no memory.
        unsafe { libc::kill(libc::getpid(), signal) };
        exit(128 + signal);
    }
    exit(status.exit_status().unwrap_or(1))
}

/// Brings up `lo`, the loopback interface of the calling process's network
/// namespace.
pub(crate) fn loopback_up() -> Result<(), Errno> {
    // SAFETY: `socket` takes only integer arguments and returns a new
    // descriptor or -1.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd == -1 {
        return Err(last_errno());
    }
    // SAFETY: `ifreq` is a plain C struct of integers, arrays and a union of
    // them, for which all zero bytes is a valid value.
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    for (to, from) in request.ifr_name.iter_mut().zip(b"lo") {
        *to = *from as libc::c_char;
    }
    request.ifr_ifru.ifru_flags = (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
    #[allow(
        clippy::unnecessary_cast,
        reason = "the request is a c_ulong with glibc and a c_int with musl"
    )]
    let set_flags = libc::SIOCSIFFLAGS as libc::Ioctl;
    // SAFETY: `fd` is the socket opened above, and `request` is an
    // initialized `ifreq` with a NUL-terminated name, which `SIOCSIFFLAGS`
    // only reads.
    let ret = unsafe { libc::ioctl(fd, set_flags, &raw mut request) };
    let result = if ret == -1 { Err(last_errno()) } else { Ok(()) };
    // SAFETY: `fd` is open and owned here, and nothing uses it afterwards.
    unsafe { libc::close(fd) };
    result
}
