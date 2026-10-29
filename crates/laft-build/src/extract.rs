//! Placing fetched sources in the source tree, with archives extracted by
//! rules that keep every entry inside its destination.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

use crate::source::{SourceError, io_error};
use crate::{Fetch, Source};

/// The mode bits extraction keeps from an archive.
const EXEC_BITS: u32 = 0o111;

/// How an archive is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    TarLz,
    Zip,
}

impl Format {
    /// The format of a file named `file`.
    fn of(file: &str) -> Option<Self> {
        let formats = [
            (".tar", Self::Tar),
            (".tar.gz", Self::TarGz),
            (".tgz", Self::TarGz),
            (".tar.bz2", Self::TarBz2),
            (".tar.xz", Self::TarXz),
            (".txz", Self::TarXz),
            (".tar.zst", Self::TarZst),
            (".tar.lz", Self::TarLz),
            (".zip", Self::Zip),
        ];
        formats
            .into_iter()
            .find(|(ext, _)| file.ends_with(ext))
            .map(|(_, format)| format)
    }
}

/// Places the fetched file `file` of `source` in the source tree `src`.
///
/// A file that is not extracted is copied to `<src>/<dest>/<file>`. An
/// archive is extracted into `<src>/<dest>`, by these rules:
///
/// 1. Entries with an absolute path or a `..` component are refused.
/// 2. A symbolic link is allowed only when its target, resolved inside
///    the destination, stays inside it, and nothing is ever written
///    through one. A hard link must point to an earlier file of the same
///    archive.
/// 3. Ownership is ignored.
/// 4. Files keep their executable bits and are otherwise `0644`,
///    directories are `0755`.
/// 5. Device nodes and FIFOs are skipped with a warning.
/// 6. `strip_components` leading components are dropped. When the source
///    leaves it unset, one is dropped exactly when the archive holds a
///    single directory at its top.
///
/// The archive is extracted into `scratch` first, which must be on the
/// same filesystem as `src`, then moved into place. The returned strings
/// are warnings about skipped entries.
///
/// # Errors
///
/// Fails with [`SourceError::Unsafe`] for an entry that breaks a rule,
/// with [`SourceError::Unsupported`] for a file that cannot be read as its
/// archive format, and with [`SourceError::Io`] when the tree cannot be
/// written.
pub fn unpack(
    source: &Source,
    file: &Path,
    src: &Path,
    scratch: &Path,
) -> Result<Vec<String>, SourceError> {
    let dest = src.join(&source.dest);
    let (name, extract) = match &source.fetch {
        Fetch::Url { file, extract, .. } => (file.as_str(), *extract),
        Fetch::Git { .. } => ("git.tar", true),
    };
    if !extract {
        fs::create_dir_all(&dest).map_err(io_error(&dest))?;
        let target = dest.join(name);
        fs::copy(file, &target).map_err(io_error(&target))?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644))
            .map_err(io_error(&target))?;
        return Ok(Vec::new());
    }
    let Some(format) = Format::of(name) else {
        return Err(SourceError::Unsupported {
            what: file.display().to_string(),
            reason: format!("{name} is not an archive format that can be extracted"),
        });
    };
    let staging = scratch.join(format!("unpack-{}", source.label));
    remove_tree(&staging)?;
    fs::create_dir_all(&staging).map_err(io_error(&staging))?;
    let strip = source.strip_components.unwrap_or(0) as usize;
    let mut extractor = Extractor {
        archive: file.to_path_buf(),
        root: staging.clone(),
        strip,
        files: BTreeSet::new(),
        lowest: usize::MAX,
        warnings: Vec::new(),
    };
    extractor.extract(format)?;
    let mut top = staging.clone();
    if source.strip_components.is_none() {
        let entries = fs::read_dir(&staging)
            .map_err(io_error(&staging))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io_error(&staging))?;
        if let [only] = entries.as_slice()
            && only.file_type().map_err(io_error(&only.path()))?.is_dir()
        {
            if extractor.lowest == 0 {
                return Err(SourceError::Unsafe {
                    archive: file.to_path_buf(),
                    entry: only.file_name().to_string_lossy().into_owned(),
                    reason: "holds a symbolic link that leads out of it, and it is stripped"
                        .to_owned(),
                });
            }
            top = only.path();
        }
    }
    merge(&top, &dest)?;
    remove_tree(&staging)?;
    Ok(extractor.warnings)
}

/// Writes the entries of one archive below `root`.
struct Extractor {
    archive: PathBuf,
    root: PathBuf,
    strip: usize,
    /// The regular files written so far, which hard links may point to.
    files: BTreeSet<PathBuf>,
    /// The fewest components below the root that resolving any symbolic
    /// link passed through.
    lowest: usize,
    warnings: Vec<String>,
}

