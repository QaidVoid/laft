//! The init-agnostic service model, as the system configuration declares it.
//!
//! Every type deserializes from the evaluated configuration. Fields that the
//! configuration leaves out take the defaults documented on them.

use std::collections::{BTreeMap, BTreeSet};

use laft_core::Name;
use serde::{Deserialize, Serialize};

/// The init system a generation boots with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Init {
    /// runit, the only backend this version implements.
    #[default]
    Runit,
    /// s6. Reserved, and rejected by validation.
    S6,
    /// dinit. Reserved, and rejected by validation.
    Dinit,
}

impl Init {
    /// The name of the backend, as recorded in a generation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runit => "runit",
            Self::S6 => "s6",
            Self::Dinit => "dinit",
        }
    }

    /// The command line of PID 1, or `None` for a backend this version does
    /// not implement.
    ///
    /// ```
    /// assert_eq!(laft_svc::Init::Runit.pid1(), Some(&["/usr/bin/runit"][..]));
    /// assert_eq!(laft_svc::Init::S6.pid1(), None);
    /// ```
    pub fn pid1(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Runit => Some(&["/usr/bin/runit"]),
            Self::S6 | Self::Dinit => None,
        }
    }
}

/// How a service is supervised.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    /// A supervised process that runs `exec` and is restarted by policy.
    #[default]
    Longrun,
    /// Commands that run to completion: `up` on start, `down` on stop.
    Oneshot,
}

impl ServiceKind {
    /// The kind as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Longrun => "longrun",
            Self::Oneshot => "oneshot",
        }
    }
}

/// When a service counts as ready, so that services ordered after it may
/// start.
///
/// It deserializes in the form an evaluated configuration takes: the name
/// of a variant without fields, such as `"immediate"`, or a map from the
/// variant name to its fields, such as `{"notify": {"fd": 3}}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum Ready {
    /// As soon as the service's command is executed.
    #[default]
    Immediate,
    /// When the service writes a newline to file descriptor `fd`. Closing
    /// the descriptor afterwards is allowed but not required. Closing it
    /// before a newline means the service never becomes ready.
    Notify {
        /// The descriptor the service writes to, 3 to 1023.
        fd: u32,
    },
    /// When `cmd`, run as the service user, exits with status 0.
    Check {
        /// The command, as argv. Its first element is an absolute path.
        cmd: Vec<String>,
        /// Seconds between two attempts, 1 to 3600. Default 1.
        #[serde(default = "default_interval")]
        interval: u32,
        /// Seconds after which the check gives up, 1 to 86400. Default 60.
        #[serde(default = "default_timeout")]
        timeout: u32,
    },
}

fn default_interval() -> u32 {
    1
}

fn default_timeout() -> u32 {
    60
}

/// Where the output of a service goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Log {
    /// Into the service's own rotated log directory under `/var/log`.
    #[default]
    Logged,
    /// Nowhere.
    Discard,
    /// To the console of the supervisor.
    Console,
}

/// Whether the supervisor restarts a longrun after it exits.
///
/// Exit status 111 is reserved for launcher failures, such as a dependency
/// that is not ready yet, and restarts the service under every policy but
/// [`Restart::Never`]. A service whose own program exits with 111 is
/// treated the same way, because the supervisor cannot tell the two apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    /// Always.
    #[default]
    Always,
    /// Unless it exited with status 0.
    OnFailure,
    /// Never, not even after a launcher failure. A service with this
    /// policy that fails its dependency gate stays down, so it should only
    /// depend on services that are ready before it starts.
    Never,
}

/// The signal that asks a service to stop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Signal {
    /// `SIGTERM`.
    #[default]
    Term,
    /// `SIGINT`.
    Int,
    /// `SIGHUP`.
    Hup,
    /// `SIGQUIT`.
    Quit,
    /// `SIGUSR1`.
    Usr1,
    /// `SIGUSR2`.
    Usr2,
}

impl Signal {
    const ALL: [Self; 6] = [
        Self::Term,
        Self::Int,
        Self::Hup,
        Self::Quit,
        Self::Usr1,
        Self::Usr2,
    ];

    /// The signal with number `number`, if it is one a service may stop
    /// with.
    pub(crate) fn from_number(number: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.number() == number)
    }

    /// The name `kill -s` takes, such as `INT`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Term => "TERM",
            Self::Int => "INT",
            Self::Hup => "HUP",
            Self::Quit => "QUIT",
            Self::Usr1 => "USR1",
            Self::Usr2 => "USR2",
        }
    }

    /// The signal number on Linux.
    pub fn number(self) -> i32 {
        match self {
            Self::Term => 15,
            Self::Int => 2,
            Self::Hup => 1,
            Self::Quit => 3,
            Self::Usr1 => 10,
            Self::Usr2 => 12,
        }
    }
}

/// What live reconciliation does when only the spec of a service changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OnChange {
    /// Stop the service, then start it again.
    Restart,
    /// Run the oneshot's `down` command, if any, then its `up` command.
    Rerun,
    /// Leave the running service alone.
    Ignore,
    /// Leave it alone and tell the administrator that a reboot applies it.
    Reboot,
}

/// How a logged service's output is rotated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogRotation {
    /// Bytes per file before rotation, 4096 to 1 GiB. Default 1000000.
    pub size: u64,
    /// Rotated files kept, 1 to 1000. Default 10.
    pub count: u32,
}

impl Default for LogRotation {
    fn default() -> Self {
        Self {
            size: 1_000_000,
            count: 10,
        }
    }
}

