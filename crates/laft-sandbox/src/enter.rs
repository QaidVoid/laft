use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{FileType, Mode, OFlags, ResolveFlags};
use rustix::io::Errno;
use rustix::mount::{MountAttrFlags, MountPropagationFlags, MoveMountFlags, OpenTreeFlags};
use rustix::thread::UnshareFlags;

use crate::error::{Error, Step};
use crate::sys;

const CLONE: OpenTreeFlags = OpenTreeFlags::OPEN_TREE_CLONE
    .union(OpenTreeFlags::OPEN_TREE_CLOEXEC)
    .union(OpenTreeFlags::AT_EMPTY_PATH);

const BENEATH: ResolveFlags = ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS);

const BOTH_EMPTY: MoveMountFlags =
    MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH.union(MoveMountFlags::MOVE_MOUNT_T_EMPTY_PATH);

/// Moves the calling process into a private mount namespace in which the
/// `usr` directory of the system generation `generation` shows the view tree
/// `view`, with the generation's `privileged` files bound on top.
///
/// `generation` and `view` are absolute paths that contain no symlinks, for
/// example `/run/laft/mounts/42` and `/laft/views/dev/7/usr`. They are opened
/// only inside the new namespace, because a descriptor opened before it
/// refers to the old namespace's mounts.
///
/// This is the mount sequence of the setuid helper that enters a view. The
/// caller must be real root and single-threaded, and it drops privileges and
/// executes the command itself afterwards. No user namespace is created, so
/// root keeps its privileges in the new namespace and setuid programs keep
/// working there. The steps are:
///
/// 1. Check every privileged path before touching anything. Each one is
///    absolute and lies below `/usr`, like `/usr/bin/doas`, with no `..`
///    component.
/// 2. `unshare(CLONE_NEWNS)`. Every mount below lives only in the new
///    namespace and disappears with its last process, so a failure at any
///    later step leaves nothing behind on the host.
/// 3. Make every mount a slave, recursively from `/`. Mounts the host makes
///    later, such as a USB drive, still appear inside, and nothing mounted
///    inside propagates back to the host.
/// 4. Open `generation` and `view`, refusing symlinks on the whole path.
/// 5. Clone each privileged file of the generation as a detached mount,
///    read-only and `nodev` with setuid allowed. This happens before step 7
///    covers the generation's `usr`, which is where the files live.
/// 6. Clone `view` as a detached mount, read-only, `nosuid` and `nodev`, so
///    that no setuid file or device node in a view grants anything.
/// 7. Attach the view clone over the generation's `usr`. Only `usr` is
///    covered, so the rest of the generation, such as its rendered
///    configuration, stays the system's.
/// 8. Attach each privileged clone over the same path, which now resolves
///    inside the view. A path the view lacks is skipped, because a view only
///    gains the privileged programs it already contains.
///
/// Every lookup of a privileged path is relative to `generation`, stays
/// beneath it and refuses symlinks, and every mount is made through a
/// descriptor, so a symlink planted in either tree cannot redirect a mount.
/// Privileged sources and targets must be regular files.
///
/// Requires Linux 5.12 or newer for `mount_setattr`.
///
/// # Errors
///
/// Fails when a privileged path is malformed, missing from the generation or
/// names something other than a regular file, when `generation` or `view` is
/// not a directory reached without symlinks, when the caller lacks
/// `CAP_SYS_ADMIN` or has more than one thread, and when the kernel lacks the
/// mount API (`ENOSYS`). A failure
/// after step 2 leaves the process in the new namespace with some of the
/// mounts made, so the caller must exit rather than continue.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// let generation = Path::new("/run/laft/mounts/42");
/// let view = Path::new("/laft/views/dev/7/usr");
/// let privileged = [Path::new("/usr/bin/doas")];
/// laft_sandbox::enter_view(generation, view, &privileged)?;
/// # Ok::<(), laft_sandbox::Error>(())
/// ```
pub fn enter_view(generation: &Path, view: &Path, privileged: &[&Path]) -> Result<(), Error> {
    let targets = privileged
        .iter()
        .map(|path| below_usr(path).ok_or_else(|| Error::at(Step::InvalidPath, path, Errno::INVAL)))
        .collect::<Result<Vec<_>, _>>()?;

    sys::unshare(UnshareFlags::NEWNS).map_err(|e| Error::new(Step::Unshare, e))?;
    rustix::mount::mount_change(
        c"/",
        MountPropagationFlags::DOWNSTREAM | MountPropagationFlags::REC,
    )
    .map_err(|e| Error::new(Step::Propagation, e))?;
    let generation = open_root(generation)?;
    let generation = generation.as_fd();
    let view = open_root(view)?;

    let mut sources = Vec::with_capacity(targets.len());
    for (path, target) in privileged.iter().zip(&targets) {
        let at = |step| move |errno| Error::at(step, path, errno);
        let file = open_beneath(generation, target).map_err(at(Step::CloneTree))?;
        regular_file(&file, path)?;
        let source = rustix::mount::open_tree(&file, c"", CLONE).map_err(at(Step::CloneTree))?;
        sys::mount_setattr(
            source.as_fd(),
            MountAttrFlags::MOUNT_ATTR_RDONLY | MountAttrFlags::MOUNT_ATTR_NODEV,
            MountAttrFlags::MOUNT_ATTR_NOSUID,
        )
        .map_err(at(Step::SetAttributes))?;
        sources.push(source);
    }

    let usr =
        rustix::mount::open_tree(&view, c"", CLONE).map_err(|e| Error::new(Step::CloneTree, e))?;
    sys::mount_setattr(
        usr.as_fd(),
        MountAttrFlags::MOUNT_ATTR_RDONLY
            | MountAttrFlags::MOUNT_ATTR_NOSUID
            | MountAttrFlags::MOUNT_ATTR_NODEV,
        MountAttrFlags::empty(),
    )
    .map_err(|e| Error::new(Step::SetAttributes, e))?;
    let usr_target = rustix::fs::openat2(
        generation,
        c"usr",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        BENEATH,
    )
    .map_err(|e| Error::at(Step::OpenTarget, Path::new("/usr"), e))?;
    rustix::mount::move_mount(&usr, c"", &usr_target, c"", BOTH_EMPTY)
        .map_err(|e| Error::at(Step::Attach, Path::new("/usr"), e))?;

    for ((path, target), source) in privileged.iter().zip(&targets).zip(&sources) {
        let file = match open_beneath(generation, target) {
            Ok(file) => file,
            Err(Errno::NOENT) => continue,
            Err(errno) => return Err(Error::at(Step::OpenTarget, path, errno)),
        };
        regular_file(&file, path)?;
        rustix::mount::move_mount(source, c"", &file, c"", BOTH_EMPTY)
            .map_err(|e| Error::at(Step::Attach, path, e))?;
    }
    Ok(())
}

