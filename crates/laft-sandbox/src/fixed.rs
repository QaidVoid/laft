use std::ffi::{CStr, CString, OsStr};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::process;

use rustix::fs::{Mode, OFlags, ResolveFlags};
use rustix::io::Errno;
use rustix::mount::{FsMountFlags, FsOpenFlags, MountAttrFlags, MoveMountFlags};
use rustix::process::{Gid, Uid};

use crate::error::{Error, Step};
use crate::sandbox::{
    Command, Failure, Layout, ROOT_LINKS, attach, clone_tree, drop_capabilities, enter_root, fail,
    fresh_mount, mount_dev, new_root, resolve,
};
use crate::sys;

/// The uid and gid fixed programs run as: `nobody` and `nogroup` of the
/// static system accounts.
pub const NOBODY: u32 = 65534;

/// The mount points created in the root of a fixed program's sandbox.
const ROOT_DIRS: [&CStr; 10] = [
    c"usr",
    c"etc",
    c"proc",
    c"dev",
    c"tmp",
    c"var",
    c"home",
    c"root",
    c"run",
    c"run/laft",
];

/// The sandbox of a fixed program: a program from a tree under construction
/// that Laft runs with arguments it builds itself, such as a trigger that
/// computes a cache from the tree.
///
/// The program runs as [`NOBODY`] inside new mount, PID, IPC, UTS and
/// network namespaces, and sees only this:
///
/// | Path | Contents |
/// |---|---|
/// | `/` | an empty tmpfs, read-only |
/// | `/usr` | the tree's `usr`, read-only, `nosuid`, `nodev` |
/// | each writable directory | the same directory of the tree, writable, `nosuid`, `nodev` |
/// | `/etc` | the configuration directory given, read-only |
/// | `/bin`, `/sbin`, `/lib`, `/lib64` | symlinks into `usr` |
/// | `/run/laft/system`, `/run/laft/booted` | symlinks to `/`, so that paths through them resolve in the tree |
/// | `/proc`, `/dev` | as in a build [`crate::Sandbox`] |
/// | `/tmp` | a private tmpfs, mode `1777` |
/// | `/var`, `/home`, `/root` | private tmpfs mounts owned by [`NOBODY`] |
///
/// The network namespace has no usable interface. The capability bounding
/// set is emptied and `no_new_privs` is set. The environment holds exactly
/// `PATH=/usr/bin`, `LC_ALL=C.UTF-8`, `HOME=/nonexistent` and
/// `SOURCE_DATE_EPOCH=1`, and the working directory is `/`.
///
/// When the caller is root, no user namespace is made: the program switches
/// to the real uid and gid [`NOBODY`] without supplementary groups, so it can
/// write only to what [`NOBODY`] owns, and the caller makes the writable
/// directories [`NOBODY`]'s before the run. Otherwise a user namespace maps
/// [`NOBODY`] to the caller, which owns the tree already, and the read-only
/// mounts keep everything outside the writable directories unchanged.
///
/// # Examples
///
/// ```no_run
/// use laft_sandbox::FixedSandbox;
///
/// let sandbox = FixedSandbox::new("/laft/tmp/gen-42", "/laft/tmp/gen-42/etc")
///     .writable("usr/lib/modules/6.17.5-laft1");
/// let output = sandbox
///     .command("/usr/bin/depmod")
///     .args(["-a", "6.17.5-laft1"])
///     .output()?;
/// assert!(output.status.success());
/// # Ok::<(), laft_sandbox::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct FixedSandbox {
    tree: PathBuf,
    etc: PathBuf,
    writable: Vec<PathBuf>,
}

impl FixedSandbox {
    /// A sandbox whose `/usr` is `tree/usr` and whose `/etc` is `etc`, with
    /// no writable directory.
    pub fn new(tree: impl Into<PathBuf>, etc: impl Into<PathBuf>) -> Self {
        Self {
            tree: tree.into(),
            etc: etc.into(),
            writable: Vec::new(),
        }
    }

    /// Makes `dir` writable: a path relative to the tree below `usr`, such
    /// as `usr/lib/modules/6.17.5-laft1`. It must be a directory that is
    /// reached without symlinks, which is checked when a command is
    /// spawned.
    pub fn writable(mut self, dir: impl Into<PathBuf>) -> Self {
        self.writable.push(dir.into());
        self
    }

    /// A command that runs `program`, an absolute path inside the sandbox.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut inner = process::Command::new(program);
        inner
            .env_clear()
            .env("PATH", "/usr/bin")
            .env("LC_ALL", "C.UTF-8")
            .env("HOME", "/nonexistent")
            .env("SOURCE_DATE_EPOCH", "1");
        Command::new(Layout::Fixed(self.clone()), inner)
    }
}

/// What the child needs to build the root of a fixed program's sandbox.
pub(crate) struct FixedRoot {
    usr: CString,
    etc: CString,
    /// Each writable directory: its resolved path on the host and its path
    /// relative to the new root.
    writable: Vec<(CString, CString)>,
    /// Whether the child switches to [`NOBODY`] itself, because it runs as
    /// real root without a user namespace.
    switch_ids: bool,
}

