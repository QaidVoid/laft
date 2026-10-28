//! The privileged phase: find the active generation and the view version,
//! authorize the caller, lock the view version and mount it.
//!
//! Every lookup refuses symlinks, and every lookup below a directory that
//! was already opened stays beneath it. Record files are trusted only after
//! the checks of [`records::check_file`] on the opened descriptor.

use std::fs::File;
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::{
    AtFlags, FileType, FlockOperation, Mode, OFlags, ResolveFlags, StatxAttributes, StatxFlags,
};
use rustix::io::{Errno, FdFlags};

use crate::Failure;
use crate::access::{self, Caller};
use crate::records::{self, FileFacts, MAX_RECORD};

/// Where the active generation is named and mounted.
const RUN: &str = "/run/laft";

/// Where views live.
const VIEWS: &str = "/laft/views";

const BENEATH: ResolveFlags = ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS);

/// Authorizes `caller` for `view` and moves the process into a private
/// mount namespace in which `/usr` shows the view.
///
/// Returns the descriptor of the view version's `view.json`. It holds a
/// shared lock that keeps garbage collection away from the view version,
/// and it is not close-on-exec, so the lock lasts as long as the command
/// and anything that inherits it.
///
/// # Errors
///
/// Fails when a file is missing or untrusted, when a record does not match,
/// when the caller may not enter the view and when mounting fails. A failure
/// after the mount namespace was created leaves partial mounts in it, so the
/// process must exit.
pub(crate) fn enter(view: &str, caller: &Caller) -> Result<OwnedFd, Failure> {
    let run = open_dir(rustix::fs::CWD, RUN, ResolveFlags::NO_SYMLINKS)?;
    let target = rustix::fs::readlinkat(&run, "system", Vec::new())
        .map_err(failed(format!("reading {RUN}/system")))?;
    let generation = records::generation_link(target.as_bytes())
        .ok_or_else(|| Failure::error(format!("{RUN}/system does not name a generation")))?;
    let generation_dir = format!("{RUN}/mounts/{generation}");
    let generation_fd = open_dir(&run, &format!("mounts/{generation}"), BENEATH)?;
    check_mount_root(&generation_fd, &generation_dir)?;

    let views_dir = format!("{VIEWS}/{view}");
    let views = match open_dir(rustix::fs::CWD, &views_dir, ResolveFlags::NO_SYMLINKS) {
        Err(failure) if failure.errno == Some(Errno::NOENT) => {
            return Err(Failure::error(format!("no view {view:?}")));
        }
        result => result?,
    };
    let target = rustix::fs::readlinkat(&views, "current", Vec::new())
        .map_err(failed(format!("reading {views_dir}/current")))?;
    let version = records::version_link(target.as_bytes()).ok_or_else(|| {
        Failure::error(format!("{views_dir}/current does not name a view version"))
    })?;
    let version_fd = open_dir(&views, &version.to_string(), BENEATH)?;

    let view_path = format!("{views_dir}/{version}/view.json");
    let (lock, bytes) = read_record(&version_fd, "view.json", &view_path)?;
    let record = records::parse_view(&bytes, view, version)
        .map_err(|e| Failure::error(format!("{view_path}: {e}")))?;
    if !access::may_enter(caller, &record) {
        return Err(Failure::error(format!(
            "uid {} may not enter view {view:?}",
            caller.uid
        )));
    }

    let privileged_path = format!("{generation_dir}/privileged.json");
    let (_, bytes) = read_record(&generation_fd, "privileged.json", &privileged_path)?;
    let privileged = records::parse_privileged(&bytes)
        .map_err(|e| Failure::error(format!("{privileged_path}: {e}")))?;
    let paths: Vec<&Path> = match records::privileged_paths(&privileged, view, version) {
        Some(paths) => paths.iter().map(Path::new).collect(),
        None => {
            crate::note(&format!(
                "privileged programs are unavailable in view {view:?} until the next laft apply"
            ));
            Vec::new()
        }
    };

    rustix::fs::flock(&lock, FlockOperation::LockShared)
        .map_err(failed(format!("locking {view_path}")))?;
    rustix::io::fcntl_setfd(&lock, FdFlags::empty())
        .map_err(failed(format!("keeping {view_path} open")))?;
    drop((run, generation_fd, views, version_fd));

    let usr = PathBuf::from(format!("{views_dir}/{version}/usr"));
    laft_sandbox::enter_view(Path::new(&generation_dir), &usr, &paths).map_err(|e| {
        let errno = e.io_error().and_then(Errno::from_io_error);
        let hint = if errno == Some(Errno::NOSYS) {
            " (the kernel lacks the descriptor based mount API)"
        } else {
            ""
        };
        Failure::error(format!("entering view {view:?}: {e}{hint}"))
    })?;
    Ok(lock)
}

/// Maps a system call error about `what` to a failure.
fn failed(what: String) -> impl FnOnce(Errno) -> Failure {
    move |errno| Failure::os(format!("{what}: {}", std::io::Error::from(errno)), errno)
}

/// Opens the directory `path` relative to `dir` with `resolve`.
fn open_dir(dir: impl AsFd, path: &str, resolve: ResolveFlags) -> Result<OwnedFd, Failure> {
    rustix::fs::openat2(
        dir,
        path,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        resolve,
    )
    .map_err(failed(format!("opening {path}")))
}

/// Checks that `dir`, shown as `shown`, is the root of a mount, as a
/// mounted generation is.
fn check_mount_root(dir: &OwnedFd, shown: &str) -> Result<(), Failure> {
    let stat = rustix::fs::statx(dir, "", AtFlags::EMPTY_PATH, StatxFlags::BASIC_STATS)
        .map_err(failed(format!("inspecting {shown}")))?;
    if !stat
        .stx_attributes_mask
        .contains(StatxAttributes::MOUNT_ROOT)
    {
        return Err(Failure::error("the kernel does not report mount roots"));
    }
    if !stat.stx_attributes.contains(StatxAttributes::MOUNT_ROOT) {
        return Err(Failure::error(format!(
            "{shown} is not a mounted generation"
        )));
    }
    Ok(())
}

/// Opens the record file `name` beneath `dir`, shown as `shown`, checks it
/// and reads it. Returns its descriptor and its contents.
fn read_record(dir: &OwnedFd, name: &str, shown: &str) -> Result<(OwnedFd, Vec<u8>), Failure> {
    // `O_NONBLOCK` keeps a FIFO in place of the record from blocking the
    // open. The type check below then refuses it.
    let fd = rustix::fs::openat2(
        dir,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOCTTY | OFlags::NONBLOCK,
        Mode::empty(),
        BENEATH,
    )
    .map_err(failed(format!("opening {shown}")))?;
    let stat = rustix::fs::fstat(&fd).map_err(failed(format!("inspecting {shown}")))?;
    let facts = FileFacts {
        regular: FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile,
        uid: stat.st_uid,
        mode: stat.st_mode & 0o7777,
        size: u64::try_from(stat.st_size).unwrap_or(u64::MAX),
    };
    records::check_file(facts, MAX_RECORD)
        .map_err(|reason| Failure::error(format!("{shown}: {reason}")))?;

    let mut bytes = Vec::new();
    let mut file = File::from(fd);
    file.by_ref()
        .take(MAX_RECORD + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Failure::error(format!("reading {shown}: {e}")))?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(Failure::error(format!("{shown}: too large")));
    }
    Ok((file.into(), bytes))
}
