//! The lock of a build root, which keeps two runs from writing to one build
//! root at the same time.

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use laft_core::Blake3Hash;
use rustix::fs::FlockOperation;
use serde::{Deserialize, Serialize};

use crate::BuildRoot;
use crate::error::{BuildError, io};

/// The environment variable through which a run that holds the lock of a
/// build root shares it with the `laft-build` it starts inside a root, see
/// [`BuildRoot::lock`].
pub const LOCK_ENV: &str = "LAFT_BUILD_LOCK";

/// The exclusive lock of a [`BuildRoot`], held until it is dropped or the
/// process exits.
#[derive(Debug)]
pub struct RootLock {
    token: String,
    _file: Option<File>,
}

impl RootLock {
    /// The token that lets a `laft-build` this run starts share the lock,
    /// passed in [`LOCK_ENV`].
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// What the lock file says about its holder.
#[derive(Debug, Serialize, Deserialize)]
struct Holder {
    pid: u32,
    command: String,
    token: String,
}

impl BuildRoot {
    /// The lock file, `lock` in the build root.
    pub fn lock_path(&self) -> PathBuf {
        self.path().join("lock")
    }

    /// Takes the exclusive lock of the build root for `command`, which every
    /// command that writes to the build root holds for its whole run. It is
    /// an advisory lock on the file [`BuildRoot::lock_path`], which also
    /// names the process that holds it.
    ///
    /// A run that holds the lock and starts another `laft-build` on the same
    /// build root, such as a build inside the root of a stage, passes its
    /// [`RootLock::token`] as `inherited`, and that run shares the lock
    /// instead of waiting for it.
    ///
    /// # Errors
    ///
    /// Fails with `B032` at once when another run holds the lock and
    /// `inherited` is not its token, and with [`BuildError::Io`] when the
    /// build root or the lock file cannot be created or locked.
    pub fn lock(&self, command: &str, inherited: Option<&str>) -> Result<RootLock, BuildError> {
        let path = self.lock_path();
        fs::create_dir_all(self.path()).map_err(io(self.path()))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(io(&path))?;
        match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(rustix::io::Errno::WOULDBLOCK) => {
                let holder = fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Holder>(&b).ok());
                if let (Some(holder), Some(token)) = (&holder, inherited)
                    && holder.token == token
                {
                    return Ok(RootLock {
                        token: token.to_owned(),
                        _file: None,
                    });
                }
                let who = holder.map_or_else(
                    || "another laft-build run".to_owned(),
                    |h| format!("laft-build {} (pid {})", h.command, h.pid),
                );
                return Err(BuildError::problem(
                    "B032",
                    format!(
                        "the build root {} is in use: {who} holds its lock {}",
                        self.path().display(),
                        path.display()
                    ),
                ));
            }
            Err(e) => return Err(io(&path)(e.into())),
        }
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let token =
            Blake3Hash::of(format!("{pid} {nanos} {}", path.display()).as_bytes()).to_string();
        let holder = Holder {
            pid,
            command: command.to_owned(),
            token: token.clone(),
        };
        let bytes = serde_json::to_vec(&holder)
            .map_err(|e| BuildError::problem("B023", format!("{}: {e}", path.display())))?;
        file.set_len(0)
            .and_then(|()| file.write_all_at(&bytes, 0))
            .map_err(io(&path))?;
        Ok(RootLock {
            token,
            _file: Some(file),
        })
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
