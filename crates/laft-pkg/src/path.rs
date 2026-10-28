//! Rules for paths inside a package payload.

/// The longest payload path, in bytes.
const MAX_LEN: usize = 4095;

/// Directories that a payload never contains.
const FORBIDDEN: [&str; 3] = ["usr/sbin", "usr/lib64", "usr/local"];

/// Checks that `path` is a payload path, and says why not otherwise.
///
/// A payload path is relative, uses `/` as separator, has no empty, `.` or
/// `..` components and no NUL byte, is at most 4095 bytes long, lies under
/// `usr` or `etc`, and is not `usr/sbin`, `usr/lib64` or `usr/local` or under
/// one of them.
pub(crate) fn check(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("a path cannot be empty");
    }
    if path.len() > MAX_LEN {
        return Err("a path is at most 4095 bytes long");
    }
    if path.contains('\0') {
        return Err("a path cannot contain a NUL byte");
    }
    if path.starts_with('/') {
        return Err("a path must be relative");
    }
    if path
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err("a path has no empty, `.` or `..` components and no trailing `/`");
    }
    if !matches!(path.split('/').next(), Some("usr" | "etc")) {
        return Err("a payload path lies under `usr` or `etc`");
    }
    if FORBIDDEN.iter().any(|f| {
        path.strip_prefix(f)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }) {
        return Err("`usr/sbin`, `usr/lib64` and `usr/local` are not allowed");
    }
    Ok(())
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
