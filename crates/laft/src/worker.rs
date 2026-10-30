//! The split of a command that writes on a running system into two
//! processes.
//!
//! `/laft` is mounted read-only in the initial mount namespace, but
//! activation must mount generations there. So the `laft` process the user
//! started becomes the coordinator: it stays in the initial namespace and
//! starts the store worker, `laft internal worker -- <the same arguments>`.
//! The worker moves into a private mount namespace, makes `/laft` writable
//! there, and runs the command. Whenever it needs a mount in the initial
//! namespace, it asks the coordinator over a Unix socket. The coordinator
//! also reconciles the services for it, because the supervisor and the
//! programs that reconciliation starts belong to the initial namespace,
//! and only there does the new generation's mount appear. Requests and
//! responses are JSON documents, each preceded by its length as a
//! little-endian `u32`.
//!
//! The socket is created by the coordinator in `/run/laft`, readable only
//! by root, and both ends check the process on the other side: the
//! coordinator accepts only its child, and the worker refuses to run
//! unless its peer is its parent.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rustix::mount::{MountFlags, UnmountFlags};
use serde::{Deserialize, Serialize};

use laft_activate::Services;

use crate::cli::{BootCommand, Command, EtcCommand, GensCommand, RepoCommand, ViewCommand};
use crate::env::Env;
use crate::failure::{Failure, exit};

/// The environment variable that tells the worker where the coordinator's
/// socket is.
const SOCKET_ENV: &str = "LAFT_WORKER_SOCKET";

/// The largest message either side accepts.
const MAX_MESSAGE: u32 = 1 << 20;

/// The worker's connection to its coordinator, set once at startup.
static CHANNEL: OnceLock<Mutex<UnixStream>> = OnceLock::new();

/// What the worker asks the coordinator to do in the initial namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Request {
    /// Mount generation `generation` at `/run/laft/mounts/<generation>`,
    /// read-only and `nodev` but with setuid honored.
    Mount {
        /// The generation.
        generation: u64,
    },
    /// Detach every generation mount except those of `keep`.
    Unmount {
        /// The generations to keep mounted.
        keep: Vec<u64>,
    },
    /// Bring the running services in line with generation `generation`
    /// after a switch from `previous`.
    Services {
        /// The generation the system switched from.
        previous: Option<u64>,
        /// The generation the system switched to.
        generation: u64,
    },
}

/// The coordinator's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Response {
    /// Why the request failed, or `None` when it succeeded.
    pub(crate) error: Option<String>,
    /// What reconciling the services did, for a services request.
    #[serde(default)]
    pub(crate) services: Option<Services>,
}

/// Whether `command` writes to the system and so must run in the worker
/// when `env` is a running system whose `/laft` is read-only.
pub(crate) fn needed(env: &Env, command: &Command) -> bool {
    let writes = match command {
        Command::Plan(_) | Command::Apply(_) | Command::Upgrade { .. } => true,
        Command::Switch { .. } | Command::Rollback { .. } | Command::Activate { .. } => true,
        Command::Gc(_) | Command::Install { .. } => true,
        Command::Gens(GensCommand::Rm { .. }) => true,
        Command::Boot(BootCommand::SetDefault { .. } | BootCommand::MarkGood) => true,
        Command::Boot(BootCommand::Install) => true,
        Command::Etc(EtcCommand::Reset { .. }) => true,
        Command::Repo(RepoCommand::Sync { .. } | RepoCommand::Reset { .. }) => true,
        Command::View(ViewCommand::Build { .. }) => true,
        Command::Check { refresh, .. } => *refresh,
        _ => false,
    };
    writes
        && !env.offline_root
        && rustix::process::geteuid().is_root()
        && read_only(&env.path("/laft"))
}

/// Whether `path` is on a read-only mount.
fn read_only(path: &Path) -> bool {
    rustix::fs::statvfs(path)
        .is_ok_and(|s| s.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY))
}

fn framed_write(stream: &mut UnixStream, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let len = u32::try_from(bytes.len()).map_err(io::Error::other)?;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&bytes)
}

/// Reads one message, or `None` at the end of the stream.
fn framed_read<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match stream.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE {
        return Err(io::Error::other("the message is too large"));
    }
    let mut bytes = vec![0u8; len as usize];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(io::Error::other)
}

/// The process on the other end of `stream`.
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    rustix::net::sockopt::socket_peercred(stream)
        .ok()
        .and_then(|cred| u32::try_from(cred.pid.as_raw_nonzero().get()).ok())
}

/// Runs the command as the coordinator: starts the worker with the same
/// arguments, serves its requests and returns its exit status.
pub(crate) fn coordinate(env: &Env) -> Result<u8, Failure> {
    let dir = env.path("/run/laft");
    let path = dir.join(format!("worker.{}.sock", std::process::id()));
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .map_err(|e| Failure::general(format!("{}: {e}", path.display())))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let spawned = Process::new("/proc/self/exe")
        .args(["internal", "worker", "--"])
        .args(&args)
        .env(SOCKET_ENV, &path)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            let _ = fs::remove_file(&path);
            return Err(Failure::general(format!("starting the store worker: {e}")));
        }
    };
    let accepted = accept(&listener, &mut child);
    let _ = fs::remove_file(&path);
    if let Some(mut stream) = accepted {
        while let Ok(Some(request)) = framed_read::<Request>(&mut stream) {
            let response = match serve(env, &request) {
                Ok(services) => Response {
                    error: None,
                    services,
                },
                Err(f) => Response {
                    error: Some(f.message),
                    services: None,
                },
            };
            if framed_write(&mut stream, &response).is_err() {
                break;
            }
        }
    }
    let status = child.wait()?;
    Ok(match status.code() {
        Some(code) => u8::try_from(code).unwrap_or(exit::FAILURE),
        None => 130,
    })
}

