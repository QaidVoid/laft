use std::ffi::{CStr, CString, OsStr};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Child, ExitStatus, Output, Stdio};

use rustix::fs::{CWD, Mode, OFlags};
use rustix::io::Errno;
use rustix::mount::{
    FsMountFlags, FsOpenFlags, MountAttrFlags, MountPropagationFlags, MoveMountFlags,
    OpenTreeFlags, UnmountFlags,
};
use rustix::pipe::PipeFlags;
use rustix::process::{Resource, Rlimit, Signal, WaitOptions};
use rustix::thread::{CapabilitySet, UnshareFlags};

use crate::error::{Error, Step};
use crate::fixed::{FixedRoot, FixedSandbox, NOBODY};
use crate::sys::{self, Fork};

/// The hostname and domain name inside every sandbox.
pub const HOSTNAME: &str = "laft-build";

/// The highest number of open files a sandboxed process may have, unless the
/// caller's own hard limit is lower.
pub const MAX_OPEN_FILES: u64 = 65536;

/// The device nodes bound into the sandbox's `/dev`.
const DEVICES: [(&CStr, &CStr); 6] = [
    (c"/dev/null", c"null"),
    (c"/dev/zero", c"zero"),
    (c"/dev/full", c"full"),
    (c"/dev/random", c"random"),
    (c"/dev/urandom", c"urandom"),
    (c"/dev/tty", c"tty"),
];

const DEVICE_LINKS: [(&CStr, &CStr); 4] = [
    (c"fd", c"/proc/self/fd"),
    (c"stdin", c"/proc/self/fd/0"),
    (c"stdout", c"/proc/self/fd/1"),
    (c"stderr", c"/proc/self/fd/2"),
];

/// The host files bound into `/etc` with [`Network::Host`], when the host
/// has them.
const HOST_ETC: [(&CStr, &CStr); 3] = [
    (c"/etc/resolv.conf", c"resolv.conf"),
    (c"/etc/hosts", c"hosts"),
    (c"/etc/nsswitch.conf", c"nsswitch.conf"),
];

const ROOT_DIRS: [&CStr; 6] = [c"usr", c"build", c"out", c"proc", c"dev", c"tmp"];

pub(crate) const ROOT_LINKS: [(&CStr, &CStr); 4] = [
    (c"bin", c"usr/bin"),
    (c"sbin", c"usr/bin"),
    (c"lib", c"usr/lib"),
    (c"lib64", c"usr/lib"),
];

/// How a sandbox reaches the network.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Network {
    /// A new network namespace whose only interface is a loopback interface
    /// that is down, so nothing is reachable. The sandbox has no `/etc`.
    #[default]
    Isolated,
    /// The caller's network namespace, for the phase that fetches sources.
    ///
    /// So that names resolve, the host's `/etc/resolv.conf`, `/etc/hosts`
    /// and `/etc/nsswitch.conf` are bound read-only into an otherwise empty,
    /// read-only `/etc`. A file the host lacks is left out.
    Host,
}

/// The layout of a build sandbox: a view as `/usr`, a writable `/build` and
/// a writable `/out`.
///
/// Every command started from a sandbox runs as uid 0 and gid 0 inside new
/// user, mount, PID, IPC and UTS namespaces, and a new network namespace
/// unless [`Network::Host`] is chosen. Those ids map to the caller's own uid
/// and gid, so files the command writes to `/build` and `/out` belong to the
/// caller. The command sees this tree and nothing else of the host:
///
/// | Path | Contents |
/// |---|---|
/// | `/` | an empty tmpfs, read-only |
/// | `/usr` | the view, read-only, `nosuid`, `nodev` |
/// | `/bin`, `/sbin` | symlinks to `usr/bin` |
/// | `/lib`, `/lib64` | symlinks to `usr/lib` |
/// | `/build` | the build directory, writable, the working directory |
/// | `/out` | the output directory, writable |
/// | `/proc` | a new procfs that shows only the sandbox's processes |
/// | `/dev` | a read-only tmpfs with `null`, `zero`, `full`, `random`, `urandom` and `tty` bound from the host, and `fd`, `stdin`, `stdout`, `stderr` links |
/// | `/tmp` | a private tmpfs |
/// | `/etc` | only with [`Network::Host`]: the host's name resolution files, read-only |
///
/// Before the command is executed, its capability bounding set is emptied,
/// `no_new_privs` is set, the core size limit is 0 and the open file limit
/// is [`MAX_OPEN_FILES`]. Its environment holds only `PATH=/usr/bin` and
/// what [`Command::env`] adds. The hostname is [`HOSTNAME`].
///
/// # Examples
///
/// ```no_run
/// use std::process::Stdio;
///
/// use laft_sandbox::Sandbox;
///
/// let sandbox = Sandbox::new("work/k/root/usr", "work/k/build", "work/k/out");
/// let output = sandbox
///     .command("/usr/bin/bash")
///     .arg("-c")
///     .arg("make && make DESTDIR=/out install")
///     .stdin(Stdio::null())
///     .output()?;
/// assert!(output.status.success());
/// # Ok::<(), laft_sandbox::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Sandbox {
    view: PathBuf,
    build: PathBuf,
    out: PathBuf,
    network: Network,
}

