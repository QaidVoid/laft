use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use laft_core::Timestamp;
use rustix::fs::FlockOperation;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, io};
use crate::root::{read_json, to_json};
use crate::{Root, fsx};

/// The exclusive transaction lock of a root.
///
/// Every command that writes under the root holds it, so transactions,
/// recovery and garbage collection never overlap. It is an advisory lock on
/// `laft/state/lock`, released when the `Lock` is dropped or the process
/// exits, and the holder is recorded in `laft/state/lock.info` so that a
/// waiting command can say who it waits for.
///
/// ```no_run
/// use laft_store::{Error, Root};
///
/// let root = Root::default();
/// let lock = match root.try_lock("apply") {
///     Err(Error::Busy(Some(holder))) => {
///         eprintln!("waiting for lock held by pid {} ({})", holder.pid, holder.command);
///         root.lock("apply")?
///     }
///     other => other?,
/// };
/// for recovery in lock.recover()? {
///     eprintln!("{recovery}");
/// }
/// # Ok::<(), Error>(())
/// ```
#[derive(Debug)]
pub struct Lock {
    root: Root,
    _file: File,
}

/// Who holds the transaction lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockInfo {
    /// The process id of the holder.
    pub pid: u32,
    /// The command it runs, for example `apply`.
    pub command: String,
    /// When it took the lock, in RFC 3339 UTC.
    pub started: String,
}

impl Root {
    /// Takes the transaction lock for `command`, waiting while another
    /// process holds it. Creates the directories under `laft/` that are
    /// missing first.
    ///
    /// # Errors
    ///
    /// Fails when the lock file cannot be created, locked or described.
    pub fn lock(&self, command: &str) -> Result<Lock> {
        self.acquire(command, FlockOperation::LockExclusive)
    }

    /// Takes the transaction lock for `command` unless another process holds
    /// it. Creates the directories under `laft/` that are missing first.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Busy`], naming the holder when it is known, when
    /// the lock is held, and otherwise as [`Self::lock`] does.
    pub fn try_lock(&self, command: &str) -> Result<Lock> {
        self.acquire(command, FlockOperation::NonBlockingLockExclusive)
    }

    fn acquire(&self, command: &str, operation: FlockOperation) -> Result<Lock> {
        self.init()?;
        let path = self.lock_path();
        // The descriptor is read-only so that the lock can be taken where
        // `laft` is mounted read-only. Only creating the file needs write
        // access.
        if !path.exists() {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(io(&path))?;
        }
        let file = File::open(&path).map_err(io(&path))?;
        match rustix::fs::flock(&file, operation) {
            Ok(()) => {}
            Err(rustix::io::Errno::WOULDBLOCK) => {
                return Err(Error::Busy(read_json(&self.lock_info_path()).ok()));
            }
            Err(e) => return Err(io(&path)(e)),
        }
        let info = LockInfo {
            pid: std::process::id(),
            command: command.to_owned(),
            started: Timestamp::now().to_string(),
        };
        fsx::write_atomic(&self.lock_info_path(), &to_json(&info)?, 0o644)?;
        Ok(Lock {
            root: self.clone(),
            _file: file,
        })
    }

    fn lock_path(&self) -> PathBuf {
        self.state_dir().join("lock")
    }

    fn lock_info_path(&self) -> PathBuf {
        self.state_dir().join("lock.info")
    }
}

impl Lock {
    /// The root this lock protects.
    pub fn root(&self) -> &Root {
        &self.root
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