impl FixedRoot {
    pub(crate) fn new(sandbox: &FixedSandbox, privileged: bool) -> Result<Self, Error> {
        let mut writable = Vec::new();
        for dir in &sandbox.writable {
            check_plain_dir(&sandbox.tree, dir)?;
            let target = CString::new(dir.as_os_str().as_bytes())
                .map_err(|_| Error::malformed(Step::InvalidWritable, dir))?;
            writable.push((resolve(&sandbox.tree.join(dir))?, target));
        }
        Ok(Self {
            usr: resolve(&sandbox.tree.join("usr"))?,
            etc: resolve(&sandbox.etc)?,
            writable,
            switch_ids: privileged,
        })
    }

    pub(crate) fn build(&self) -> Result<(), Failure> {
        let nosuid = MountAttrFlags::MOUNT_ATTR_NOSUID | MountAttrFlags::MOUNT_ATTR_NODEV;
        let readonly = nosuid | MountAttrFlags::MOUNT_ATTR_RDONLY;
        let usr = clone_tree(&self.usr, readonly)?;
        let etc = clone_tree(&self.etc, readonly)?;
        let root = new_root()?;
        for dir in ROOT_DIRS {
            rustix::fs::mkdirat(&root, dir, Mode::from_raw_mode(0o755))
                .map_err(fail(Step::Populate))?;
        }
        for (link, target) in ROOT_LINKS {
            rustix::fs::symlinkat(target, &root, link).map_err(fail(Step::Populate))?;
        }
        for link in [c"run/laft/system", c"run/laft/booted"] {
            rustix::fs::symlinkat(c"/", &root, link).map_err(fail(Step::Populate))?;
        }
        attach(&usr, &root, c"usr")?;
        // Each writable directory is cloned from the host and attached over
        // the same directory of the read-only `/usr`, one at a time, because
        // the child must not allocate.
        for (source, target) in &self.writable {
            let dir = clone_tree(source, nosuid)?;
            let at = rustix::fs::openat2(
                &root,
                target.as_c_str(),
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
            )
            .map_err(fail(Step::Writable))?;
            rustix::mount::move_mount(
                &dir,
                c"",
                &at,
                c"",
                MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH | MoveMountFlags::MOVE_MOUNT_T_EMPTY_PATH,
            )
            .map_err(fail(Step::Writable))?;
        }
        attach(&etc, &root, c"etc").map_err(|(_, errno)| (Step::BindEtc, errno))?;
        let proc = fresh_mount(c"proc", None, nosuid | MountAttrFlags::MOUNT_ATTR_NOEXEC)
            .map_err(fail(Step::MountProc))?;
        attach(&proc, &root, c"proc")?;
        mount_dev(&root, false)?;
        let tmp = fresh_mount(c"tmpfs", Some(c"1777"), nosuid).map_err(fail(Step::MountTmp))?;
        attach(&tmp, &root, c"tmp")?;
        for (dir, mode) in [(c"var", c"0755"), (c"home", c"0755"), (c"root", c"0700")] {
            let scratch = scratch_mount(mode).map_err(fail(Step::MountScratch))?;
            attach(&scratch, &root, dir)?;
        }
        enter_root(&root)?;
        drop_capabilities()?;
        if self.switch_ids {
            let (uid, gid) = (Uid::from_raw(NOBODY), Gid::from_raw(NOBODY));
            rustix::thread::set_thread_groups(&[]).map_err(fail(Step::SwitchIds))?;
            rustix::thread::set_thread_res_gid(gid, gid, gid).map_err(fail(Step::SwitchIds))?;
            rustix::thread::set_thread_res_uid(uid, uid, uid).map_err(fail(Step::SwitchIds))?;
        }
        rustix::thread::set_no_new_privs(true).map_err(fail(Step::NoNewPrivs))?;
        rustix::process::chdir(c"/").map_err(fail(Step::WorkingDirectory))?;
        sys::cloexec_from(3).map_err(fail(Step::Descriptors))
    }
}

/// A tmpfs owned by [`NOBODY`] with permissions `mode`.
fn scratch_mount(mode: &CStr) -> Result<OwnedFd, Errno> {
    let context = rustix::mount::fsopen(c"tmpfs", FsOpenFlags::FSOPEN_CLOEXEC)?;
    rustix::mount::fsconfig_set_string(&context, c"mode", mode)?;
    rustix::mount::fsconfig_set_string(&context, c"uid", c"65534")?;
    rustix::mount::fsconfig_set_string(&context, c"gid", c"65534")?;
    rustix::mount::fsconfig_create(&context)?;
    rustix::mount::fsmount(
        &context,
        FsMountFlags::FSMOUNT_CLOEXEC,
        MountAttrFlags::MOUNT_ATTR_NOSUID | MountAttrFlags::MOUNT_ATTR_NODEV,
    )
}

/// Checks that `dir` is a relative path below `usr` made of plain
/// components, and that every directory on the way from `tree` to it is a
/// real directory rather than a symlink, so that a link in the tree cannot
/// make a host directory writable.
fn check_plain_dir(tree: &Path, dir: &Path) -> Result<(), Error> {
    let mut components = dir.components();
    let below_usr = components.next() == Some(Component::Normal("usr".as_ref()))
        && components.clone().next().is_some()
        && components.all(|c| matches!(c, Component::Normal(_)));
    if !below_usr {
        return Err(Error::malformed(Step::InvalidWritable, dir));
    }
    let mut path = tree.to_path_buf();
    for component in dir.components() {
        path.push(component);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|e| Error::at(Step::Resolve, &path, e))?;
        if !metadata.is_dir() {
            return Err(Error::malformed(Step::InvalidWritable, dir));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "fixed_tests.rs"]
mod tests;