impl Sandbox {
    /// A sandbox with `view` as `/usr`, `build` as `/build` and `out` as
    /// `/out`, and no network.
    ///
    /// `view` is the `usr` tree of an assembled view. The directories are
    /// looked up when a command is spawned.
    pub fn new(
        view: impl Into<PathBuf>,
        build: impl Into<PathBuf>,
        out: impl Into<PathBuf>,
    ) -> Self {
        Self {
            view: view.into(),
            build: build.into(),
            out: out.into(),
            network: Network::Isolated,
        }
    }

    /// Chooses how commands reach the network.
    pub fn network(mut self, network: Network) -> Self {
        self.network = network;
        self
    }

    /// A command that runs `program` inside this sandbox.
    ///
    /// A `program` without a slash is searched in `PATH` inside the sandbox.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut inner = process::Command::new(program);
        inner.env_clear().env("PATH", "/usr/bin");
        Command::new(Layout::Build(self.clone()), inner)
    }
}

/// The kind of sandbox a command runs in.
#[derive(Debug, Clone)]
pub(crate) enum Layout {
    /// A build sandbox.
    Build(Sandbox),
    /// A sandbox for a fixed program.
    Fixed(FixedSandbox),
}

/// A command to run in a [`Sandbox`], configured like
/// [`std::process::Command`].
///
/// The sandbox fixes the working directory, the ids and the root, so only
/// the arguments, the environment and the standard streams can be set.
#[derive(Debug)]
pub struct Command {
    layout: Layout,
    inner: process::Command,
    stdin_set: bool,
}

impl Command {
    pub(crate) fn new(layout: Layout, inner: process::Command) -> Self {
        Self {
            layout,
            inner,
            stdin_set: false,
        }
    }

