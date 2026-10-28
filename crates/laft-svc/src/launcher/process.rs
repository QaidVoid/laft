//! Setting up the process of a service: directories, limits, environment
//! and credentials.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};

use rustix::fs::{Mode, OFlags};
use rustix::io::{Errno, FdFlags};
use rustix::process::{Gid, Resource, Rlimit, Uid};

use super::accounts::Credentials;
use crate::error::{Error, io};
use crate::spec::Spec;

/// Creates the directory `relative` below `base` below `root`, then gives
/// it exactly `uid`, `gid` and `mode`, also when it exists already.
///
/// `base`, such as `/var/lib`, is created when missing and may be reached
/// through symlinks. Below it, every component is opened without following
/// symlinks, so that an account owning one directory cannot redirect the
/// ownership change to a file elsewhere. Missing parents below `base` are
/// created with mode `0o755` and owned by the launcher.
pub(crate) fn ensure_dir(
    root: &Path,
    base: &str,
    relative: &str,
    uid: u32,
    gid: u32,
    mode: u32,
) -> Result<(), Error> {
    let base_path = root.join(base.trim_start_matches('/'));
    fs::create_dir_all(&base_path).map_err(io(&base_path))?;
    let mut dir: OwnedFd = File::open(&base_path).map_err(io(&base_path))?.into();
    let mut path = base_path;
    let components: Vec<&str> = relative.split('/').collect();
    for (i, component) in components.iter().enumerate() {
        path.push(component);
        let created = match rustix::fs::mkdirat(&dir, *component, Mode::from_raw_mode(0o755)) {
            Ok(()) => true,
            Err(Errno::EXIST) => false,
            Err(e) => return Err(io(&path)(e.into())),
        };
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        dir = rustix::fs::openat(&dir, *component, flags, Mode::empty())
            .map_err(|e| io(&path)(e.into()))?;
        let last = i + 1 == components.len();
        if last {
            rustix::fs::fchown(&dir, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))
                .map_err(|e| io(&path)(e.into()))?;
            rustix::fs::fchmod(&dir, Mode::from_raw_mode(mode)).map_err(|e| io(&path)(e.into()))?;
        } else if created {
            rustix::fs::fchmod(&dir, Mode::from_raw_mode(0o755))
                .map_err(|e| io(&path)(e.into()))?;
        }
    }
    Ok(())
}

/// The limits a process had before [`apply_limits`] changed them.
pub(crate) type SavedLimits = Vec<(Resource, Rlimit)>;

/// Applies the service's resource limits to the current process, each as
/// soft and hard limit, and returns the limits they replaced.
pub(crate) fn apply_limits(spec: &Spec) -> Result<SavedLimits, Error> {
    let limits = &spec.limits;
    let mut saved = Vec::new();
    for (resource, value) in [
        (Resource::Nofile, limits.nofile),
        (Resource::Nproc, limits.nproc),
        (Resource::Core, limits.core),
        (Resource::Memlock, limits.memlock),
    ] {
        if let Some(value) = value {
            let limit = Rlimit {
                current: Some(value),
                maximum: Some(value),
            };
            let previous = rustix::process::getrlimit(resource);
            rustix::process::setrlimit(resource, limit).map_err(|e| Error::Setup {
                name: spec.name.clone(),
                step: "set a resource limit",
                source: e.into(),
            })?;
            saved.push((resource, previous));
        }
    }
    Ok(saved)
}

/// Puts back the limits that [`apply_limits`] replaced. Raising a hard
/// limit again needs `CAP_SYS_RESOURCE`, which the launcher has as root.
pub(crate) fn restore_limits(name: &str, saved: SavedLimits) -> Result<(), Error> {
    for (resource, limit) in saved.into_iter().rev() {
        rustix::process::setrlimit(resource, limit).map_err(|e| Error::Setup {
            name: name.to_owned(),
            step: "restore a resource limit",
            source: e.into(),
        })?;
    }
    Ok(())
}

/// The environment of a service, built from scratch: `PATH`, the account's
/// `HOME`, `USER`, `LOGNAME` and `SHELL`, then the service's own variables,
/// which may override them.
pub(crate) fn environment(spec: &Spec, credentials: &Credentials) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([
        ("PATH".to_owned(), "/usr/bin".to_owned()),
        ("HOME".to_owned(), credentials.home.clone()),
        ("USER".to_owned(), credentials.user.clone()),
        ("LOGNAME".to_owned(), credentials.user.clone()),
        ("SHELL".to_owned(), credentials.shell.clone()),
    ]);
    env.extend(spec.env.clone());
    env
}

