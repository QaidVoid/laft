//! Reading recipe files and the std catalog.

use std::borrow::Cow;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use stav::{ImportRoot, LoadError, Loader, MAX_SOURCE_LEN, RelPath};

/// The directory of a recipe repository that shared modules live in.
pub(crate) const COMMON: &str = "common";

/// The directory of a recipe repository that recipes live in.
pub(crate) const RECIPES: &str = "recipes";

/// The file name of a recipe.
pub(crate) const RECIPE_FILE: &str = "package.stav";

/// The file name of a repository descriptor.
pub(crate) const DESCRIPTOR: &str = "recipes.stav";

/// Returns the std module `std/<name>`.
pub(crate) fn std_module(name: &str) -> Option<&'static str> {
    match name {
        "build" => Some(include_str!("../stav/std/build.stav")),
        _ => None,
    }
}

/// Which files of a repository a root may read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The descriptor `recipes.stav` only.
    Descriptor,
    /// The recipe of one package and the shared modules below `common/`.
    Package(String),
}

/// Serves the std catalog from the binary and files from recipe
/// repositories.
#[derive(Debug, Clone)]
pub(crate) struct RepoLoader {
    roots: Vec<PathBuf>,
    scope: Scope,
}

impl RepoLoader {
    /// Returns a loader for the repositories `roots` that reads only what
    /// `scope` allows.
    pub(crate) fn new(roots: Vec<PathBuf>, scope: Scope) -> Self {
        Self { roots, scope }
    }

    fn allowed(&self, rel: &RelPath) -> bool {
        let parts: Vec<&str> = rel.as_str().split('/').collect();
        match (&self.scope, parts.as_slice()) {
            (Scope::Descriptor, [file]) => *file == DESCRIPTOR,
            (Scope::Package(name), [RECIPES, dir, RECIPE_FILE]) => dir == name,
            (Scope::Package(_), [COMMON, _, ..]) => true,
            _ => false,
        }
    }
}

impl Loader for RepoLoader {
    fn std_module(&self, name: &str) -> Option<Cow<'static, str>> {
        std_module(name).map(Cow::Borrowed)
    }

    /// Reads `rel` below one of the repositories. Symbolic links are
    /// resolved, and the real path must stay inside the real repository.
    fn read(&self, root: &ImportRoot, rel: &RelPath) -> Result<String, LoadError> {
        if !self.roots.iter().any(|r| r == root.path()) {
            return Err(LoadError::OutsideRoot);
        }
        if !self.allowed(rel) {
            return Err(LoadError::Other(match self.scope {
                Scope::Descriptor => format!("a repository descriptor is {DESCRIPTOR} alone"),
                Scope::Package(_) => {
                    format!("a recipe can only import modules below {COMMON}/")
                }
            }));
        }
        read_inside(root.path(), &root.path().join(rel.to_path()))
    }
}

/// Reads the file `path`, whose real path must lie inside the real
/// directory `root`.
fn read_inside(root: &Path, path: &Path) -> Result<String, LoadError> {
    let real_root = fs::canonicalize(root).map_err(error)?;
    let real = fs::canonicalize(path).map_err(error)?;
    if !real.starts_with(&real_root) {
        return Err(LoadError::OutsideRoot);
    }
    // Checked before opening, because opening a FIFO would block.
    if !fs::metadata(&real).map_err(error)?.is_file() {
        return Err(LoadError::NotAFile);
    }
    let file = fs::File::open(&real).map_err(error)?;
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_LEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > MAX_SOURCE_LEN {
        return Err(LoadError::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| LoadError::Other("the file is not UTF-8 text".into()))
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
