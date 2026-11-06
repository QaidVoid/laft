//! Vendor tarballs: dependencies that a language tool downloads, packed
//! reproducibly so that a recipe can pin them by hash.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::os::unix::ffi::OsStrExt;
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
/// for a path too long for them. The pax record is written before the
/// header, whose name field also carries the longest prefix of the path
/// that fits, so a reader without pax support still sees a name. The
/// `.git` and `.jj` directories of the tree are not packed. Entries are
/// sorted bytewise by path, so directories come before their contents.
/// Every entry has the modification time [`SOURCE_DATE_EPOCH`], owner and
/// group 0 and no names, and mode `0o755` for directories and executable
/// files, `0o644` for other files and `0o777` for symlinks. Nothing else
/// of the tree is recorded, so the same tree always gives the same bytes.
///
/// # Errors
///
/// Fails when the tree cannot be read, holds a device, FIFO or socket,
/// holds a symlink whose target is not a plain relative path inside the
/// tree or is too long for the archive's link name field, or when `file`
/// cannot be written.
pub fn normalized_tarball(dir: &Path, file: &Path) -> Result<(), BuildError> {
    let out = File::create(file).map_err(io(file))?;
    let mut builder = tar::Builder::new(BufWriter::new(out));
    for (rel, kind) in fsx::walk(dir)?.into_iter().filter(|(rel, _)| !vcs(rel)) {
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
                // The header names the size of the open handle, so a file
                // replaced between the open and the read never disagrees
                // with the bytes the archive holds.
                let content = File::open(&path).map_err(io(&path))?;
                let metadata = content.metadata().map_err(io(&path))?;
                let exec = metadata.permissions().mode() & 0o111 != 0;
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(if exec { 0o755 } else { 0o644 });
                header.set_size(metadata.len());
                header.set_cksum();
                builder.append(&header, content).map_err(io(file))?;
            }
            Kind::Symlink => {
                let target = fs::read_link(&path).map_err(io(&path))?;
                if plain_rel(&target).is_none() {
                    return Err(refused(
                        file,
                        &rel,
                        format!(
                            "has the symlink target {}, which is not a plain relative path",
                            target.display()
                        ),
                    ));
                }
                if target.as_os_str().as_bytes().len() > 100 {
                    return Err(refused(
                        file,
                        &rel,
                        format!(
                            "has a symlink target {} bytes long, and the archive carries no pax linkpath record",
                            target.as_os_str().as_bytes().len()
                        ),
                    ));
                }
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

/// Whether a path below the packed root enters a version control
/// directory, which never belongs into a reproducible archive.
fn vcs(rel: &str) -> bool {
    rel.split('/').any(|part| part == ".git" || part == ".jj")
}

/// Unpacks the normalized tarball at `archive` into `dest`, which is
/// created. The inverse of [`normalized_tarball`] for a `.tar` file, and
/// the client side of a recipe snapshot: the recipe repository that
/// produced an index generation, unpacked for builds to evaluate.
///
/// Only plain relative paths inside `dest` are written: directories,
/// regular files with their recorded mode, and relative symlinks that
/// stay below `dest`. Entries outside it, absolute paths, device files
/// and other kinds are refused.
///
/// # Errors
///
/// Fails when the archive cannot be read, an entry would leave `dest`, or
/// a file cannot be written.
pub fn unpack_normalized_tarball(archive: &Path, dest: &Path) -> Result<(), BuildError> {
    fs::create_dir_all(dest).map_err(io(dest))?;
    let file = File::open(archive).map_err(io(archive))?;
    let reader = zstd::stream::read::Decoder::new(BufReader::new(file))
        .map_err(|e| BuildError::problem("B024", format!("{}: {e}", archive.display())))?;
    let mut tar = tar::Archive::new(reader);
    let entries = tar.entries().map_err(|e| refused_io(archive, e))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| refused_io(archive, e))?;
        let raw = entry
            .path()
            .map_err(|e| refused_io(archive, e))?
            .into_owned();
        let plain = plain_rel(&raw).ok_or_else(|| {
            refused(
                archive,
                &raw.to_string_lossy(),
                "the path is not a plain relative path",
            )
        })?;
        let target = dest.join(&plain);
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                fs::create_dir_all(&target).map_err(io(&target))?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(io(parent))?;
                }
                // `remove_file` first, so `create` never writes through a
                // symlink planted at the target.
                let _ = fs::remove_file(&target);
                let mut out = File::create(&target).map_err(io(&target))?;
                std::io::copy(&mut entry, &mut out).map_err(io(&target))?;
                out.sync_all().map_err(io(&target))?;
                let mode = entry.header().mode().map_err(|e| refused_io(archive, e))?;
                fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777))
                    .map_err(io(&target))?;
            }
            tar::EntryType::Symlink => {
                let link = entry
                    .link_name()
                    .map_err(|e| refused_io(archive, e))?
                    .unwrap_or_default()
                    .into_owned();
                if plain_rel(Path::new(&link)).is_none() {
                    return Err(refused(
                        archive,
                        &raw.to_string_lossy(),
                        "the symlink target is not a plain relative path",
                    ));
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(io(parent))?;
                }
                std::os::unix::fs::symlink(&link, &target).map_err(io(&target))?;
            }
            other => {
                return Err(refused(
                    archive,
                    &raw.to_string_lossy(),
                    format!("which is a {other:?} entry"),
                ));
            }
        }
    }
    Ok(())
}

/// The plain relative path of `path`, or `None` when it is absolute,
/// holds a component that leaves the root, holds a control character, or
/// is empty.
fn plain_rel(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::Normal(part) => {
                if part.as_bytes().iter().any(|b| *b < 0x20 || *b == 0x7f) {
                    return None;
                }
                out.push(part);
            }
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// A `B024` problem about `archive` that cannot be read.
fn refused_io(archive: &Path, error: impl std::fmt::Display) -> BuildError {
    BuildError::problem(
        "B024",
        format!("{} cannot be read: {error}", archive.display()),
    )
}

/// A `B024` problem about `entry` of `archive`.
fn refused(archive: &Path, entry: &str, reason: impl std::fmt::Display) -> BuildError {
    BuildError::problem(
        "B024",
        format!("{} holds {entry}, {reason}", archive.display()),
    )
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
