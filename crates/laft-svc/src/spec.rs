//! The service spec file: the complete, resolved description of how to run
//! one service, which the launcher reads at every start.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use laft_core::Blake3Hash;
use serde::{Deserialize, Serialize};

use crate::error::{Error, corrupt, io};
use crate::model::{OnChange, Restart, ServiceKind};

/// The version of the spec file format.
pub const SPEC_VERSION: u32 = 1;

/// The resolved description of one enabled service, stored as canonical
/// JSON in `services/spec/<name>.json` of a generation.
///
/// Defaults are filled in, the implicit dependency on [`crate::EARLY`] is
/// part of `after`, directory paths are absolute, durations are in
/// milliseconds and signals are numbers. Account names stay names, because
/// the ids of accounts that are not in the static registry can change, so
/// the launcher resolves them at every start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// The format version, [`SPEC_VERSION`].
    pub spec_version: u32,
    /// The service name.
    pub name: String,
    /// How the service is supervised.
    pub kind: ServiceKind,
    /// Whether the service belongs to the early boot graph.
    pub early: bool,
    /// The command of a longrun, empty for a oneshot.
    pub exec: Vec<String>,
    /// The start command of a oneshot, empty for a longrun.
    pub up: Vec<String>,
    /// The stop command of a oneshot, possibly empty.
    pub down: Vec<String>,
    /// The account the service runs as.
    pub user: String,
    /// The primary group, or `None` for the primary group of `user`.
    pub group: Option<String>,
    /// Supplementary groups, sorted.
    pub groups: Vec<String>,
    /// Environment variables added to the minimal environment.
    pub env: BTreeMap<String, String>,
    /// The working directory.
    pub cwd: String,
    /// The file mode creation mask.
    pub umask: u32,
    /// The services that must be ready first, sorted and without
    /// duplicates.
    pub after: Vec<String>,
    /// When the service counts as ready.
    pub ready: SpecReady,
    /// Where the output goes.
    pub log: SpecLog,
    /// Directories under `/var/lib` to create before every start.
    pub state_dirs: Vec<SpecDir>,
    /// Directories under `/run` to create before every start.
    pub runtime_dirs: Vec<SpecDir>,
    /// The restart policy.
    pub restart: Restart,
    /// Milliseconds to wait before a restart.
    pub restart_delay_ms: u64,
    /// The number of the signal that asks the service to stop.
    pub stop_signal: i32,
    /// Milliseconds between the stop signal and `SIGKILL`.
    pub stop_timeout_ms: u64,
    /// Resource limits.
    pub limits: SpecLimits,
    /// What reconciliation does when only the spec changes.
    pub on_change: OnChange,
}

/// When a service counts as ready, with durations in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum SpecReady {
    /// As soon as the command is executed.
    Immediate,
    /// When the service writes a newline to descriptor `fd`.
    Notify {
        /// The descriptor.
        fd: u32,
    },
    /// When `cmd` exits with status 0.
    Check {
        /// The command, as argv.
        cmd: Vec<String>,
        /// Milliseconds between two attempts.
        interval_ms: u64,
        /// Milliseconds after which the check gives up.
        timeout_ms: u64,
    },
}

/// Where the output of a service goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum SpecLog {
    /// Into a rotated log directory written by the unprivileged `log`
    /// account.
    Logged {
        /// The log directory, always `/var/log/<name>`.
        dir: String,
        /// Bytes per file before rotation.
        size: u64,
        /// Rotated files kept.
        count: u32,
    },
    /// Nowhere.
    Discard,
    /// To the console of the supervisor.
    Console,
}

/// A directory to create before every start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecDir {
    /// The absolute path.
    pub path: String,
    /// The permission bits.
    pub mode: u32,
    /// The owning user, or `None` for the service user.
    pub owner: Option<String>,
    /// The owning group, or `None` for the primary group of `owner` when it
    /// is set, and the service's group otherwise.
    pub group: Option<String>,
}

/// Resource limits, each applied as both soft and hard limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecLimits {
    /// Open files.
    pub nofile: Option<u64>,
    /// Processes of the service user.
    pub nproc: Option<u64>,
    /// Core dump size in bytes.
    pub core: Option<u64>,
    /// Locked memory in bytes.
    pub memlock: Option<u64>,
}

impl Spec {
    /// The canonical JSON bytes of the spec file.
    ///
    /// # Errors
    ///
    /// Fails only when a number is too large to be written exactly as
    /// canonical JSON, which validated specs never contain.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        laft_core::to_canonical_json(self).map_err(|e| Error::Encode {
            name: self.name.clone(),
            reason: e.to_string(),
        })
    }

    /// Reads the spec file at `path`.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read, and when it is not a spec of
    /// version [`SPEC_VERSION`].
    pub fn read(path: &Path) -> Result<Self, Error> {
        let bytes = fs::read(path).map_err(io(path))?;
        Self::parse(&bytes, path)
    }

    pub(crate) fn parse(bytes: &[u8], path: &Path) -> Result<Self, Error> {
        #[derive(Deserialize)]
        struct Version {
            spec_version: u32,
        }
        let version: Version =
            serde_json::from_slice(bytes).map_err(|e| corrupt(path, e.to_string()))?;
        if version.spec_version != SPEC_VERSION {
            return Err(corrupt(
                path,
                format!("unsupported spec version {}", version.spec_version),
            ));
        }
        serde_json::from_slice(bytes).map_err(|e| corrupt(path, e.to_string()))
    }

    /// The spec hash: the BLAKE3 hash of the spec file's bytes.
    ///
    /// # Errors
    ///
    /// Fails as [`Self::to_bytes`] does.
    pub fn hash(&self) -> Result<Blake3Hash, Error> {
        Ok(Blake3Hash::of(&self.to_bytes()?))
    }

    /// The ready timeout used when waiting for the service: the check
    /// timeout, or 60 seconds for other kinds of readiness.
    pub fn ready_timeout_ms(&self) -> u64 {
        match self.ready {
            SpecReady::Check { timeout_ms, .. } => timeout_ms,
            SpecReady::Immediate | SpecReady::Notify { .. } => 60_000,
        }
    }
}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