/// Accepts the connection of `child`, or `None` once it exited without
/// connecting.
fn accept(listener: &UnixListener, child: &mut Child) -> Option<UnixStream> {
    listener.set_nonblocking(true).ok()?;
    loop {
        match listener.accept() {
            Ok((stream, _)) if peer_pid(&stream) == Some(child.id()) => {
                stream.set_nonblocking(false).ok()?;
                return Some(stream);
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if child.try_wait().ok().flatten().is_some() {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// Carries out `request` in this process's mount namespace, and returns
/// what reconciling the services did for a services request.
pub(crate) fn serve(env: &Env, request: &Request) -> Result<Option<Services>, Failure> {
    match request {
        Request::Mount { generation } => mount(env, *generation).map(|()| None),
        Request::Unmount { keep } => unmount(env, keep).map(|()| None),
        Request::Services {
            previous,
            generation,
        } => laft_activate::reconcile_services(&env.root, *previous, *generation)
            .map(Some)
            .map_err(Failure::general),
    }
}

/// The mount points of this mount namespace.
fn mount_points() -> Vec<PathBuf> {
    fs::read_to_string("/proc/self/mountinfo")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split(' ').nth(4))
        .map(|p| PathBuf::from(p.replace("\\040", " ")))
        .collect()
}

fn mount(env: &Env, generation: u64) -> Result<(), Failure> {
    let target = env.path(&format!("/run/laft/mounts/{generation}"));
    if mount_points().contains(&target) {
        return Ok(());
    }
    fs::create_dir_all(&target)?;
    let source = env.path(&format!("/laft/gens/{generation}"));
    let fail =
        |e: rustix::io::Errno| Failure::general(format!("mounting generation {generation}: {e}"));
    rustix::mount::mount_bind(&source, &target).map_err(fail)?;
    rustix::mount::mount_remount(
        &target,
        MountFlags::BIND | MountFlags::RDONLY | MountFlags::NODEV,
        "",
    )
    .map_err(fail)
}

fn unmount(env: &Env, keep: &[u64]) -> Result<(), Failure> {
    let dir = env.path("/run/laft/mounts");
    let mut failed = Vec::new();
    for item in fs::read_dir(&dir)? {
        let item = item?;
        let keep = item
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u64>().ok())
            .is_some_and(|n| keep.contains(&n));
        if keep {
            continue;
        }
        let path = item.path();
        if mount_points().contains(&path)
            && let Err(e) = rustix::mount::unmount(&path, UnmountFlags::DETACH)
        {
            failed.push(format!("{}: {e}", path.display()));
            continue;
        }
        if let Err(e) = fs::remove_dir(&path) {
            failed.push(format!("{}: {e}", path.display()));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Failure::general(failed.join(", ")))
    }
}

/// Sends `request` to the coordinator when this process is the worker, and
/// otherwise carries it out here, in the initial namespace. Returns what
/// reconciling the services did for a services request.
pub(crate) fn request(env: &Env, request: &Request) -> Result<Option<Services>, Failure> {
    let Some(channel) = CHANNEL.get() else {
        return serve(env, request);
    };
    let mut stream = channel
        .lock()
        .map_err(|_| Failure::general("the coordinator connection is unusable"))?;
    framed_write(&mut stream, request)?;
    let response: Response =
        framed_read(&mut stream)?.ok_or_else(|| Failure::general("the coordinator went away"))?;
    match response.error {
        None => Ok(response.services),
        Some(error) => Err(Failure::general(error)),
    }
}

/// Becomes the store worker: connects to the parent coordinator, enters a
/// private mount namespace with `/laft` writable, and returns the
/// arguments of the command to run.
///
/// # Errors
///
/// Fails when the process was not started by a `laft` coordinator, or the
/// namespace cannot be set up.
pub(crate) fn enter(args: Vec<String>) -> Result<Vec<String>, Failure> {
    let refuse = || {
        Failure::new(
            exit::USAGE,
            "laft internal worker runs only as the child of a laft process",
        )
    };
    let path = std::env::var_os(SOCKET_ENV).ok_or_else(refuse)?;
    let stream = UnixStream::connect(&path).map_err(|_| refuse())?;
    let parent = rustix::process::getppid().map(|p| p.as_raw_nonzero().get());
    if peer_pid(&stream).and_then(|p| i32::try_from(p).ok()) != parent {
        return Err(refuse());
    }
    laft_sandbox::private_mount_namespace()
        .map_err(|e| Failure::general(format!("entering a private mount namespace: {e}")))?;
    let laft = Path::new("/laft");
    if mount_points().iter().any(|p| p == laft) {
        rustix::mount::mount_remount(
            laft,
            MountFlags::BIND | MountFlags::NOSUID | MountFlags::NODEV,
            "",
        )
        .map_err(|e| Failure::general(format!("making /laft writable: {e}")))?;
    } else if read_only(laft) {
        return Err(Failure::general("/laft is read-only and not a mount point"));
    }
    let _ = CHANNEL.set(Mutex::new(stream));
    Ok(std::iter::once("laft".to_owned()).chain(args).collect())
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