impl Extractor {
    fn extract(&mut self, format: Format) -> Result<(), SourceError> {
        let file = File::open(&self.archive).map_err(io_error(&self.archive))?;
        let reader = BufReader::new(file);
        match format {
            Format::Tar => self.tar(reader),
            Format::TarGz => self.tar(flate2::read::MultiGzDecoder::new(reader)),
            Format::TarBz2 => self.tar(bzip2::read::MultiBzDecoder::new(reader)),
            Format::TarXz => self.tar(lzma_rust2::XzReader::new(reader, true)),
            Format::TarZst => {
                let decoder = zstd::stream::read::Decoder::with_buffer(reader)
                    .map_err(io_error(&self.archive))?;
                self.tar(decoder)
            }
            Format::TarLz => self.tar(lzma_rust2::LzipReader::new(reader)),
            Format::Zip => self.zip(),
        }
    }

    fn unreadable(&self, error: impl std::fmt::Display) -> SourceError {
        SourceError::Unsupported {
            what: self.archive.display().to_string(),
            reason: format!("the archive cannot be read: {error}"),
        }
    }

    fn unsafe_entry(&self, entry: &str, reason: impl Into<String>) -> SourceError {
        SourceError::Unsafe {
            archive: self.archive.clone(),
            entry: entry.to_owned(),
            reason: reason.into(),
        }
    }

