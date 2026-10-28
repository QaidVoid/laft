use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// An error from rendering the static `/etc` tree, writing the account
/// databases or activating `/etc`.
#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed.
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A file that Laft wrote itself is malformed, for example a state
    /// record or the declared accounts of a generation.
    Corrupt {
        /// The malformed file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// The store or a generation could not be read.
    Store(laft_store::Error),
    /// The configuration is invalid. Every problem found is listed.
    Config(Vec<ConfigError>),
    /// Two packages ship the same path in `etc/`.
    PackageConflict {
        /// The path, relative to `/etc`.
        path: String,
        /// Both packages, as `<name> <version>-r<rev>`.
        owners: [String; 2],
    },
    /// An account database has a line that cannot be parsed. Nothing was
    /// written. Code `C0410`.
    Malformed {
        /// The database.
        path: PathBuf,
        /// The 1-based line number.
        line: usize,
        /// What is wrong with the line.
        reason: String,
    },
    /// The account database lock was not acquired in time, because another
    /// program is editing the databases. Code `C0409`.
    LockTimeout(PathBuf),
    /// An account kept from the current databases has the id of a declared
    /// account. Code `C0411`.
    IdCollision {
        /// `user` or `group`.
        kind: &'static str,
        /// The declared account.
        declared: String,
        /// The undeclared account that has the same id.
        existing: String,
        /// The id.
        id: u32,
    },
    /// A secret file is missing, not owned by the user running Laft, or
    /// accessible by its group or others. Code `C0412`.
    Secret {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// A password file does not hold a valid password hash. Code `C0413`.
    InvalidHash(PathBuf),
    /// Paths in `/etc` that activation would have to overwrite although
    /// Laft did not create them. Nothing was changed.
    Conflicts(Vec<Conflict>),
    /// A path given to reset is not managed by the generation `/etc` was
    /// last activated for, or is an account database.
    NotResettable {
        /// The path, relative to `/etc`.
        path: String,
        /// Why it cannot be reset.
        reason: &'static str,
    },
    /// A request that the current state does not allow. The message says
    /// which.
    Invalid(String),
    /// A required trigger failed, or a trigger changed what it may not
    /// change.
    Trigger {
        /// The trigger instance, such as `depmod:6.17.5-laft1`.
        instance: String,
        /// What went wrong, with the tool's standard error.
        reason: String,
    },
    /// The sandbox of a required trigger could not be set up.
    Sandbox {
        /// The trigger instance.
        instance: String,
        /// The sandbox error.
        source: laft_sandbox::Error,
    },
    /// The services could not be rendered.
    Services(laft_svc::Error),
    /// A file needed to boot is missing or malformed, such as the kernel,
    /// a module index or a program of the initramfs.
    Boot(String),
    /// A failure injected by a test at a named point.
    #[cfg(test)]
    Injected(&'static str),
}

impl Error {
    /// The diagnostic code of the error, for example `C0411`, when it has
    /// one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Malformed { .. } => Some("C0410"),
            Self::LockTimeout(_) => Some("C0409"),
            Self::IdCollision { .. } => Some("C0411"),
            Self::Secret { .. } => Some("C0412"),
            Self::InvalidHash(_) => Some("C0413"),
            _ => None,
        }
    }
}

/// A path in `/etc` that activation refuses to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The path, relative to `/etc`.
    pub path: String,
    /// Why it is a conflict.
    pub kind: ConflictKind,
}

/// Why a path in `/etc` is a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// The path exists and Laft did not create it. Adopting it moves it to
    /// `<path>.laft-orig`.
    Unmanaged,
    /// The path is a symlink that points somewhere other than the static
    /// tree.
    ForeignLink(String),
    /// A copy-mode file was changed outside Laft since Laft last wrote it.
    Changed,
    /// The path should be adopted, but `<path>.laft-orig` exists already.
    OrigExists,
    /// A directory above the path exists as something other than a
    /// directory.
    NotDirectory(String),
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = &self.path;
        match &self.kind {
            ConflictKind::Unmanaged => write!(
                f,
                "/etc/{path} exists and is not managed by laft\n  hint: adopt /etc/{path} to move it to /etc/{path}.laft-orig"
            ),
            ConflictKind::ForeignLink(target) => write!(
                f,
                "/etc/{path} is a symlink to {target}, not into /etc/static\n  hint: adopt /etc/{path} to move it to /etc/{path}.laft-orig"
            ),
            ConflictKind::Changed => write!(
                f,
                "/etc/{path} was changed outside laft\n  hint: adopt /etc/{path} to move it to /etc/{path}.laft-orig"
            ),
            ConflictKind::OrigExists => write!(
                f,
                "/etc/{path} cannot be adopted because /etc/{path}.laft-orig exists"
            ),
            ConflictKind::NotDirectory(dir) => {
                write!(f, "/etc/{path} needs /etc/{dir} to be a directory")
            }
        }
    }
}

