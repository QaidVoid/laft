//! Vendor tarballs: dependencies that a language tool downloads, packed
//! reproducibly so that a recipe can pin them by hash.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::BufWriter;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::{BuildError, io};
use crate::exec::Executor;
use crate::fsx::{self, Kind};
use crate::source::hash_file;
use crate::{SOURCE_DATE_EPOCH, SourceCache, SourceError, Vendor, VendorKind};

/// Writes the tree `dir` as a normalized tarball to `file`.
///
/// Entries use the POSIX formats: ustar headers, with a pax `path` record
/// for a path too long for them. They are sorted bytewise by path, so
/// directories come before their contents. Every entry has the modification
/// time [`SOURCE_DATE_EPOCH`], owner and group 0 and no names, and mode
/// `0o755` for directories and executable files, `0o644` for other files
/// and `0o777` for symlinks. Nothing else of the tree is recorded, so the
/// same tree always gives the same bytes.
///
/// # Errors
///
/// Fails when the tree cannot be read, holds a device, FIFO or socket, or
/// when `file` cannot be written.
pub fn normalized_tarball(dir: &Path, file: &Path) -> Result<(), BuildError> {
    let out = File::create(file).map_err(io(file))?;
    let mut builder = tar::Builder::new(BufWriter::new(out));
    for (rel, kind) in fsx::walk(dir)? {
        let path = dir.join(&rel);
        let mut header = tar::Header::new_ustar();
        header.set_mtime(SOURCE_DATE_EPOCH);
        header.set_uid(0);
        header.set_gid(0);
        let name = match kind {
            Kind::Dir => format!("{rel}/"),
            _ => rel.clone(),
        };
        if header.set_path(&name).is_err() {
            builder
                .append_pax_extensions([("path", name.as_bytes())])
                .map_err(io(file))?;
            let field = &mut header.as_old_mut().name;
            let n = name.len().min(field.len());
            field.fill(0);
            field[..n].copy_from_slice(&name.as_bytes()[..n]);
        }
        match kind {
            Kind::Dir => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
                header.set_cksum();
                builder
                    .append(&header, std::io::empty())
                    .map_err(io(file))?;
            }
            Kind::File => {
                let metadata = fs::metadata(&path).map_err(io(&path))?;
                let exec = metadata.permissions().mode() & 0o111 != 0;
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(if exec { 0o755 } else { 0o644 });
                header.set_size(metadata.len());
                header.set_cksum();
                let content = File::open(&path).map_err(io(&path))?;
                builder.append(&header, content).map_err(io(file))?;
            }
            Kind::Symlink => {
                let target = fs::read_link(&path).map_err(io(&path))?;
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                header.set_link_name(&target).map_err(io(&path))?;
                header.set_cksum();
                builder
                    .append(&header, std::io::empty())
                    .map_err(io(file))?;
            }
            Kind::Other => {
                return Err(BuildError::problem(
                    "B024",
                    format!("{} is a device, FIFO or socket", path.display()),
                ));
            }
        }
    }
    let writer = builder.into_inner().map_err(io(file))?;
    writer
        .into_inner()
        .map_err(|e| io(file)(e.into_error()))?
        .sync_all()
        .map_err(io(file))
}

/// The command that vendors the dependencies of `vendor` into
/// `/out/vendor`.
fn command(vendor: &Vendor) -> String {
    let dir = crate::style::quote(&vendor.dir);
    match vendor.kind {
        VendorKind::Cargo => format!(
            "cd \"$SRC\"/{dir}\nexport CARGO_HOME=/build/home/cargo\n\
             cargo vendor --locked --versioned-dirs /out/vendor >/dev/null"
        ),
        VendorKind::Go => format!(
            "cd \"$SRC\"/{dir}\nexport GOPATH=/build/home/go GOMODCACHE=/build/home/go/mod\n\
             go mod vendor -o /out/vendor"
        ),
    }
}

/// Produces the vendor tarball of `vendor` for the package `what` and adds
/// it to `cache`, unless the cache has it already. Returns its path.
///
/// The vendoring tool runs through `exec` with the host's network, in the
/// source tree that is already unpacked, and writes into `out/vendor`,
/// where `out` is the host directory that `exec` shows as `/out`. The tree
/// is packed with [`normalized_tarball`], and the tarball enters the cache
/// only when its hash is the one the recipe declares.
///
/// # Errors
///
/// Fails with `B020` when the tarball has another hash, with `B040` when
/// the tool fails, and when files cannot be written.
pub(crate) fn vendor(
    what: &str,
    vendor: &Vendor,
    exec: &Executor,
    env: &BTreeMap<String, String>,
    out: &Path,
    cache: &SourceCache,
) -> Result<PathBuf, BuildError> {
    let cached = cache.path(&vendor.sha256);
    if cached.is_file() {
        return Ok(cached);
    }
    let tree = out.join("vendor");
    fsx::remove_tree(&tree)?;
    exec.fetch(
        &format!("vendor-{}", vendor.kind.as_str()),
        &command(vendor),
        env,
    )?;
    let tarball = out.join(format!("vendor-{}.tar", vendor.kind.as_str()));
    normalized_tarball(&tree, &tarball)?;
    fsx::remove_tree(&tree)?;
    let actual = hash_file(&tarball)?;
    if actual != vendor.sha256 {
        fs::remove_file(&tarball).map_err(io(&tarball))?;
        return Err(SourceError::Mismatch {
            what: format!("{} vendor tarball of {what}", vendor.kind.as_str()),
            url: format!("{} vendor", vendor.kind.as_str()),
            expected: vendor.sha256,
            actual,
        }
        .into());
    }
    cache.insert(&tarball)?;
    fs::remove_file(&tarball).map_err(io(&tarball))?;
    Ok(cached)
}

#[cfg(test)]
#[path = "vendor_tests.rs"]
mod tests;
