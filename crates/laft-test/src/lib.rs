//! Test helpers shared by the Laft crates.
//!
//! [`tempdir`] creates a temporary directory that is removed with
//! everything in it when it is dropped, even when the test made parts of
//! the tree read-only, as the store does with its entries, generations and
//! views.
//!
//! ```
//! use std::fs;
//! use std::os::unix::fs::PermissionsExt;
//!
//! let dir = laft_test::tempdir();
//! let sealed = dir.path().join("sealed");
//! fs::create_dir(&sealed)?;
//! fs::write(sealed.join("file"), b"content")?;
//! fs::set_permissions(&sealed, fs::Permissions::from_mode(0o555))?;
//! let path = dir.path().to_path_buf();
//! drop(dir);
//! assert!(!path.exists());
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! [`write_program`] writes a stand-in program that a test can execute.

#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// A temporary directory that removes its whole tree when dropped.
///
/// Before removing the tree, it adds the owner write bit to every
/// directory in it, deepest first, without following symbolic links, so
/// that read-only directories do not keep their contents alive.
#[derive(Debug)]
pub struct TempDir(tempfile::TempDir);

/// Creates a new temporary directory in the system's temporary directory.
///
/// # Panics
///
/// Panics when the directory cannot be created, which leaves the test
/// nothing to work in.
pub fn tempdir() -> TempDir {
    TempDir(tempfile::tempdir().expect("a temporary directory can be created"))
}

impl TempDir {
    /// The path of the directory.
    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // A failure here only means that the removal below may fail too,
        // which `tempfile` ignores as it would without this step.
        let _ = make_writable(self.0.path());
    }
}

/// Writes `content` to `path` and makes it an executable program with
/// mode `0755`, replacing any file there.
///
/// A short-lived `/bin/sh` child writes the file, so this process never
/// holds a descriptor that is open for writing to it. A child that another
/// test thread spawns copies every descriptor of this process and keeps
/// the copies until it executes its own program. `O_CLOEXEC` closes them
/// only then. Executing `path` while such a copy is open fails with
/// `ETXTBSY`, so tests that write their programs themselves fail at random
/// when they run in parallel.
///
/// ```
/// let dir = laft_test::tempdir();
/// let program = dir.path().join("hello");
/// laft_test::write_program(&program, b"#!/bin/sh\necho hello\n");
/// let output = std::process::Command::new(&program).output()?;
/// assert_eq!(output.stdout, b"hello\n");
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Panics
///
/// Panics when `/bin/sh` cannot write the file or its mode cannot be set,
/// which leaves the test without its program.
pub fn write_program(path: &Path, content: &[u8]) {
    let mut child = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("/bin/sh runs");
    child
        .stdin
        .take()
        .expect("the input is piped")
        .write_all(content)
        .expect("the shell reads the program");
    let status = child.wait().expect("the shell can be waited for");
    assert!(status.success(), "/bin/sh cannot write {}", path.display());
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("the mode can be set");
}

/// Adds the owner write bit to `dir` and every directory below it,
/// deepest first. Symbolic links are never followed.
///
/// # Errors
///
/// Fails when a directory cannot be read or its mode cannot be changed.
pub fn make_writable(dir: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(dir)?;
    if !metadata.is_dir() {
        return Ok(());
    }
    let mode = metadata.permissions().mode();
    if mode & 0o500 != 0o500 {
        fs::set_permissions(dir, fs::Permissions::from_mode(mode | 0o700))?;
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            make_writable(&entry.path())?;
        }
    }
    let mode = fs::symlink_metadata(dir)?.permissions().mode();
    if mode & 0o200 == 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(mode | 0o200))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