/// One way in which the configuration is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A user or group name is invalid. Code `C0405`.
    InvalidName {
        /// `user` or `group`.
        kind: &'static str,
        /// The name.
        name: String,
    },
    /// A user sets both `password_hash` and `password_file`. Code `C0406`.
    PasswordSources(String),
    /// An explicit id lies in a range reserved for the static accounts.
    /// Code `C0407`.
    ReservedId {
        /// `user` or `group`.
        kind: &'static str,
        /// The account.
        name: String,
        /// The id.
        id: u32,
    },
    /// The id of a static system account was changed. Code `C0408`.
    RegistryId {
        /// `user` or `group`.
        kind: &'static str,
        /// The account.
        name: String,
        /// The id the configuration gives it.
        id: u32,
        /// Its fixed id.
        fixed: u32,
    },
    /// Two declared accounts have the same id. Code `C0414`.
    DuplicateId {
        /// `user` or `group`.
        kind: &'static str,
        /// The id.
        id: u32,
        /// Both accounts.
        names: [String; 2],
    },
    /// A regular user would take a system group with its name as its
    /// primary group. Code `C0415`.
    SystemPrimaryGroup(String),
    /// No id is free in the range an account allocates from.
    IdsExhausted {
        /// `user` or `group`.
        kind: &'static str,
        /// The account.
        name: String,
    },
    /// A reference names an account that is not declared.
    UnknownAccount {
        /// `user` or `group`.
        kind: &'static str,
        /// The missing account.
        name: String,
        /// Where it is referenced.
        context: String,
    },
    /// A value cannot be written into an account database or a rendered
    /// file, for example because it contains a colon or a line break.
    InvalidValue {
        /// Where the value is set.
        context: String,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// A `file` path is not an absolute, normalized path below `/etc/`.
    /// Code `C0420`.
    InvalidPath(String),
    /// A `file` path is reserved for Laft. Code `C0421`.
    ReservedPath(String),
    /// An enabled `file` does not set exactly one content field. Code
    /// `C0422`.
    Content(String),
    /// A path is both a file and the directory of another entry. Code
    /// `C0425`.
    PathClash {
        /// The entry that needs a directory.
        path: String,
        /// The ancestor that is not a directory.
        file: String,
    },
    /// Structured content cannot be rendered in the chosen format. Code
    /// `C0426`.
    Structure {
        /// The file.
        path: String,
        /// Why.
        reason: String,
    },
    /// An `env` key is not a valid variable name. Code `C0427`.
    EnvKey {
        /// The file.
        path: String,
        /// The key.
        key: String,
    },
    /// A sysctl key is malformed. Code `C0431`.
    SysctlKey(String),
    /// A kernel module is both loaded and blacklisted. Code `C0433`.
    BlacklistedModule(String),
    /// A sysctl value is not an integer, a string or a boolean. Code
    /// `C0434`.
    SysctlValue(String),
    /// A privileged path is not an absolute, normalized path below
    /// `/usr/bin/` or `/usr/lib/`.
    PrivilegedPath(String),
    /// The capabilities of a privileged file do not parse.
    InvalidCaps {
        /// The privileged path.
        path: String,
        /// The capability text.
        caps: String,
    },
}