    fn tar(&mut self, reader: impl Read) -> Result<(), SourceError> {
        let mut archive = tar::Archive::new(reader);
        let entries = archive.entries().map_err(|e| self.unreadable(e))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| self.unreadable(e))?;
            let raw = entry.path().map_err(|e| self.unreadable(e))?;
            let raw = raw.to_string_lossy().into_owned();
            let kind = entry.header().entry_type();
            let mode = entry.header().mode().map_err(|e| self.unreadable(e))?;
            let link = entry
                .link_name()
                .map_err(|e| self.unreadable(e))?
                .map(|l| l.to_string_lossy().into_owned());
            match kind {
                tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => {}
                tar::EntryType::Directory => {
                    if let Some(rel) = self.target(&raw)? {
                        self.dir(&raw, &rel)?;
                    }
                }
                tar::EntryType::Regular
                | tar::EntryType::Continuous
                | tar::EntryType::GNUSparse => {
                    if let Some(rel) = self.target(&raw)? {
                        self.file(&raw, &rel, mode, &mut entry)?;
                    }
                }
                tar::EntryType::Symlink => {
                    if let Some(rel) = self.target(&raw)? {
                        self.symlink(&raw, &rel, link.as_deref().unwrap_or_default())?;
                    }
                }
                tar::EntryType::Link => {
                    if let Some(rel) = self.target(&raw)? {
                        self.hard_link(&raw, &rel, link.as_deref().unwrap_or_default())?;
                    }
                }
                other => self
                    .warnings
                    .push(format!("skipped {raw}, which is a {other:?} entry")),
            }
        }
        Ok(())
    }

    fn zip(&mut self) -> Result<(), SourceError> {
        let file = File::open(&self.archive).map_err(io_error(&self.archive))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| self.unreadable(e))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| self.unreadable(e))?;
            let raw = entry.name().map_err(|e| self.unreadable(e))?.into_owned();
            let mode = entry.unix_mode().unwrap_or(0o644);
            let Some(rel) = self.target(&raw)? else {
                continue;
            };
            if entry.is_dir() {
                self.dir(&raw, &rel)?;
            } else if mode & 0o170_000 == 0o120_000 {
                let mut target = String::new();
                entry
                    .read_to_string(&mut target)
                    .map_err(|e| self.unreadable(e))?;
                self.symlink(&raw, &rel, &target)?;
            } else if mode & 0o170_000 == 0 || mode & 0o170_000 == 0o100_000 {
                self.file(&raw, &rel, mode, &mut entry)?;
            } else {
                self.warnings.push(format!(
                    "skipped {raw}, which is not a file, directory or link"
                ));
            }
        }
        Ok(())
    }

    /// Returns the path of the entry `raw` below the root after stripping,
    /// or `None` for an entry that stripping removes.
    fn target(&self, raw: &str) -> Result<Option<PathBuf>, SourceError> {
        let rel = self.relative(raw)?;
        let parts: Vec<Component<'_>> = rel.components().skip(self.strip).collect();
        Ok((!parts.is_empty()).then(|| parts.iter().collect()))
    }

    /// Checks that `raw` is a relative path without `..` and returns it
    /// without `.` components.
    fn relative(&self, raw: &str) -> Result<PathBuf, SourceError> {
        if raw.starts_with('/') {
            return Err(self.unsafe_entry(raw, "has an absolute path"));
        }
        let mut out = PathBuf::new();
        for part in raw.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(self.unsafe_entry(raw, "escapes the destination")),
                part => out.push(part),
            }
        }
        Ok(out)
    }

    /// Creates the directories above `rel`, refusing to pass through a
    /// symbolic link or a file.
    fn parents(&self, raw: &str, rel: &Path) -> Result<(), SourceError> {
        let mut path = self.root.clone();
        let parents: Vec<_> = rel.components().collect();
        for part in &parents[..parents.len().saturating_sub(1)] {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(m) if m.is_dir() => {}
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(self.unsafe_entry(raw, "would be written through a symbolic link"));
                }
                Ok(_) => return Err(self.unsafe_entry(raw, "is below a file")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&path).map_err(io_error(&path))?;
                    set_mode(&path, 0o755)?;
                }
                Err(e) => return Err(io_error(&path)(e)),
            }
        }
        Ok(())
    }

    /// Removes whatever is at `path` unless it is a directory.
    fn clear(&self, raw: &str, path: &Path) -> Result<(), SourceError> {
        match fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => Err(self.unsafe_entry(raw, "replaces a directory")),
            Ok(_) => fs::remove_file(path).map_err(io_error(path)),
            Err(_) => Ok(()),
        }
    }

    fn dir(&mut self, raw: &str, rel: &Path) -> Result<(), SourceError> {
        self.parents(raw, rel)?;
        let path = self.root.join(rel);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(self.unsafe_entry(raw, "replaces a file with a directory")),
            Err(_) => fs::create_dir(&path).map_err(io_error(&path))?,
        }
        set_mode(&path, 0o755)
    }

    fn file(
        &mut self,
        raw: &str,
        rel: &Path,
        mode: u32,
        content: &mut dyn Read,
    ) -> Result<(), SourceError> {
        self.parents(raw, rel)?;
        let path = self.root.join(rel);
        self.clear(raw, &path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(io_error(&path))?;
        io::copy(content, &mut file).map_err(|e| self.unreadable(e))?;
        set_mode(&path, 0o644 | (mode & EXEC_BITS))?;
        self.files.insert(rel.to_path_buf());
        Ok(())
    }

    fn symlink(&mut self, raw: &str, rel: &Path, target: &str) -> Result<(), SourceError> {
        if target.is_empty() || target.starts_with('/') {
            return Err(
                self.unsafe_entry(raw, format!("links to {target}, outside the destination"))
            );
        }
        // Resolve the target lexically from the link's own directory, as
        // the link would, and require every step to stay inside.
        let mut depth = rel.components().count() - 1;
        for part in target.split('/') {
            match part {
                "" | "." => {}
                ".." if depth == 0 => {
                    return Err(self
                        .unsafe_entry(raw, format!("links to {target}, outside the destination")));
                }
                ".." => {
                    depth -= 1;
                    self.lowest = self.lowest.min(depth);
                }
                _ => depth += 1,
            }
        }
        self.parents(raw, rel)?;
        let path = self.root.join(rel);
        self.clear(raw, &path)?;
        symlink(target, &path).map_err(io_error(&path))
    }

    fn hard_link(&mut self, raw: &str, rel: &Path, target: &str) -> Result<(), SourceError> {
        let earlier = self.target(target)?;
        let Some(earlier) = earlier.filter(|t| self.files.contains(t)) else {
            return Err(self.unsafe_entry(
                raw,
                format!("is a hard link to {target}, which is not an earlier file of the archive"),
            ));
        };
        self.parents(raw, rel)?;
        let path = self.root.join(rel);
        self.clear(raw, &path)?;
        let from = self.root.join(&earlier);
        fs::hard_link(&from, &path).map_err(io_error(&path))?;
        self.files.insert(rel.to_path_buf());
        Ok(())
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), SourceError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(io_error(path))
}

/// Moves everything below `from` into `to`, merging directories that
/// exist in both and replacing anything else.
fn merge(from: &Path, to: &Path) -> Result<(), SourceError> {
    fs::create_dir_all(to).map_err(io_error(to))?;
    for entry in fs::read_dir(from).map_err(io_error(from))? {
        let entry = entry.map_err(io_error(from))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let is_dir = entry.file_type().map_err(io_error(&source))?.is_dir();
        match fs::symlink_metadata(&target) {
            Ok(m) if m.is_dir() && is_dir => {
                merge(&source, &target)?;
                continue;
            }
            Ok(m) if m.is_dir() => remove_tree(&target)?,
            Ok(_) => fs::remove_file(&target).map_err(io_error(&target))?,
            Err(_) => {}
        }
        fs::rename(&source, &target).map_err(io_error(&target))?;
    }
    Ok(())
}

/// Removes the tree at `path`, if any.
fn remove_tree(path: &Path) -> Result<(), SourceError> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_error(path)(e)),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
