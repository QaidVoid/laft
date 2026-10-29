//! Reading the configuration files and the std catalog.

use std::borrow::Cow;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use stav::{ImportRoot, LoadError, Loader, MAX_SOURCE_LEN, RelPath};

/// The directory below the configuration directory that holds secrets,
/// which the configuration never reads.
const SECRETS: &str = "secrets";

/// Serves the std catalog from the binary and every other file from the
/// configuration directory.
#[derive(Debug, Clone)]
pub(crate) struct DiskLoader {
    root: PathBuf,
}

impl DiskLoader {
    /// Returns a loader for the configuration directory `root`.
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }
}

impl Loader for DiskLoader {
    fn std_module(&self, name: &str) -> Option<Cow<'static, str>> {
        match name {
            "services" => Some(Cow::Borrowed(include_str!("../stav/std/services.stav"))),
            "desktop" => Some(Cow::Borrowed(include_str!("../stav/std/desktop.stav"))),
            _ => None,
        }
    }

    /// Reads `rel` below `root`, which must be the configuration directory.
    /// Symbolic links are resolved, and the real path must stay inside the
    /// real configuration directory and outside its `secrets` directory.
    fn read(&self, root: &ImportRoot, rel: &RelPath) -> Result<String, LoadError> {
        if root.path() != self.root {
            return Err(LoadError::OutsideRoot);
        }
        let real_root = fs::canonicalize(&self.root).map_err(error)?;
        let path = fs::canonicalize(self.root.join(rel.to_path())).map_err(error)?;
        let inside = path
            .strip_prefix(&real_root)
            .map_err(|_| LoadError::OutsideRoot)?;
        if inside.starts_with(SECRETS) {
            return Err(LoadError::Other(format!(
                "files in {SECRETS}/ are never read by the configuration"
            )));
        }
        // Checked before opening, because opening a FIFO would block.
        if !fs::metadata(&path).map_err(error)?.is_file() {
            return Err(LoadError::NotAFile);
        }
        let file = fs::File::open(&path).map_err(error)?;
        if !file.metadata().map_err(error)?.is_file() {
            return Err(LoadError::NotAFile);
        }
        let mut bytes = Vec::new();
        file.take(MAX_SOURCE_LEN as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        if bytes.len() > MAX_SOURCE_LEN {
            return Err(LoadError::TooLarge);
        }
        String::from_utf8(bytes).map_err(|_| LoadError::Other("the file is not UTF-8 text".into()))
    }
}

fn error(e: io::Error) -> LoadError {
    match e.kind() {
        io::ErrorKind::NotFound => LoadError::NotFound,
        _ => LoadError::Other(e.to_string()),
    }
}

#[cfg(test)]
#[path = "loader_tests.rs"]
mod tests;