impl ConfigError {
    /// The diagnostic code of the error, for example `C0415`, when it has
    /// one.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidName { .. } => "C0405",
            Self::PasswordSources(_) => "C0406",
            Self::ReservedId { .. } => "C0407",
            Self::RegistryId { .. } => "C0408",
            Self::DuplicateId { .. } => "C0414",
            Self::SystemPrimaryGroup(_) => "C0415",
            Self::InvalidPath(_) => "C0420",
            Self::ReservedPath(_) => "C0421",
            Self::Content(_) => "C0422",
            Self::PathClash { .. } => "C0425",
            Self::Structure { .. } => "C0426",
            Self::EnvKey { .. } => "C0427",
            Self::SysctlKey(_) => "C0431",
            Self::BlacklistedModule(_) => "C0433",
            Self::SysctlValue(_) => "C0434",
            Self::IdsExhausted { .. }
            | Self::UnknownAccount { .. }
            | Self::InvalidValue { .. }
            | Self::PrivilegedPath(_)
            | Self::InvalidCaps { .. } => {
                return None;
            }
        })
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { kind, name } => write!(
                f,
                "invalid {kind} name {name:?}: expected a lowercase letter or `_`, then up to 31 of `a-z`, `0-9`, `_` and `-`"
            ),
            Self::PasswordSources(name) => {
                write!(f, "user {name:?} sets both password_hash and password_file")
            }
            Self::ReservedId { kind, name, id } => write!(
                f,
                "{kind} {name:?} has id {id}, which is reserved for the static system accounts"
            ),
            Self::RegistryId {
                kind,
                name,
                id,
                fixed,
            } => write!(
                f,
                "{kind} {name:?} is a static system account with id {fixed}, which cannot be changed to {id}"
            ),
            Self::DuplicateId { kind, id, names } => write!(
                f,
                "{kind}s {:?} and {:?} both have id {id}",
                names[0], names[1]
            ),
            Self::SystemPrimaryGroup(name) => write!(
                f,
                "user {name:?} would take the system group {name:?} as its primary group. Set group explicitly."
            ),
            Self::IdsExhausted { kind, name } => {
                write!(f, "no free id is left for {kind} {name:?}")
            }
            Self::UnknownAccount {
                kind,
                name,
                context,
            } => write!(f, "{context}: no {kind} named {name:?} is declared"),
            Self::InvalidValue { context, reason } => write!(f, "{context}: {reason}"),
            Self::InvalidPath(path) => write!(
                f,
                "file {path:?}: expected an absolute, normalized path below /etc/"
            ),
            Self::ReservedPath(path) => write!(f, "file {path:?}: the path is reserved for laft"),
            Self::Content(path) => write!(
                f,
                "file {path:?}: exactly one of text, source, link, json, toml, yaml, ini and env must be set"
            ),
            Self::PathClash { path, file } => write!(
                f,
                "{path:?} needs {file:?} to be a directory, but it is a file"
            ),
            Self::Structure { path, reason } => write!(f, "file {path:?}: {reason}"),
            Self::EnvKey { path, key } => {
                write!(f, "file {path:?}: env key {key:?} is not a variable name")
            }
            Self::SysctlKey(key) => write!(f, "sysctl key {key:?} is malformed"),
            Self::BlacklistedModule(module) => {
                write!(f, "kernel module {module:?} is both loaded and blacklisted")
            }
            Self::SysctlValue(key) => write!(
                f,
                "sysctl {key:?}: the value must be an integer, a string or a boolean"
            ),
            Self::PrivilegedPath(path) => write!(
                f,
                "privileged {path:?}: privileged files must live under /usr/bin or /usr/lib"
            ),
            Self::InvalidCaps { path, caps } => {
                write!(f, "privileged {path:?}: invalid capability text {caps:?}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Corrupt { path, reason } => write!(f, "{} is corrupt: {reason}", path.display()),
            Self::Store(e) => e.fmt(f),
            Self::Config(errors) => {
                let s = if errors.len() == 1 { "" } else { "s" };
                write!(f, "{} configuration error{s}", errors.len())?;
                for error in errors {
                    match error.code() {
                        Some(code) => write!(f, "\n  {code}: {error}")?,
                        None => write!(f, "\n  {error}")?,
                    }
                }
                Ok(())
            }
            Self::PackageConflict { path, owners } => write!(
                f,
                "/etc/{path} is shipped by both {} and {}",
                owners[0], owners[1]
            ),
            Self::Malformed { path, line, reason } => {
                write!(f, "{}:{line}: {reason}", path.display())
            }
            Self::LockTimeout(path) => write!(
                f,
                "{} is held by another program, which is editing the account databases",
                path.display()
            ),
            Self::IdCollision {
                kind,
                declared,
                existing,
                id,
            } => write!(
                f,
                "declared {kind} {declared:?} and undeclared {kind} {existing:?} both have id {id}"
            ),
            Self::Secret { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::InvalidHash(path) => {
                write!(f, "{} does not hold a valid password hash", path.display())
            }
            Self::Conflicts(conflicts) => {
                let s = if conflicts.len() == 1 { "" } else { "s" };
                write!(f, "{} conflict{s} in /etc", conflicts.len())?;
                for conflict in conflicts {
                    write!(f, "\n  {conflict}")?;
                }
                Ok(())
            }
            Self::NotResettable { path, reason } => write!(f, "/etc/{path}: {reason}"),
            Self::Invalid(message) => f.write_str(message),
            Self::Trigger { instance, reason } => write!(f, "trigger {instance}: {reason}"),
            Self::Sandbox { instance, source } => {
                write!(f, "trigger {instance}: cannot set up the sandbox: {source}")
            }
            Self::Services(e) => e.fmt(f),
            Self::Boot(message) => f.write_str(message),
            #[cfg(test)]
            Self::Injected(point) => write!(f, "injected failure at {point}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Store(e) => Some(e),
            Self::Sandbox { source, .. } => Some(source),
            Self::Services(e) => Some(e),
            _ => None,
        }
    }
}

impl From<laft_store::Error> for Error {
    fn from(e: laft_store::Error) -> Self {
        Self::Store(e)
    }
}

/// The result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Wraps an I/O error from an operation on `path`.
pub(crate) fn io(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// An error for a malformed file that Laft wrote.
pub(crate) fn corrupt(path: &Path, reason: impl fmt::Display) -> Error {
    Error::Corrupt {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

/// Something that does not stop rendering or activation, but that the user
/// should know about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// The user replaced a managed symlink, so Laft leaves the path alone
    /// until the configuration stops managing it or the path is reset.
    UserReplaced(String),
    /// A conflicting path was left alone, because activation at boot cannot
    /// stop.
    Skipped(Conflict),
    /// An account that was not declared was removed, because the accounts
    /// are not mutable.
    RemovedAccount {
        /// `user` or `group`.
        kind: &'static str,
        /// The account.
        name: String,
    },
    /// A declared file is not world readable. Its content still stays in
    /// the generation until the generation is collected, so it should not
    /// be a secret.
    PrivateFile {
        /// The absolute path.
        path: String,
        /// Its mode.
        mode: u32,
    },
    /// An optional trigger failed. The generation has no output for it.
    TriggerFailed {
        /// The trigger instance.
        instance: String,
        /// Why, with the tool's standard error.
        reason: String,
    },
    /// An enabled privileged entry names a path that is not a regular file
    /// of the generation, so it has no effect.
    InactivePrivileged(String),
    /// A line of an account database had fewer fields than the database
    /// has, as another tool wrote it. The missing fields were read as
    /// empty.
    ShortLine {
        /// The database.
        path: PathBuf,
        /// The 1-based line number.
        line: usize,
    },
    /// The initramfs is host only, but the drivers of the boot devices of
    /// the assembling machine could not be found, so it carries the generic
    /// storage and input drivers instead.
    NoHostModules,
    /// A copy-mode file that is no longer declared was changed outside
    /// Laft, so it is left in place and Laft forgets it.
    ChangedCopyKept(String),
    /// Laft did not run as root, so the privileged copies were made and
    /// recorded but keep the caller as owner and carry neither setuid,
    /// setgid nor capabilities.
    PrivilegesNotApplied(Vec<String>),
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserReplaced(path) => write!(
                f,
                "user replaced managed file /etc/{path}, leaving it\n  hint: reset /etc/{path} to manage it again"
            ),
            Self::Skipped(conflict) => write!(f, "left alone: {conflict}"),
            Self::ShortLine { path, line } => write!(
                f,
                "{}:{line}: the line has missing fields, which were read as empty",
                path.display()
            ),
            Self::NoHostModules => f.write_str(
                "the boot device drivers of this machine are unknown, so the host only initramfs carries the generic drivers",
            ),
            Self::ChangedCopyKept(path) => write!(
                f,
                "/etc/{path} is no longer declared but was changed outside laft, leaving it"
            ),
            Self::RemovedAccount { kind, name } => write!(
                f,
                "removed undeclared {kind} {name:?}, because users.mutable is false"
            ),
            Self::PrivateFile { path, mode } => write!(
                f,
                "file {path:?} has mode {mode:o} but stays on disk in every generation that declares it. Keep secrets out of declared files."
            ),
            Self::TriggerFailed { instance, reason } => {
                write!(f, "optional trigger {instance} failed: {reason}")
            }
            Self::InactivePrivileged(path) => write!(
                f,
                "privileged {path:?} is inactive: the generation has no regular file at that path"
            ),
            Self::PrivilegesNotApplied(paths) => write!(
                f,
                "not running as root: the owner, setuid and setgid bits and capabilities of {} were recorded in privileged.json but not applied",
                paths.join(", ")
            ),
        }
    }
}