    /// Adds an argument.
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.inner.arg(arg);
        self
    }

    /// Adds several arguments.
    pub fn args(mut self, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Self {
        self.inner.args(args);
        self
    }

    /// Sets an environment variable, replacing an earlier value, including
    /// the default `PATH`.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.inner.env(key, value);
        self
    }

    /// Sets the command's standard input. It is inherited by default, except
    /// by [`Command::output`].
    pub fn stdin(mut self, stdin: impl Into<Stdio>) -> Self {
        self.inner.stdin(stdin);
        self.stdin_set = true;
        self
    }

    /// Sets the command's standard output. It is inherited by default.
    pub fn stdout(mut self, stdout: impl Into<Stdio>) -> Self {
        self.inner.stdout(stdout);
        self
    }

    /// Sets the command's standard error. It is inherited by default.
    pub fn stderr(mut self, stderr: impl Into<Stdio>) -> Self {
        self.inner.stderr(stderr);
        self
    }

    /// Sets up the sandbox and starts the command in it.
    ///
    /// The returned [`Child`] is a small process outside the sandbox that
    /// waits for the command and exits the way it did, with the same code
    /// or killed by the same signal. The command itself runs as PID 1 of the
    /// sandbox, so it should reap the processes it starts, as shells do.
    /// Killing the child with [`Child::kill`] kills every process of the
    /// sandbox. Dropping it does not.
    ///
    /// Use piped standard streams to read the command's output while it
    /// runs.
    ///
    /// `SIGCHLD` has its default disposition in the sandbox, even when the
    /// caller ignores it, so the command can wait for its own children.
    ///
    /// # Errors
    ///
    /// Fails when a directory of the sandbox does not exist, when a step of
    /// the setup fails, for example because the kernel forbids unprivileged
    /// user namespaces, or when the program cannot be executed. The error
    /// names the step.
    pub fn spawn(self) -> Result<Child, Error> {
        let Self {
            layout, mut inner, ..
        } = self;
        let setup = Setup::new(&layout)?;
        let (reader, writer) =
            rustix::pipe::pipe_with(PipeFlags::CLOEXEC).map_err(|e| Error::new(Step::Pipe, e))?;
        let hook = move || setup.run(writer.as_fd());
        // SAFETY: the hook runs in the forked child before `exec`. It only
        // makes system calls on data prepared before the fork and never
        // allocates, takes a lock or touches state shared with other threads
        // of the parent, which is what a child of a multithreaded process
        // may do.
        unsafe { inner.pre_exec(hook) };
        let spawned = inner.spawn();
        drop(inner);
        spawned.map_err(|error| {
            let mut code = [0u8];
            // The setup hook reports its step before failing. Without a
            // report, the hook succeeded and `exec` failed.
            let step = match rustix::io::read(&reader, &mut code) {
                Ok(1) => Step::from_code(code[0]),
                _ => Step::Exec,
            };
            Error::new(step, error)
        })
    }

    /// Runs the command to completion with captured standard output and
    /// standard error, and returns them with its exit status.
    ///
    /// Standard input is null unless [`Command::stdin`] set it, so a command
    /// that reads it sees end of file at once.
    ///
    /// # Errors
    ///
    /// Fails like [`Command::spawn`], or when reading the output fails.
    pub fn output(mut self) -> Result<Output, Error> {
        if !self.stdin_set {
            self.inner.stdin(Stdio::null());
        }
        let child = self.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        child
            .wait_with_output()
            .map_err(|e| Error::new(Step::Wait, e))
    }

    /// Runs the command to completion and returns its exit status.
    ///
    /// # Errors
    ///
    /// Fails like [`Command::spawn`], or when waiting for it fails.
    pub fn status(self) -> Result<ExitStatus, Error> {
        self.spawn()?.wait().map_err(|e| Error::new(Step::Wait, e))
    }
}

/// Everything the forked child needs, prepared before the fork so that the
/// child never allocates.
struct Setup {
    namespaces: UnshareFlags,
    /// The id maps of the new user namespace, or `None` when the caller is
    /// root and needs none.
    id_maps: Option<(Vec<u8>, Vec<u8>)>,
    open_files: u64,
    root: RootSetup,
}

/// How the child builds its root.
enum RootSetup {
    Build(BuildRoot),
    Fixed(FixedRoot),
}

/// The root of a build sandbox.
struct BuildRoot {
    view: CString,
    build: CString,
    out: CString,
    host_etc: bool,
}

pub(crate) type Failure = (Step, Errno);

pub(crate) fn fail(step: Step) -> impl Fn(Errno) -> Failure {
    move |errno| (step, errno)
}

/// `path` with every symlink resolved, as a C string for the child.
pub(crate) fn resolve(path: &Path) -> Result<CString, Error> {
    let path = std::fs::canonicalize(path).map_err(|e| Error::at(Step::Resolve, path, e))?;
    CString::new(path.as_os_str().as_bytes())
        .map_err(|e| Error::at(Step::Resolve, &path, std::io::Error::other(e)))
}

impl Setup {
    fn new(layout: &Layout) -> Result<Self, Error> {
        let mut namespaces = UnshareFlags::NEWNS
            | UnshareFlags::NEWPID
            | UnshareFlags::NEWIPC
            | UnshareFlags::NEWUTS;
        let uid = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        let (root, inside, user_namespace) = match layout {
            Layout::Build(sandbox) => {
                if sandbox.network == Network::Isolated {
                    namespaces |= UnshareFlags::NEWNET;
                }
                let root = RootSetup::Build(BuildRoot {
                    view: resolve(&sandbox.view)?,
                    build: resolve(&sandbox.build)?,
                    out: resolve(&sandbox.out)?,
                    host_etc: sandbox.network == Network::Host,
                });
                (root, 0, true)
            }
            Layout::Fixed(sandbox) => {
                namespaces |= UnshareFlags::NEWNET;
                let privileged = rustix::process::geteuid().is_root();
                let root = RootSetup::Fixed(FixedRoot::new(sandbox, privileged)?);
                (root, NOBODY, !privileged)
            }
        };
        let id_maps = user_namespace.then(|| {
            namespaces |= UnshareFlags::NEWUSER;
            (
                format!("{inside} {uid} 1\n").into_bytes(),
                format!("{inside} {gid} 1\n").into_bytes(),
            )
        });
        let hard = rustix::process::getrlimit(Resource::Nofile).maximum;
        Ok(Self {
            namespaces,
            id_maps,
            open_files: hard.map_or(MAX_OPEN_FILES, |hard| hard.min(MAX_OPEN_FILES)),
            root,
        })
    }