/// A directory the launcher creates and corrects before every start.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dir {
    /// The path relative to `/var/lib` for state directories, or to `/run`
    /// for runtime directories. Components consist of `A-Z`, `a-z`, `0-9`,
    /// `.`, `_`, `@`, `+` and `-`, and are never `.` or `..`.
    pub path: String,
    /// The permission bits, at most `0o7777`. Default `0o755`.
    #[serde(default = "default_dir_mode")]
    pub mode: u32,
    /// The owning user. Default: the service user.
    #[serde(default)]
    pub owner: Option<String>,
    /// The owning group. Default: the primary group of `owner` when it is
    /// set, otherwise the service's group.
    #[serde(default)]
    pub group: Option<String>,
}

fn default_dir_mode() -> u32 {
    0o755
}

impl Dir {
    /// A directory at `path` with the default mode and owners.
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            mode: default_dir_mode(),
            owner: None,
            group: None,
        }
    }
}

/// Resource limits. Each set limit applies as both the soft and the hard
/// limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Open files, 1 to 1073741816.
    pub nofile: Option<u64>,
    /// Processes of the service user, 1 to 4194304.
    pub nproc: Option<u64>,
    /// Core dump size in bytes, at most 1 TiB. 0 disables core dumps.
    pub core: Option<u64>,
    /// Locked memory in bytes, at most 1 TiB.
    pub memlock: Option<u64>,
}

/// A system service, keyed by its name in [`ServiceConfig::services`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Service {
    /// A one-line description for the administrator.
    pub description: String,
    /// How the service is supervised. Default longrun.
    pub kind: ServiceKind,
    /// Whether the service is part of the generation. Default true.
    pub enable: bool,
    /// For a longrun, the command to supervise, as argv.
    pub exec: Vec<String>,
    /// For a oneshot, the command run on start, as argv.
    pub up: Vec<String>,
    /// For a oneshot, the command run on stop, as argv. May be empty.
    pub down: Vec<String>,
    /// The account the service runs as. Default `root`.
    pub user: String,
    /// The primary group. Default: the primary group of `user`.
    pub group: Option<String>,
    /// Supplementary groups.
    pub groups: BTreeSet<String>,
    /// Environment variables, added to a minimal environment.
    pub env: BTreeMap<String, String>,
    /// The working directory, an absolute path. Default `/`.
    pub cwd: String,
    /// The file mode creation mask, at most `0o777`. Default `0o022`.
    pub umask: u32,
    /// Services that must be ready before this one starts.
    pub after: Vec<String>,
    /// When the service counts as ready.
    pub ready: Ready,
    /// Where its output goes.
    pub log: Log,
    /// How its log is rotated when `log` is [`Log::Logged`].
    pub rotate: LogRotation,
    /// Directories under `/var/lib` to create before every start.
    pub state_dirs: Vec<Dir>,
    /// Directories under `/run` to create before every start.
    pub runtime_dirs: Vec<Dir>,
    /// The restart policy of a longrun.
    pub restart: Restart,
    /// Seconds to wait before a restart, at most 3600. Default 1.
    pub restart_delay: u32,
    /// The signal that asks the service to stop. Default `TERM`.
    pub stop_signal: Signal,
    /// Seconds between the stop signal and `SIGKILL`, 1 to 3600. Default 10.
    pub stop_timeout: u32,
    /// Resource limits.
    pub limits: Limits,
    /// What reconciliation does when only the spec changes. Default:
    /// restart for longruns, rerun for oneshots.
    pub on_change: Option<OnChange>,
    /// Whether the service belongs to the early boot graph. Reserved for
    /// the standard services.
    pub early: bool,
    /// Packages added to the system view while the service is enabled.
    pub packages: BTreeSet<Name>,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            description: String::new(),
            kind: ServiceKind::Longrun,
            enable: true,
            exec: Vec::new(),
            up: Vec::new(),
            down: Vec::new(),
            user: "root".to_owned(),
            group: None,
            groups: BTreeSet::new(),
            env: BTreeMap::new(),
            cwd: "/".to_owned(),
            umask: 0o022,
            after: Vec::new(),
            ready: Ready::Immediate,
            log: Log::Logged,
            rotate: LogRotation::default(),
            state_dirs: Vec::new(),
            runtime_dirs: Vec::new(),
            restart: Restart::Always,
            restart_delay: 1,
            stop_signal: Signal::Term,
            stop_timeout: 10,
            limits: Limits::default(),
            on_change: None,
            early: false,
            packages: BTreeSet::new(),
        }
    }
}

impl Service {
    /// A longrun that supervises `exec`, with every other field at its
    /// default.
    pub fn longrun(exec: &[&str]) -> Self {
        Self {
            exec: strings(exec),
            ..Self::default()
        }
    }

    /// A oneshot that runs `up` on start, with every other field at its
    /// default.
    pub fn oneshot(up: &[&str]) -> Self {
        Self {
            kind: ServiceKind::Oneshot,
            up: strings(up),
            ..Self::default()
        }
    }
}

pub(crate) fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|&s| s.to_owned()).collect()
}

/// The terminals that get a login prompt.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Getty {
    /// Terminal names such as `tty1` or `ttyS0`. Each gets a service named
    /// `getty-<tty>`. Default `tty1` and `tty2`.
    pub ttys: Vec<String>,
}

impl Default for Getty {
    fn default() -> Self {
        Self {
            ttys: strings(&["tty1", "tty2"]),
        }
    }
}

/// The parts of the system configuration that describe services.
///
/// Other fields of the system configuration are ignored when deserializing,
/// so the whole evaluated system value can be read into this type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    /// The init system.
    pub init: Init,
    /// Every declared service by name, including the standard ones, after
    /// all contributions are merged.
    pub services: BTreeMap<String, Service>,
    /// The terminals that get a login prompt.
    pub getty: Getty,
}
