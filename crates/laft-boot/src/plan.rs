//! The mounts and runtime files of a boot as plans of operations.
//!
//! Each step that changes the mount table is first written down as a list of
//! [`Op`]s by a pure function, then carried out by [`execute`]. The plans
//! take the root of the initramfs and of the new root as arguments, so they
//! can be run against any directory.

use std::fmt;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, symlink};
use std::path::{Path, PathBuf};

use rustix::mount::{MountFlags, mount, mount_bind, mount_move, mount_remount};

/// The kernel filesystems that the initramfs mounts and hands to the new
/// root, in the order they are moved.
pub(crate) const KERNEL_FILESYSTEMS: [&str; 4] = ["dev", "proc", "sys", "run"];

/// One change to the mount table or to the runtime files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    /// Mounts a new filesystem of type `fstype` from `source` at `target`.
    Mount {
        /// The device or a name for a virtual filesystem.
        source: String,
        /// The mount point.
        target: PathBuf,
        /// The filesystem type.
        fstype: String,
        /// The generic mount flags.
        flags: MountFlags,
        /// The filesystem specific options.
        data: String,
    },
    /// Creates a directory and its missing parents with mode 0755.
    MkdirAll(PathBuf),
    /// Bind mounts `source` at `target`.
    Bind {
        /// The tree to show.
        source: PathBuf,
        /// The mount point.
        target: PathBuf,
    },
    /// Changes the flags of the bind mount at `target` to exactly `flags`.
    RemountBind {
        /// The mount point.
        target: PathBuf,
        /// The flags the mount ends up with.
        flags: MountFlags,
    },
    /// Moves the mount at `source` to `target`.
    Move {
        /// The current mount point.
        source: PathBuf,
        /// The new mount point.
        target: PathBuf,
    },
    /// Creates the symlink `link` pointing at `target`, replacing a symlink
    /// left by an earlier attempt.
    Symlink {
        /// The target, stored as is.
        target: String,
        /// The path of the link.
        link: PathBuf,
    },
    /// Writes `contents` to the file `path`.
    Write {
        /// The file.
        path: PathBuf,
        /// Its new content.
        contents: String,
    },
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mount {
                source,
                target,
                fstype,
                ..
            } => write!(f, "mount {source} ({fstype}) at {}", target.display()),
            Self::MkdirAll(path) => write!(f, "create {}", path.display()),
            Self::Bind { source, target } => {
                write!(f, "bind {} at {}", source.display(), target.display())
            }
            Self::RemountBind { target, .. } => write!(f, "remount {}", target.display()),
            Self::Move { source, target } => {
                write!(f, "move {} to {}", source.display(), target.display())
            }
            Self::Symlink { target, link } => {
                write!(f, "link {} to {target}", link.display())
            }
            Self::Write { path, .. } => write!(f, "write {}", path.display()),
        }
    }
}

fn kernel_fs(root: &Path, name: &str, fstype: &str, flags: MountFlags, data: &str) -> Op {
    Op::Mount {
        source: fstype.to_owned(),
        target: root.join(name),
        fstype: fstype.to_owned(),
        flags,
        data: data.to_owned(),
    }
}

/// Mounts `/proc`, `/sys`, `/dev` and `/run` below the initramfs `root` and
/// creates `/run/laft`.
pub(crate) fn kernel_filesystems(root: &Path) -> Vec<Op> {
    let hardened = MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC;
    vec![
        kernel_fs(root, "proc", "proc", hardened, ""),
        kernel_fs(root, "sys", "sysfs", hardened, ""),
        kernel_fs(root, "dev", "devtmpfs", MountFlags::NOSUID, "mode=0755"),
        kernel_fs(
            root,
            "run",
            "tmpfs",
            MountFlags::NOSUID | MountFlags::NODEV,
            "mode=0755",
        ),
        Op::MkdirAll(root.join("run/laft")),
    ]
}

/// Mounts generation `generation` of the new root `newroot` for the boot,
/// names it as the system and booted generation in `/run/laft` below the
/// initramfs `root`, records its init `backend`, and then makes `/laft` of
/// the new root read-only.
///
/// The generation mount is the only mount that honors setuid bits, so its
/// remount leaves out `nosuid`. It is cloned from `/laft` before `/laft`
/// becomes `nosuid`.
pub(crate) fn activate(root: &Path, newroot: &Path, generation: u64, backend: &str) -> Vec<Op> {
    let run = root.join("run/laft");
    let mount = run.join("mounts").join(generation.to_string());
    let laft = newroot.join("laft");
    let target = format!("mounts/{generation}");
    vec![
        Op::MkdirAll(mount.clone()),
        Op::Bind {
            source: laft.join("gens").join(generation.to_string()),
            target: mount.clone(),
        },
        Op::RemountBind {
            target: mount,
            flags: MountFlags::RDONLY | MountFlags::NODEV,
        },
        Op::Symlink {
            target: target.clone(),
            link: run.join("system"),
        },
        Op::Symlink {
            target,
            link: run.join("booted"),
        },
        Op::Write {
            path: run.join("init-backend"),
            contents: format!("{backend}\n"),
        },
        Op::Bind {
            source: laft.clone(),
            target: laft.clone(),
        },
        Op::RemountBind {
            target: laft,
            flags: MountFlags::RDONLY | MountFlags::NOSUID | MountFlags::NODEV,
        },
    ]
}

/// Moves the kernel filesystems of the initramfs `root` into the new root
/// `newroot`, creating their mount points there when missing. Moving `/run`
/// takes the generation mount below it along.
pub(crate) fn move_kernel_filesystems(root: &Path, newroot: &Path) -> Vec<Op> {
    KERNEL_FILESYSTEMS
        .iter()
        .flat_map(|name| {
            [
                Op::MkdirAll(newroot.join(name)),
                Op::Move {
                    source: root.join(name),
                    target: newroot.join(name),
                },
            ]
        })
        .collect()
}

/// Carries out `ops` in order and stops at the first that fails.
///
/// # Errors
///
/// Fails with the operation and the reason when an operation fails.
pub(crate) fn execute(ops: &[Op]) -> Result<(), String> {
    for op in ops {
        apply(op).map_err(|e| format!("cannot {op}: {e}"))?;
    }
    Ok(())
}

fn apply(op: &Op) -> io::Result<()> {
    match op {
        Op::Mount {
            source,
            target,
            fstype,
            flags,
            data,
        } => mount(
            source.as_str(),
            target,
            fstype.as_str(),
            *flags,
            std::ffi::CString::new(data.as_str())?.as_c_str(),
        )?,
        Op::MkdirAll(path) => DirBuilder::new().recursive(true).mode(0o755).create(path)?,
        Op::Bind { source, target } => mount_bind(source, target)?,
        Op::RemountBind { target, flags } => {
            mount_remount(target, *flags | MountFlags::BIND, c"")?;
        }
        Op::Move { source, target } => mount_move(source, target)?,
        Op::Symlink { target, link } => {
            if fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink()) {
                fs::remove_file(link)?;
            }
            symlink(target, link)?;
        }
        Op::Write { path, contents } => fs::write(path, contents)?,
    }
    Ok(())
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