/// The command that runs `argv` for the service, with its environment and
/// working directory.
pub(crate) fn command(spec: &Spec, credentials: &Credentials, argv: &[String]) -> Command {
    let mut command = Command::new(&argv[0]);
    command
        .arg0(&argv[0])
        .args(&argv[1..])
        .env_clear()
        .envs(environment(spec, credentials))
        .current_dir(&spec.cwd);
    command
}

/// Switches the calling thread to the service's credentials: supplementary
/// groups, then group, then user, and checks the result. A service that
/// runs as root keeps uid 0, and its groups change only when it declares
/// a group or supplementary groups.
///
/// The system calls change only the calling thread, unlike their C library
/// counterparts. A process started from this thread inherits the thread's
/// credentials, and a successful `execve` from it gives them to the whole
/// new program.
///
/// The launcher calls this on its only thread before it executes a service
/// or its logger. It starts helpers as separate processes, and the one
/// thread it does start, in [`spawn_as`], is joined before that function
/// returns. Even with other threads alive, `execve` ends them and runs the
/// new program with the calling thread's credentials only.
pub(crate) fn drop_privileges(name: &str, credentials: &Credentials) -> Result<(), Error> {
    if !changes_credentials(credentials) {
        return Ok(());
    }
    let setup = |step: &'static str| {
        move |e: Errno| Error::Setup {
            name: name.to_owned(),
            step,
            source: e.into(),
        }
    };
    let groups: Vec<Gid> = credentials
        .groups
        .iter()
        .map(|&g| Gid::from_raw(g))
        .collect();
    let (uid, gid) = (
        Uid::from_raw(credentials.uid),
        Gid::from_raw(credentials.gid),
    );
    rustix::thread::set_thread_groups(&groups).map_err(setup("set the groups"))?;
    rustix::thread::set_thread_res_gid(gid, gid, gid).map_err(setup("set the group"))?;
    if !uid.is_root() {
        rustix::thread::set_thread_res_uid(uid, uid, uid).map_err(setup("set the user"))?;
    }
    let dropped = rustix::process::getuid() == uid
        && rustix::process::geteuid() == uid
        && rustix::process::getgid() == gid
        && rustix::process::getegid() == gid;
    if !dropped {
        return Err(setup("verify the dropped credentials")(Errno::PERM));
    }
    Ok(())
}

/// Whether running as `credentials` changes anything for a launcher that
/// runs as root.
fn changes_credentials(credentials: &Credentials) -> bool {
    credentials.uid != 0 || credentials.declares_groups
}

/// Starts `command` with the service's credentials, leaving the launcher's
/// own credentials unchanged.
///
/// The credentials are dropped on a short-lived thread that starts the
/// process and ends, so the launcher keeps the privileges it needs to
/// record the outcome.
pub(crate) fn spawn_as(
    name: &str,
    credentials: &Credentials,
    mut command: Command,
) -> Result<Child, Error> {
    let program = command.get_program().to_string_lossy().into_owned();
    let exec_error = |source| Error::Exec {
        name: name.to_owned(),
        program: program.clone(),
        source,
    };
    if !changes_credentials(credentials) {
        return command.spawn().map_err(exec_error);
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                drop_privileges(name, credentials)?;
                command.spawn().map_err(exec_error)
            })
            .join()
            .unwrap_or_else(|_| {
                Err(exec_error(io::Error::other(
                    "the thread that drops privileges panicked",
                )))
            })
    })
}

/// Moves `fd` to descriptor number `target` and lets it survive `execve`.
///
/// Fails when `target` is in use already.
pub(crate) fn place_fd(name: &str, fd: &OwnedFd, target: u32) -> Result<OwnedFd, Error> {
    let setup = |source| Error::Setup {
        name: name.to_owned(),
        step: "place the readiness descriptor",
        source,
    };
    let target =
        i32::try_from(target).map_err(|_| setup(io::Error::other("invalid descriptor")))?;
    let placed =
        rustix::io::fcntl_dupfd_cloexec(fd, target).map_err(|e| setup(io::Error::from(e)))?;
    if placed.as_raw_fd() != target {
        return Err(setup(io::Error::other(format!(
            "descriptor {target} is in use"
        ))));
    }
    rustix::io::fcntl_setfd(&placed, FdFlags::empty()).map_err(|e| setup(e.into()))?;
    Ok(placed)
}

/// Replaces `path` with `bytes` so that a crash leaves the old or the new
/// content: a temporary file in the same directory is written, synced and
/// renamed into place, then the directory is synced.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), Error> {
    let dir = path.parent().unwrap_or(path);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{}.tmp", name.unwrap_or_default()));
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .map_err(io(&tmp))?;
    file.write_all(bytes).map_err(io(&tmp))?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(io(&tmp))?;
    file.sync_all().map_err(io(&tmp))?;
    fs::rename(&tmp, path).map_err(io(path))?;
    File::open(dir).and_then(|d| d.sync_all()).map_err(io(dir))
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