    /// Runs in the child of `fork`. On failure it sends the failed step to
    /// the parent through `report` before the standard library reports the
    /// error number.
    fn run(&self, report: BorrowedFd<'_>) -> std::io::Result<()> {
        self.enter().map_err(|(step, errno)| {
            let _ = rustix::io::write(report, &[step as u8]);
            errno.into()
        })
    }

    fn enter(&self) -> Result<(), Failure> {
        sys::unshare(self.namespaces).map_err(fail(Step::Unshare))?;
        if let Some((uid_map, gid_map)) = &self.id_maps {
            write_proc(c"/proc/self/setgroups", b"deny")?;
            write_proc(c"/proc/self/uid_map", uid_map)?;
            write_proc(c"/proc/self/gid_map", gid_map)?;
        }
        let open_files = Rlimit {
            current: Some(self.open_files),
            maximum: Some(self.open_files),
        };
        let no_core = Rlimit {
            current: Some(0),
            maximum: Some(0),
        };
        rustix::process::setrlimit(Resource::Nofile, open_files).map_err(fail(Step::Limits))?;
        rustix::process::setrlimit(Resource::Core, no_core).map_err(fail(Step::Limits))?;

        // A caller that ignores SIGCHLD makes the kernel reap children on its
        // own, so `wait_for` could not learn how PID 1 ended. The ignored
        // disposition would also survive the fork and `exec` into the
        // command, which then could not wait for its own children.
        sys::default_signal(libc::SIGCHLD);
        // The new PID namespace applies to children only. This process stays
        // outside as the one the caller waits for, and the forked child
        // becomes PID 1 of the sandbox.
        // SAFETY: both sides of the fork only make system calls and never
        // allocate or take locks.
        match unsafe { sys::fork() }.map_err(fail(Step::Fork))? {
            Fork::Parent(init) => wait_for(init),
            Fork::Child => {
                prepare_init()?;
                match &self.root {
                    RootSetup::Build(root) => root.build(),
                    RootSetup::Fixed(root) => root.build(),
                }
            }
        }
    }
}

/// Ties PID 1 of the sandbox to the waiting process and makes every mount
/// private.
fn prepare_init() -> Result<(), Failure> {
    // When the waiting process dies, for example from `Child::kill`, the
    // kernel kills PID 1 and with it the whole namespace.
    rustix::process::set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(fail(Step::DeathSignal))?;
    rustix::mount::mount_change(
        c"/",
        MountPropagationFlags::PRIVATE | MountPropagationFlags::REC,
    )
    .map_err(fail(Step::Propagation))
}

impl BuildRoot {
    fn build(&self) -> Result<(), Failure> {
        let nosuid = MountAttrFlags::MOUNT_ATTR_NOSUID | MountAttrFlags::MOUNT_ATTR_NODEV;
        let view = clone_tree(&self.view, nosuid | MountAttrFlags::MOUNT_ATTR_RDONLY)?;
        let build = clone_tree(&self.build, nosuid)?;
        let out = clone_tree(&self.out, nosuid)?;

        // The new root is stacked on top of the old one. Paths keep resolving
        // in the old root until `pivot_root`, so host device nodes can still
        // be cloned below.
        let root = new_root()?;
        for dir in ROOT_DIRS {
            rustix::fs::mkdirat(&root, dir, Mode::from_raw_mode(0o755))
                .map_err(fail(Step::Populate))?;
        }
        for (link, target) in ROOT_LINKS {
            rustix::fs::symlinkat(target, &root, link).map_err(fail(Step::Populate))?;
        }
        attach(&view, &root, c"usr")?;
        attach(&build, &root, c"build")?;
        attach(&out, &root, c"out")?;

        let proc = fresh_mount(c"proc", None, nosuid | MountAttrFlags::MOUNT_ATTR_NOEXEC)
            .map_err(fail(Step::MountProc))?;
        attach(&proc, &root, c"proc")?;
        mount_dev(&root)?;
        let tmp = fresh_mount(c"tmpfs", Some(c"1777"), nosuid).map_err(fail(Step::MountTmp))?;
        attach(&tmp, &root, c"tmp")?;
        if self.host_etc {
            mount_host_etc(&root)?;
        }
        enter_root(&root)?;
        drop_capabilities()?;
        rustix::thread::set_no_new_privs(true).map_err(fail(Step::NoNewPrivs))?;
        rustix::process::chdir(c"/build").map_err(fail(Step::WorkingDirectory))?;
        sys::cloexec_from(3).map_err(fail(Step::Descriptors))
    }
}

