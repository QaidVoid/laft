//! `laft-enter`, the setuid helper that runs a command inside a view.
//!
//! ```text
//! laft-enter <view> [command [args...]]
//! ```
//!
//! It runs `command` as the calling user in a private mount namespace in
//! which `/usr` is the tree of the current version of `view`. Without a
//! command it runs the caller's `SHELL` when that is an absolute path, and
//! `/bin/sh` otherwise. The view is mounted read-only, `nosuid` and `nodev`
//! over the active generation's `usr`, and the system's privileged programs
//! that the active generation allows for that view version are bound on
//! top. The rest of the file system, including `/etc`, is the host's, and
//! mounts the host makes later still appear inside.
//!
//! Root may enter every view. Anyone else must be listed in the view's
//! `view.json` by real uid, or by real or supplementary gid. The command
//! sees `LAFT_VIEW` set to the view name.
//!
//! The exit status is the command's, 126 when it cannot be executed, 127
//! when it is not found, and 1 for every error of this program, which is
//! printed on stderr after `laft-enter: `.
//!
//! The program is installed setuid root and treats its caller as hostile.
//! Before the privileged steps it marks every inherited descriptor above 2
//! close-on-exec, restores the default disposition of every signal, sets
//! the umask to 077, changes to `/` and copies the arguments and the
//! environment without interpreting them. The environment is read only
//! after privileges are dropped, with one exception: the allocator reads
//! its own `MIMALLOC_` tuning variables at startup. They change how memory
//! is obtained and what the allocator reports on stderr, never a decision
//! of this program. An allocation failure they cause aborts the program,
//! which leaves nothing behind outside its mount namespace. The binary is
//! static, so no dynamic loader runs with root privileges. The caller's
//! umask is restored right before the command is executed, so files the
//! command creates get the caller's usual permissions.

#![forbid(unsafe_code)]

mod access;
mod args;
mod command;
mod enter;
mod privileges;
mod records;

use std::ffi::OsString;
use std::fmt;

use rustix::fs::Mode;
use rustix::io::Errno;

use crate::access::Caller;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// The umask of every step before the command is executed.
const UMASK: u32 = 0o077;

/// The exit status of every error of this program.
const ERROR: i32 = 1;

/// Why the program stops without running the command, and how it exits.
#[derive(Debug)]
pub(crate) struct Failure {
    /// The exit status.
    status: i32,
    /// The message, printed after `laft-enter: `.
    message: String,
    /// The system call error behind it, if any.
    errno: Option<Errno>,
}

impl Failure {
    /// A failure that exits with `status`.
    pub(crate) fn new(status: i32, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            errno: None,
        }
    }

    /// An error of this program, exit status 1.
    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self::new(ERROR, message)
    }

    /// An error of this program caused by the system call error `errno`.
    pub(crate) fn os(message: impl Into<String>, errno: Errno) -> Self {
        Self {
            errno: Some(errno),
            ..Self::error(message)
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Prints a note that does not stop the program.
pub(crate) fn note(message: &str) {
    eprintln!("laft-enter: note: {message}");
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("laft-enter: internal error: {info}");
        std::process::exit(ERROR);
    }));
    let failure = run();
    eprintln!("laft-enter: {failure}");
    std::process::exit(failure.status);
}

/// Enters the view and executes the command. Returns only on failure.
fn run() -> Failure {
    if !rustix::process::geteuid().is_root() {
        return Failure::error("not installed setuid root");
    }
    let groups = match rustix::process::getgroups() {
        Ok(groups) => groups.into_iter().map(|gid| gid.as_raw()).collect(),
        Err(errno) => return Failure::os("reading the supplementary groups", errno),
    };
    let caller = Caller {
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        groups,
    };

    laft_sandbox::reset_signal_dispositions();
    if let Err(e) = laft_sandbox::close_inherited_on_exec() {
        return Failure::error(e.to_string());
    }
    let caller_umask = rustix::process::umask(Mode::from_raw_mode(UMASK));

    let args: Vec<OsString> = std::env::args_os().collect();
    let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    if !args::within_limits(&args, &env) {
        return Failure::error("environment too large");
    }
    let cwd = std::env::current_dir().ok();
    if let Err(e) = std::env::set_current_dir("/") {
        return Failure::error(format!("changing to /: {e}"));
    }
    let invocation = match args::parse(&args) {
        Ok(invocation) => invocation,
        Err(message) => return Failure::error(message),
    };

    let lock = match enter::enter(&invocation.view, &caller) {
        Ok(lock) => lock,
        Err(failure) => return failure,
    };
    if let Err(failure) = privileges::drop_to(&caller) {
        return failure;
    }
    let env = command::environment(env, &invocation.view);
    let failure = command::exec(invocation.command, env, cwd, caller_umask);
    drop(lock);
    failure
}
