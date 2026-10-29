//! Seeds: packages that another build root made, added to the build-side
//! store so that builds can use them before this root has built its own.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use laft_core::StoreId;

use crate::error::{BuildError, io};
use crate::{BuildRoot, fsx};

/// Adds the package file `lpkg` to the build-side store of `root` as a
/// seed and returns the directory name of its store entry.
///
/// A seed serves build views like any package of the store, until a build
/// in `root` adds a package with the same origin and variant, which
/// replaces every seed of that origin and variant. That way the packages
/// of a bootstrap stand in for the packages of a build run until the run
/// has built its own. A seed that a build in `root` makes again, with the
/// same store id, counts as built from then on, and a package that is in
/// the store already stays what it is.
///
/// The seed is recorded in `root`'s `seeds` directory as an empty file
/// named by its store id, before the package enters the store, so that an
/// interrupted call never leaves a seed that counts as built.
///
/// # Errors
///
/// Fails with [`BuildError::Io`] when `lpkg` cannot be opened or the seed
/// record cannot be written, with [`BuildError::Package`] when `lpkg` is not
/// a valid package, and with [`BuildError::Store`] when the store cannot be
/// listed or the package cannot enter it.
pub fn seed(root: &BuildRoot, lpkg: &Path) -> Result<String, BuildError> {
    let file = fs::File::open(lpkg).map_err(io(lpkg))?;
    let id = laft_pkg::Reader::new(io::BufReader::new(file))?
        .manifest()
        .store_id();
    let store = root.store();
    let prefix = format!("{id}-");
    if !store.list()?.iter().any(|d| d.starts_with(&prefix)) {
        fsx::write_atomic(&root.seeds().join(id.to_string()), b"", 0o644)?;
    }
    Ok(store.insert(lpkg, &id, &root.tmp())?)
}

/// The store ids of the seeds recorded in the directory `dir`, empty when
/// it does not exist. Names that are not store ids are ignored, so that a
/// staging file of an interrupted write never counts.
///
/// # Errors
///
/// Fails when the directory exists but cannot be read.
pub(crate) fn seeds(dir: &Path) -> Result<BTreeSet<StoreId>, BuildError> {
    let items = match fs::read_dir(dir) {
        Ok(items) => items,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(io(dir)(e)),
    };
    let mut ids = BTreeSet::new();
    for item in items {
        let name = item.map_err(io(dir))?.file_name();
        if let Some(id) = name.to_str().and_then(|n| n.parse().ok()) {
            ids.insert(id);
        }
    }
    Ok(ids)
}

/// Forgets the seed `id` recorded in `dir`, because a build made the same
/// package. A seed that is not recorded is fine.
///
/// # Errors
///
/// Fails when the record cannot be removed.
pub(crate) fn forget(dir: &Path, id: &StoreId) -> Result<(), BuildError> {
    let path = dir.join(id.to_string());
    match fs::remove_file(&path) {
        Ok(()) => fs::File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(io(dir)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(&path)(e)),
    }
}

#[cfg(test)]
#[path = "seed_tests.rs"]
mod tests;