/// Creates an empty tmpfs and stacks it on `/`, where it becomes the root
/// once [`enter_root`] runs. Paths keep resolving in the old root until
/// then.
pub(crate) fn new_root() -> Result<OwnedFd, Failure> {
    let nosuid = MountAttrFlags::MOUNT_ATTR_NOSUID | MountAttrFlags::MOUNT_ATTR_NODEV;
    let root = fresh_mount(c"tmpfs", Some(c"0755"), nosuid).map_err(fail(Step::RootFs))?;
    rustix::mount::move_mount(
        &root,
        c"",
        CWD,
        c"/",
        MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
    )
    .map_err(fail(Step::RootFs))?;
    Ok(root)
}

/// Makes `root` read-only, switches to it, detaches the old root and sets
/// the hostname.
pub(crate) fn enter_root(root: &OwnedFd) -> Result<(), Failure> {
    sys::mount_setattr(
        root.as_fd(),
        MountAttrFlags::MOUNT_ATTR_RDONLY,
        MountAttrFlags::empty(),
    )
    .map_err(fail(Step::SetAttributes))?;

    // With both arguments ".", the old root ends up stacked on the new
    // one, and detaching "." removes it.
    rustix::process::fchdir(root).map_err(fail(Step::PivotRoot))?;
    rustix::process::pivot_root(c".", c".").map_err(fail(Step::PivotRoot))?;
    rustix::mount::unmount(c".", UnmountFlags::DETACH).map_err(fail(Step::PivotRoot))?;
    rustix::process::chdir(c"/").map_err(fail(Step::PivotRoot))?;

    rustix::system::sethostname(HOSTNAME.as_bytes()).map_err(fail(Step::Hostname))?;
    rustix::system::setdomainname(HOSTNAME.as_bytes()).map_err(fail(Step::Hostname))
}

/// Mounts the sandbox's `/dev` in `root`.
pub(crate) fn mount_dev(root: &OwnedFd) -> Result<(), Failure> {
    let dev = fresh_mount(
        c"tmpfs",
        Some(c"0755"),
        MountAttrFlags::MOUNT_ATTR_NOSUID | MountAttrFlags::MOUNT_ATTR_NOEXEC,
    )
    .map_err(fail(Step::MountDev))?;
    attach(&dev, root, c"dev")?;
    for (source, name) in DEVICES {
        let node = rustix::mount::open_tree(
            CWD,
            source,
            OpenTreeFlags::OPEN_TREE_CLONE | OpenTreeFlags::OPEN_TREE_CLOEXEC,
        )
        .map_err(fail(Step::MountDev))?;
        rustix::fs::openat(
            &dev,
            name,
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o666),
        )
        .map_err(fail(Step::MountDev))?;
        attach(&node, &dev, name)?;
    }
    for (link, target) in DEVICE_LINKS {
        rustix::fs::symlinkat(target, &dev, link).map_err(fail(Step::MountDev))?;
    }
    sys::mount_setattr(
        dev.as_fd(),
        MountAttrFlags::MOUNT_ATTR_RDONLY,
        MountAttrFlags::empty(),
    )
    .map_err(fail(Step::MountDev))
}

