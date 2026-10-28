use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Declares [`Step`] with the description of each step, so that the list
/// of codes a sandbox child may send cannot miss a variant.
macro_rules! steps {
    ($($step:ident => $description:literal,)+) => {
        /// A step of sandbox setup or view entry. A failing step is reported
        /// with its description, and a sandbox child sends the step's code to
        /// its parent.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        pub(crate) enum Step {
            $($step,)+
        }

        impl Step {
            const ALL: &[Self] = &[$(Self::$step,)+];

            fn describe(self) -> &'static str {
                match self {
                    $(Self::$step => $description,)+
                }
            }
        }
    };
}

steps! {
    Resolve => "resolving a sandbox directory",
    Pipe => "creating the setup pipe",
    Unshare => "creating the namespaces",
    MapIds => "mapping the build user",
    Limits => "setting resource limits",
    Fork => "starting the namespace init",
    DeathSignal => "tying the namespace init to its parent",
    Propagation => "changing mount propagation",
    CloneTree => "cloning a mount",
    SetAttributes => "setting mount attributes",
    RootFs => "creating the root filesystem",
    Populate => "creating mount points",
    Attach => "attaching a mount",
    MountProc => "mounting /proc",
    MountDev => "mounting /dev",
    MountTmp => "mounting /tmp",
    MountEtc => "binding the host's name resolution files into /etc",
    PivotRoot => "switching to the new root",
    Hostname => "setting the hostname",
    Capabilities => "dropping capabilities",
    NoNewPrivs => "setting no_new_privs",
    WorkingDirectory => "changing to the working directory",
    Descriptors => "closing inherited descriptors",
    Exec => "executing the command",
    Wait => "waiting for the command",
    Unknown => "setting up the sandbox at a step with an unknown code",
    InvalidPath => "expecting an absolute path below /usr, got",
    OpenTarget => "opening a mount target",
    NotRegularFile => "expecting a regular file at",
    InvalidWritable => "expecting a directory below usr reached without symlinks, got",
    Writable => "attaching a writable directory",
    BindEtc => "binding the configuration over /etc",
    MountScratch => "mounting a private tmpfs",
    SwitchIds => "switching to user nobody",
}

impl Step {
    /// The step whose code is `code`, or [`Step::Unknown`] for a code
    /// that names no step.
    pub(crate) fn from_code(code: u8) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|step| *step as u8 == code)
            .unwrap_or(Self::Unknown)
    }
}

/// An error from setting up a sandbox or from entering a view.
///
/// The message names the step that failed and, where there is one, the path
/// it was about, for example `mounting /proc: Operation not permitted (os
/// error 1)`. Input that is malformed before any system call is made carries
/// no operating system error.
#[derive(Debug)]
pub struct Error {
    step: Step,
    path: Option<PathBuf>,
    source: Option<io::Error>,
}

impl Error {
    pub(crate) fn new(step: Step, source: impl Into<io::Error>) -> Self {
        Self {
            step,
            path: None,
            source: Some(source.into()),
        }
    }

    pub(crate) fn at(step: Step, path: &Path, source: impl Into<io::Error>) -> Self {
        Self {
            step,
            path: Some(path.to_owned()),
            source: Some(source.into()),
        }
    }

    /// An error about the input `path` itself, which no system call reported.
    pub(crate) fn malformed(step: Step, path: &Path) -> Self {
        Self {
            step,
            path: Some(path.to_owned()),
            source: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn step(&self) -> Step {
        self.step
    }

    /// The operating system error behind the failure, or `None` when the
    /// input was rejected without one: a privileged path that is malformed
    /// or that names something other than a regular file.
    ///
    /// A missing kernel feature shows up as `ENOSYS`, and a kernel or
    /// policy that forbids unprivileged namespaces as `EPERM`, `EACCES` or
    /// `ENOSPC`.
    pub fn io_error(&self) -> Option<&io::Error> {
        self.source.as_ref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.step.describe())?;
        if let Some(path) = &self.path {
            write!(f, " {}", path.display())?;
        }
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}
