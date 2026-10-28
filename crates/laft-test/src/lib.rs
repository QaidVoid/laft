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

#![forbid(unsafe_code)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

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
