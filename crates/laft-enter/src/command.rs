//! Running the command once privileges are gone: its environment, working
//! directory, path search and exit status.
//!
//! The caller's environment was copied before the privileged steps, but it
//! is interpreted only here. Nothing in it influenced those steps.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;

use rustix::fs::Mode;
use rustix::io::Errno;

use crate::Failure;

/// The variable that tells the command which view it runs in.
pub(crate) const VIEW_VARIABLE: &str = "LAFT_VIEW";

/// The search path when the environment has no `PATH`.
const DEFAULT_PATH: &str = "/usr/bin";

/// The shell when `SHELL` is unset or not an absolute path.
const DEFAULT_SHELL: &str = "/bin/sh";

/// The exit status when the command exists but cannot be executed.
pub(crate) const NOT_EXECUTABLE: i32 = 126;

/// The exit status when the command does not exist.
pub(crate) const NOT_FOUND: i32 = 127;

/// The environment for the command: the caller's, with `LAFT_VIEW` set to
/// `view`.
///
/// An entry with an empty name or a name containing `=` cannot be passed
/// on unambiguously and is dropped. Of several entries with one name, the
/// first is kept, which is the one a lookup in the caller would have found.
pub(crate) fn environment(
    captured: Vec<(OsString, OsString)>,
    view: &str,
) -> Vec<(OsString, OsString)> {
    let mut seen = BTreeSet::new();
    let mut env: Vec<_> = captured
        .into_iter()
        .filter(|(key, _)| !key.is_empty() && !key.as_bytes().contains(&b'='))
        .filter(|(key, _)| key != VIEW_VARIABLE)
        .filter(|(key, _)| seen.insert(key.clone()))
        .collect();
    env.push((VIEW_VARIABLE.into(), view.into()));
    env
}

/// The value of `key` in `env`.
fn lookup<'a>(env: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsStr> {
    env.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_os_str())
}

/// The program to run and its arguments: `command` when it is not empty,
/// otherwise the caller's `SHELL` when that is an absolute path, otherwise
/// `/bin/sh`, without arguments.
pub(crate) fn command_line(
    command: Vec<OsString>,
    env: &[(OsString, OsString)],
) -> (OsString, Vec<OsString>) {
    let mut command = command.into_iter();
    if let Some(program) = command.next() {
        return (program, command.collect());
    }
    let shell = lookup(env, "SHELL")
        .filter(|shell| shell.as_bytes().starts_with(b"/"))
        .unwrap_or(OsStr::new(DEFAULT_SHELL));
    (shell.to_owned(), Vec::new())
}

/// The paths to try for `program`, in order.
///
/// A program that contains a `/` is used as it is. Otherwise every
/// non-empty directory of `PATH` is tried, or `/usr/bin` when `PATH` is
/// unset. An empty program names nothing.
pub(crate) fn candidates(program: &OsStr, env: &[(OsString, OsString)]) -> Vec<PathBuf> {
    if program.is_empty() {
        return Vec::new();
    }
    if program.as_bytes().contains(&b'/') {
        return vec![PathBuf::from(program)];
    }
    let path = lookup(env, "PATH").unwrap_or(OsStr::new(DEFAULT_PATH));
    path.as_bytes()
        .split(|&b| b == b':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(OsStr::from_bytes(dir)).join(program))
        .collect()
}

/// Whether a failed `exec` of one candidate lets the search go on to the
/// next: the file is missing or not permitted there.
pub(crate) fn keep_searching(errno: Errno) -> bool {
    matches!(errno, Errno::NOENT | Errno::NOTDIR | Errno::ACCESS)
}

/// The exit status after every candidate failed with `errors`, in order.
///
/// An error that stopped the search decides alone. Otherwise a candidate
/// that existed but was not permitted wins over the missing ones. Missing
/// files, and no candidate at all, mean the command was not found.
pub(crate) fn exit_status(errors: &[Errno]) -> i32 {
    match errors.last() {
        Some(&last) if !keep_searching(last) => NOT_EXECUTABLE,
        _ if errors.contains(&Errno::ACCESS) => NOT_EXECUTABLE,
        _ => NOT_FOUND,
    }
}

/// The directories to change to, in order: the caller's working directory,
/// then `HOME`, then `/`.
pub(crate) fn directories(cwd: Option<PathBuf>, env: &[(OsString, OsString)]) -> Vec<PathBuf> {
    cwd.into_iter()
        .chain(lookup(env, "HOME").map(PathBuf::from))
        .chain([PathBuf::from("/")])
        .collect()
}

/// Changes to the first directory of [`directories`] that works, restores
/// the caller's `umask` and executes the command with `env`. Returns only
/// when that fails.
pub(crate) fn exec(
    command: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    cwd: Option<PathBuf>,
    umask: Mode,
) -> Failure {
    for dir in directories(cwd, &env) {
        if std::env::set_current_dir(dir).is_ok() {
            break;
        }
    }
    rustix::process::umask(umask);
    let (program, args) = command_line(command, &env);
    let mut errors = Vec::new();
    for path in candidates(&program, &env) {
        let error = std::process::Command::new(&path)
            .arg0(&program)
            .args(&args)
            .env_clear()
            .envs(env.iter().map(|(key, value)| (key, value)))
            .exec();
        let errno = Errno::from_io_error(&error).unwrap_or(Errno::INVAL);
        errors.push(errno);
        if !keep_searching(errno) {
            break;
        }
    }
    let status = exit_status(&errors);
    let reason = match errors.last() {
        _ if status == NOT_FOUND => "not found".to_owned(),
        Some(&last) if !keep_searching(last) => std::io::Error::from(last).to_string(),
        _ => std::io::Error::from(Errno::ACCESS).to_string(),
    };
    Failure::new(status, format!("{}: {reason}", program.to_string_lossy()))
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
