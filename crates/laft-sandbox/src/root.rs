use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::process;

use rustix::fs::{CWD, Mode, OFlags};
use rustix::mount::{MountAttrFlags, MoveMountFlags};

use crate::error::{Error, Step};
use crate::sandbox::{
    Command, Failure, Layout, attach, clone_tree, drop_capabilities, fail, fresh_mount, mount_dev,
    resolve, switch_root,
};
use crate::sys;

/// The directories of the tree that the sandbox mounts over.
const MOUNT_POINTS: [&str; 3] = ["proc", "dev", "run"];

/// A sandbox whose root is an existing directory tree, such as a system
/// under construction, which commands run in as its root user.
///
/// Every command runs as uid 0 and gid 0 inside new user, mount, PID, IPC,
/// UTS and network namespaces. Those ids map to the caller's own uid and
/// gid, so the files the command writes belong to the caller, and it cannot
/// give a file to another user. It sees this tree and nothing else of the
/// host:
///
/// | Path | Contents |
/// |---|---|
/// | `/` | the tree, writable, `nosuid`, `nodev` |
/// | `/proc` | a new procfs that shows only the sandbox's processes |
/// | `/dev` | as in a build [`crate::Sandbox`] |
/// | `/run` | a private tmpfs |
/// | `/run/<name>` | each directory or file of [`RootSandbox::bind`], read-only unless it is writable |
///
/// The network namespace has no usable interface. Before the command is
/// executed, its capability bounding set is emptied and `no_new_privs` is
/// set. Its environment holds only `PATH=/usr/bin` and what
/// [`Command::env`] adds, and its working directory is `/`.
///
/// The mounts exist only in the sandbox's mount namespace. The tree's
/// `proc`, `dev` and `run` directories are created when a command is
/// spawned if they are missing, and stay empty on the host.
///
/// # Examples
///
/// ```no_run
/// use laft_sandbox::RootSandbox;
///
/// let sandbox = RootSandbox::new("/home/me/system").bind("/home/me/src", "src", false);
/// let status = sandbox
///     .command("/usr/bin/sh")
///     .args(["-c", "cd /run/src && make install"])
///     .status()?;
/// assert!(status.success());
/// # Ok::<(), laft_sandbox::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct RootSandbox {
    root: PathBuf,
    binds: Vec<Bind>,
}

/// A directory or file of the host shown below `/run`.
#[derive(Debug, Clone)]
struct Bind {
    source: PathBuf,
    name: PathBuf,
    writable: bool,
}

impl RootSandbox {
    /// A sandbox whose root is the directory `root`, without binds.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            binds: Vec::new(),
        }
    }

    /// Shows `source`, a directory or a file of the host, as `/run/<name>`,
    /// writable or read-only. `name` is a single file name, which is
    /// checked when a command is spawned.
    pub fn bind(
        mut self,
        source: impl Into<PathBuf>,
        name: impl Into<PathBuf>,
        writable: bool,
    ) -> Self {
        self.binds.push(Bind {
            source: source.into(),
            name: name.into(),
            writable,
        });
        self
    }

    /// A command that runs `program`, a path inside the tree.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut inner = process::Command::new(program);
        inner.env_clear().env("PATH", "/usr/bin");
        Command::new(Layout::Root(self.clone()), inner)
    }
}

/// What the child needs to enter the tree of a [`RootSandbox`].
pub(crate) struct RootTree {
    root: CString,
    binds: Vec<BindTarget>,
}

/// A bind, resolved before the fork.
struct BindTarget {
    source: CString,
    name: CString,
    dir: bool,
    writable: bool,
}

impl RootTree {
    pub(crate) fn new(sandbox: &RootSandbox) -> Result<Self, Error> {
        for dir in MOUNT_POINTS {
            let path = sandbox.root.join(dir);
            std::fs::create_dir_all(&path).map_err(|e| Error::at(Step::Populate, &path, e))?;
        }
        let mut binds = Vec::new();
        for bind in &sandbox.binds {
            let mut components = bind.name.components();
            let plain = matches!(
                (components.next(), components.next()),
                (Some(Component::Normal(_)), None)
            );
            if !plain {
                return Err(Error::malformed(Step::InvalidBindName, &bind.name));
            }
            let dir = std::fs::metadata(&bind.source)
                .map_err(|e| Error::at(Step::Resolve, &bind.source, e))?
                .is_dir();
            binds.push(BindTarget {
                source: resolve(&bind.source)?,
                name: c_name(&bind.name)?,
                dir,
                writable: bind.writable,
            });
        }
        Ok(Self {
            root: resolve(&sandbox.root)?,
            binds,
        })
    }

    /// Runs in the child: stacks a clone of the tree on `/`, mounts
    /// `/proc`, `/dev` and `/run` with the binds in it, and switches to it.
    pub(crate) fn build(&self) -> Result<(), Failure> {
        let nosuid = MountAttrFlags::MOUNT_ATTR_NOSUID | MountAttrFlags::MOUNT_ATTR_NODEV;
        let root = clone_tree(&self.root, nosuid)?;
        rustix::mount::move_mount(
            &root,
            c"",
            CWD,
            c"/",
            MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
        )
        .map_err(fail(Step::RootFs))?;

        let run = fresh_mount(c"tmpfs", Some(c"0755"), nosuid).map_err(fail(Step::MountRun))?;
        for bind in &self.binds {
            let attributes = if bind.writable {
                nosuid
            } else {
                nosuid | MountAttrFlags::MOUNT_ATTR_RDONLY
            };
            let tree = clone_tree(&bind.source, attributes)?;
            if bind.dir {
                rustix::fs::mkdirat(&run, bind.name.as_c_str(), Mode::from_raw_mode(0o755))
                    .map_err(fail(Step::Populate))?;
            } else {
                rustix::fs::openat(
                    &run,
                    bind.name.as_c_str(),
                    OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC,
                    Mode::from_raw_mode(0o644),
                )
                .map_err(fail(Step::Populate))?;
            }
            attach(&tree, &run, bind.name.as_c_str())?;
        }
        attach(&run, &root, c"run")?;
        let proc = fresh_mount(c"proc", None, nosuid | MountAttrFlags::MOUNT_ATTR_NOEXEC)
            .map_err(fail(Step::MountProc))?;
        attach(&proc, &root, c"proc")?;
        mount_dev(&root, true)?;

        switch_root(&root)?;
        drop_capabilities()?;
        rustix::thread::set_no_new_privs(true).map_err(fail(Step::NoNewPrivs))?;
        rustix::process::chdir(c"/").map_err(fail(Step::WorkingDirectory))?;
        sys::cloexec_from(3).map_err(fail(Step::Descriptors))
    }
}

fn c_name(name: &Path) -> Result<CString, Error> {
    CString::new(name.as_os_str().as_bytes())
        .map_err(|_| Error::malformed(Step::InvalidBindName, name))
}

#[cfg(test)]
#[path = "root_tests.rs"]
mod tests;