/// `path` relative to the generation, when it is an absolute path strictly
/// below `/usr` made of plain components.
fn below_usr(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir)
        || components.next() != Some(Component::Normal("usr".as_ref()))
    {
        return None;
    }
    let rest = components.as_path();
    let plain = components.all(|component| matches!(component, Component::Normal(_)));
    (plain && !rest.as_os_str().is_empty()).then(|| Path::new("usr").join(rest))
}

/// Opens a directory given by an absolute path without symlinks. This
/// happens after `unshare`, because a descriptor opened before it refers to
/// a mount of the old namespace, which the mount calls refuse.
fn open_root(path: &Path) -> Result<OwnedFd, Error> {
    rustix::fs::openat2(
        rustix::fs::CWD,
        path,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map_err(|e| Error::at(Step::OpenTarget, path, e))
}

fn open_beneath(generation: BorrowedFd<'_>, path: &Path) -> Result<OwnedFd, Errno> {
    rustix::fs::openat2(
        generation,
        path,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        BENEATH,
    )
}

fn regular_file(file: &OwnedFd, path: &Path) -> Result<(), Error> {
    let stat = rustix::fs::fstat(file).map_err(|e| Error::at(Step::NotRegularFile, path, e))?;
    if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile {
        Ok(())
    } else {
        Err(Error::at(Step::NotRegularFile, path, Errno::INVAL))
    }
}

#[cfg(test)]
#[path = "enter_tests.rs"]
mod tests;
