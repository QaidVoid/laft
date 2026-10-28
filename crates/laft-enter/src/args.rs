//! The command line and the size limits on what the caller passes in.

use std::ffi::{OsStr, OsString};

use laft_core::Name;

/// The usage line printed when the view is missing.
pub(crate) const USAGE: &str = "usage: laft-enter <view> [command [args...]]";

/// The most bytes the arguments and the environment may take together.
pub(crate) const MAX_BYTES: usize = 1 << 20;

/// The most entries the arguments and the environment may have together.
pub(crate) const MAX_ENTRIES: usize = 4096;

/// The view name that names the system itself and can never be entered.
const SYSTEM: &str = "system";

/// What the caller asked for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Invocation {
    /// The view to enter, a valid view name other than `system`.
    pub(crate) view: String,
    /// The command and its arguments. Empty means the default shell.
    pub(crate) command: Vec<OsString>,
}

/// Parses `args`, which start with the program name, as
/// `laft-enter <view> [command [args...]]`.
///
/// There are no options. A view name follows the package name grammar, so
/// it is a single plain path component, and `system` is reserved.
///
/// # Errors
///
/// Returns the message to print when the view is missing, is not a valid
/// name or is `system`.
pub(crate) fn parse(args: &[OsString]) -> Result<Invocation, String> {
    let Some(view) = args.get(1) else {
        return Err(USAGE.to_owned());
    };
    let name = view
        .to_str()
        .and_then(|view| view.parse::<Name>().ok())
        .ok_or_else(|| format!("invalid view name {view:?}"))?;
    if name.as_str() == SYSTEM {
        return Err(format!("the view name {SYSTEM:?} is reserved"));
    }
    Ok(Invocation {
        view: name.as_str().to_owned(),
        command: args[2..].to_vec(),
    })
}

/// Whether the arguments and the environment stay within [`MAX_BYTES`] and
/// [`MAX_ENTRIES`]. Every entry counts with its terminating NUL, and every
/// environment entry with its `=`, as they were laid out by the kernel.
pub(crate) fn within_limits(args: &[OsString], env: &[(OsString, OsString)]) -> bool {
    let entries = args.len().saturating_add(env.len());
    let len = |s: &OsStr| s.len().saturating_add(1);
    let bytes = args
        .iter()
        .map(|arg| len(arg))
        .fold(0, usize::saturating_add);
    let bytes = env
        .iter()
        .map(|(key, value)| len(key).saturating_add(len(value)))
        .fold(bytes, usize::saturating_add);
    entries <= MAX_ENTRIES && bytes <= MAX_BYTES
}

#[cfg(test)]
#[path = "args_tests.rs"]
mod tests;
