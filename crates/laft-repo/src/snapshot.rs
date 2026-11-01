//! The recipe snapshot of an index generation: the recipe repository it
//! was built from, as one reproducible `recipes/<blake3>.tar.zst`.

use std::path::Path;

use laft_index::RecipeSnapshot;

use crate::error::{Error, Result};
use crate::fsx;
use crate::layout::Repository;

/// The zstd level the snapshot is compressed with. A fixed level keeps the
/// archive reproducible.
const LEVEL: i32 = 19;

/// Packs the recipe repository at `source` into `repo` and returns the
/// `recipes` member naming it.
///
/// The tree is packed as a normalized tarball: entries are sorted by path
/// and carry no owner names, no times but [`laft_build::SOURCE_DATE_EPOCH`]
/// and no modes but the directory and executable bits. The same tree
/// therefore always gives the same archive, and two generations built from
/// it share one file. An existing file is kept, because published files
/// never change.
///
/// # Errors
///
/// Fails when the tree cannot be packed or the archive cannot be written.
pub(crate) fn write(repo: &Repository, source: &Path) -> Result<RecipeSnapshot> {
    let tar = repo.path("recipes/.packing.tar");
    if let Some(parent) = tar.parent() {
        std::fs::create_dir_all(parent).map_err(crate::error::io(&tar))?;
    }
    if fsx::size(&tar)?.is_some() {
        fsx::remove(&tar)?;
    }
    laft_build::normalized_tarball(source, &tar)
        .map_err(|e| Error::new(format!("{}: {e}", source.display())))?;
    let bytes = pack(&tar)?;
    fsx::remove(&tar)?;
    let blake3 = laft_core::Blake3Hash::of(&bytes);
    let file = format!("recipes/{blake3}.tar.zst");
    let path = repo.path(&file);
    if fsx::size(&path)?.is_none() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(crate::error::io(&path))?;
        }
        fsx::write_atomic(&path, &bytes, fsx::FILE_MODE)?;
    }
    Ok(RecipeSnapshot {
        file,
        size: bytes.len() as u64,
        blake3,
    })
}

/// The zstd frame of the tarball at `path`, compressed with a fixed level
/// and checksum.
fn pack(path: &Path) -> Result<Vec<u8>> {
    let tar = std::fs::read(path).map_err(crate::error::io(path))?;
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), LEVEL)
        .and_then(|mut e| e.include_checksum(true).map(|()| e))
        .map_err(crate::error::io(path))?;
    std::io::Write::write_all(&mut encoder, &tar).map_err(crate::error::io(path))?;
    encoder.finish().map_err(crate::error::io(path))
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