/// Creates `/etc` in `root` and binds each file of [`HOST_ETC`] the host has
/// into it, read-only. The directory becomes read-only with the root.
fn mount_host_etc(root: &OwnedFd) -> Result<(), Failure> {
    rustix::fs::mkdirat(root, c"etc", Mode::from_raw_mode(0o755)).map_err(fail(Step::Populate))?;
    let etc = rustix::fs::openat(
        root,
        c"etc",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(fail(Step::Populate))?;
    for (source, name) in HOST_ETC {
        let file = match rustix::mount::open_tree(
            CWD,
            source,
            OpenTreeFlags::OPEN_TREE_CLONE | OpenTreeFlags::OPEN_TREE_CLOEXEC,
        ) {
            Ok(file) => file,
            Err(Errno::NOENT) => continue,
            Err(errno) => return Err((Step::MountEtc, errno)),
        };
        sys::mount_setattr(
            file.as_fd(),
            MountAttrFlags::MOUNT_ATTR_RDONLY
                | MountAttrFlags::MOUNT_ATTR_NOSUID
                | MountAttrFlags::MOUNT_ATTR_NODEV
                | MountAttrFlags::MOUNT_ATTR_NOEXEC,
            MountAttrFlags::empty(),
        )
        .map_err(fail(Step::MountEtc))?;
        rustix::fs::openat(
            &etc,
            name,
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
        )
        .map_err(fail(Step::MountEtc))?;
        attach(&file, &etc, name)?;
    }
    Ok(())
}

fn write_proc(path: &CStr, data: &[u8]) -> Result<(), Failure> {
    let file = rustix::fs::open(path, OFlags::WRONLY | OFlags::CLOEXEC, Mode::empty())
        .map_err(fail(Step::MapIds))?;
    let written = rustix::io::write(&file, data).map_err(fail(Step::MapIds))?;
    if written == data.len() {
        Ok(())
    } else {
        Err((Step::MapIds, Errno::IO))
    }
}

/// Waits for PID 1 of the sandbox and ends this process the same way.
fn wait_for(init: rustix::process::Pid) -> ! {
    // SAFETY: this process never returns to the standard library. It only
    // waits and exits, so no descriptor is used after this point. Closing
    // them lets the parent see end of file on the command's pipes and the
    // standard library's exec status pipe as soon as the command holds the
    // last copy.
    unsafe { sys::close_all() };
    loop {
        match rustix::process::waitpid(Some(init), WaitOptions::empty()) {
            Ok(Some((_, status))) => sys::exit_like(status),
            Err(Errno::INTR) => {}
            _ => sys::exit(1),
        }
    }
}

pub(crate) fn clone_tree(path: &CStr, attributes: MountAttrFlags) -> Result<OwnedFd, Failure> {
    let tree = rustix::mount::open_tree(
        CWD,
        path,
        OpenTreeFlags::OPEN_TREE_CLONE | OpenTreeFlags::OPEN_TREE_CLOEXEC,
    )
    .map_err(fail(Step::CloneTree))?;
    sys::mount_setattr(tree.as_fd(), attributes, MountAttrFlags::empty())
        .map_err(fail(Step::SetAttributes))?;
    Ok(tree)
}

pub(crate) fn fresh_mount(
    filesystem: &CStr,
    mode: Option<&CStr>,
    attributes: MountAttrFlags,
) -> Result<OwnedFd, Errno> {
    let context = rustix::mount::fsopen(filesystem, FsOpenFlags::FSOPEN_CLOEXEC)?;
    if let Some(mode) = mode {
        rustix::mount::fsconfig_set_string(&context, c"mode", mode)?;
    }
    rustix::mount::fsconfig_create(&context)?;
    rustix::mount::fsmount(&context, FsMountFlags::FSMOUNT_CLOEXEC, attributes)
}

pub(crate) fn attach(mount: &OwnedFd, parent: &OwnedFd, name: &CStr) -> Result<(), Failure> {
    rustix::mount::move_mount(
        mount,
        c"",
        parent,
        name,
        MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
    )
    .map_err(fail(Step::Attach))
}

/// Empties the bounding set, so that executing the command as uid 0 grants
/// it no capabilities, and clears the ambient set.
pub(crate) fn drop_capabilities() -> Result<(), Failure> {
    for bit in 0..u64::BITS {
        match rustix::thread::remove_capability_from_bounding_set(CapabilitySet::from_bits_retain(
            1 << bit,
        )) {
            Ok(()) => {}
            // Numbers past the kernel's last capability are rejected.
            Err(Errno::INVAL) => break,
            Err(errno) => return Err((Step::Capabilities, errno)),
        }
    }
    rustix::thread::clear_ambient_capability_set().map_err(fail(Step::Capabilities))
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
