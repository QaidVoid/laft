use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// A step of sandbox setup or view entry. A failing step is reported with
/// its description, and a sandbox child sends the step's code to its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Step {
    Resolve,
    Pipe,
    Unshare,
    MapIds,
    Limits,
    Fork,
    Propagation,
    CloneTree,
    SetAttributes,
    RootFs,
    Populate,
    Attach,
    MountProc,
    MountDev,
    MountTmp,
    PivotRoot,
    Hostname,
    Capabilities,
    NoNewPrivs,
    Descriptors,
    Exec,
    Wait,
}

impl Step {
    const ALL: [Self; 22] = [
        Self::Resolve,
        Self::Pipe,
        Self::Unshare,
        Self::MapIds,
        Self::Limits,
        Self::Fork,
        Self::Propagation,
        Self::CloneTree,
        Self::SetAttributes,
        Self::RootFs,
        Self::Populate,
        Self::Attach,
        Self::MountProc,
        Self::MountDev,
        Self::MountTmp,
        Self::PivotRoot,
        Self::Hostname,
        Self::Capabilities,
        Self::NoNewPrivs,
        Self::Descriptors,
        Self::Exec,
        Self::Wait,
    ];

    pub(crate) fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|step| *step as u8 == code)
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Resolve => "resolving a sandbox directory",
            Self::Pipe => "creating the setup pipe",
            Self::Unshare => "creating the namespaces",
            Self::MapIds => "mapping the build user",
            Self::Limits => "setting resource limits",
            Self::Fork => "starting the namespace init",
            Self::Propagation => "changing mount propagation",
            Self::CloneTree => "cloning a mount",
            Self::SetAttributes => "setting mount attributes",
            Self::RootFs => "creating the root filesystem",
            Self::Populate => "creating mount points",
            Self::Attach => "attaching a mount",
            Self::MountProc => "mounting /proc",
            Self::MountDev => "mounting /dev",
            Self::MountTmp => "mounting /tmp",
            Self::PivotRoot => "switching to the new root",
            Self::Hostname => "setting the hostname",
            Self::Capabilities => "dropping capabilities",
            Self::NoNewPrivs => "setting no_new_privs",
            Self::Descriptors => "closing inherited descriptors",
            Self::Exec => "executing the command",
            Self::Wait => "waiting for the command",
        }
    }
}

/// An error from setting up a build sandbox.
///
/// The message names the step that failed and, where there is one, the path
/// it was about, for example `mounting /proc: Operation not permitted (os
/// error 1)`.
#[derive(Debug)]
pub struct Error {
    step: Step,
    path: Option<PathBuf>,
    source: io::Error,
}

impl Error {
    pub(crate) fn new(step: Step, source: impl Into<io::Error>) -> Self {
        Self {
            step,
            path: None,
            source: source.into(),
        }
    }

    pub(crate) fn at(step: Step, path: &Path, source: impl Into<io::Error>) -> Self {
        Self {
            step,
            path: Some(path.to_owned()),
            source: source.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn step(&self) -> Step {
        self.step
    }

    /// The operating system error behind the failure.
    ///
    /// A missing kernel feature shows up as `ENOSYS`, and a kernel or
    /// policy that forbids unprivileged namespaces as `EPERM`, `EACCES` or
    /// `ENOSPC`.
    pub fn io_error(&self) -> &io::Error {
        &self.source
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.step.describe())?;
        if let Some(path) = &self.path {
            write!(f, " {}", path.display())?;
        }
        write!(f, ": {}", self.source)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
