//! Placing fetched sources in the source tree, with archives extracted by
//! rules that keep every entry inside its destination.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{AtFlags, FileType, Mode, OFlags, ResolveFlags};
use rustix::io::Errno;

use crate::source::{SourceError, io_error};
use crate::{Fetch, Source};

/// The mode bits extraction keeps from an archive.
const EXEC_BITS: u32 = 0o111;

/// The longest symbolic link target a zip archive may hold, `PATH_MAX`
/// without its terminating NUL.
const LINK_MAX: u64 = 4095;

/// How many symbolic links resolving one path may pass through, the limit
/// the kernel applies.
const MAX_HOPS: usize = 40;

/// Tells concurrent extractions of one process apart in `scratch`.
static UNPACKS: AtomicU64 = AtomicU64::new(0);
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

/// Whether a file named `file` is in an archive format that can be
/// extracted.
pub(crate) fn is_archive(file: &str) -> bool {
    Format::of(file).is_some()
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
/// A file that is not extracted is copied to `<src>/<dest>/<file>`
/// without passing through a symbolic link, replacing a link at the file
/// name. An
/// archive is extracted into `<src>/<dest>`, by these rules:
///
/// 1. Entries with an absolute path or a `..` component are refused.
/// 2. Nothing is ever written through a symbolic link, and every
///    directory is opened beneath the extraction root without following
///    one. A hard link must point to an earlier regular file of the same
///    archive.
/// 3. Every symbolic link must resolve inside the destination, following
///    the links it passes through as the kernel would, once the whole
///    archive is extracted. After the archive is moved into place, every
///    link in `src` must still resolve inside `src`, so links of
///    different sources cannot combine to leave it.
/// 4. Ownership is ignored.
/// 5. Files keep their executable bits and are otherwise `0644`,
///    directories are `0755`.
/// 6. Device nodes and FIFOs are skipped with a warning.
/// 7. `strip_components` leading components are dropped. When the source
///    leaves it unset, one is dropped exactly when the archive holds a
///    single directory at its top.
///
/// The archive is extracted into a new directory in `scratch` first,
/// which must be on the same filesystem as `src`, then moved into place.
/// The source's label is never part of a path. The returned strings are
/// warnings about skipped entries.
///
/// # Errors
///
/// Fails with [`SourceError::Unsafe`] for an entry that breaks a rule,
/// with [`SourceError::Unsupported`] for a file that cannot be read as its
/// archive format, and with [`SourceError::Io`] when the tree cannot be
/// written or when `dest` or the file name is not a plain relative path.
/// After an error the source tree is incomplete and must be discarded.
pub fn unpack(
    source: &Source,
    file: &Path,
    src: &Path,
    scratch: &Path,
) -> Result<Vec<String>, SourceError> {
    let dest = src.join(&source.dest);
    if !is_plain(Path::new(&source.dest)) {
        return Err(not_plain(&dest, "destination"));
    }
    let (name, extract) = match &source.fetch {
        Fetch::Url { file, extract, .. } => (file.as_str(), *extract),
    };
    if !extract {
        let target = dest.join(name);
        if Path::new(name).components().count() != 1 || !is_plain(Path::new(name)) {
            return Err(not_plain(&target, "file name"));
        }
        copy_beneath(file, src, Path::new(&source.dest), OsStr::new(name))?;
        return Ok(Vec::new());
    }
    let Some(format) = Format::of(name) else {
        return Err(SourceError::Unsupported {
            what: file.display().to_string(),
            reason: format!("{name} is not an archive format that can be extracted"),
        });
    };
    let staging = scratch.join(format!(
        "unpack-{}-{}",
        std::process::id(),
        UNPACKS.fetch_add(1, Ordering::Relaxed)
    ));
    remove_tree(&staging)?;
    fs::create_dir(&staging).map_err(io_error(&staging))?;
    let placed = place(source, format, file, &staging, &dest);
    let removed = remove_tree(&staging);
    let warnings = placed?;
    removed?;
    check_links(file, src)?;
    Ok(warnings)
}

/// Copies `file` to `<src>/<dest>/<name>` with mode `0644`, creating the
/// missing directories. No directory on the way may be a symbolic link,
/// and a link at `name` is replaced rather than written through.
fn copy_beneath(file: &Path, src: &Path, dest: &Path, name: &OsStr) -> Result<(), SourceError> {
    let target = src.join(dest).join(name);
    let through_link = || SourceError::Unsafe {
        archive: file.to_path_buf(),
        entry: target.display().to_string(),
        reason: "would be written through a symbolic link".to_owned(),
    };
    let mut dir = rustix::fs::open(
        src,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno(src))?;
    for part in dest {
        dir = match make_dir(&dir, part) {
            Ok(fd) => fd,
            Err(Errno::LOOP | Errno::NOTDIR | Errno::EXIST) => return Err(through_link()),
            Err(e) => return Err(errno(&target)(e)),
        };
    }
    match rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
            return Err(SourceError::Unsafe {
                archive: file.to_path_buf(),
                entry: target.display().to_string(),
                reason: "replaces a directory".to_owned(),
            });
        }
        Ok(_) => rustix::fs::unlinkat(&dir, name, AtFlags::empty()).map_err(errno(&target))?,
        Err(Errno::NOENT) => {}
        Err(e) => return Err(errno(&target)(e)),
    }
    let fd = rustix::fs::openat2(
        &dir,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
        beneath(),
    )
    .map_err(errno(&target))?;
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o644)).map_err(errno(&target))?;
    let mut from = fs::File::open(file).map_err(io_error(file))?;
    io::copy(&mut from, &mut fs::File::from(fd)).map_err(io_error(&target))?;
    Ok(())
}

/// Extracts `file` into `staging`, checks its links and moves its tree
/// into `dest`.
fn place(
    source: &Source,
    format: Format,
    file: &Path,
    staging: &Path,
    dest: &Path,
) -> Result<Vec<String>, SourceError> {
    let root = rustix::fs::open(
        staging,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno(staging))?;
    let mut extractor = Extractor {
        archive: file.to_path_buf(),
        root,
        root_path: staging.to_path_buf(),
        strip: source.strip_components.unwrap_or(0) as usize,
        files: BTreeSet::new(),
        warnings: Vec::new(),
    };
    extractor.extract(format)?;
    let mut top = staging.to_path_buf();
    if source.strip_components.is_none() {
        let entries = fs::read_dir(staging)
            .map_err(io_error(staging))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io_error(staging))?;
        if let [only] = entries.as_slice()
            && only.file_type().map_err(io_error(&only.path()))?.is_dir()
        {
            top = only.path();
        }
    }
    check_links(file, &top)?;
    merge(&top, dest)?;
    Ok(extractor.warnings)
}

/// Whether `path` is empty or relative with only normal components.
fn is_plain(path: &Path) -> bool {
    path.components().all(|c| matches!(c, Component::Normal(_)))
}

/// The error for a `what` that would place a source outside the tree.
fn not_plain(path: &Path, what: &str) -> SourceError {
    SourceError::Io {
        path: path.to_path_buf(),
        error: io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the {what} is not a plain relative path"),
        ),
    }
}

/// Wraps an error of a system call on `path`.
fn errno(path: &Path) -> impl Fn(Errno) -> SourceError + '_ {
    move |e| io_error(path)(e.into())
}

/// The flags for opening anything below the extraction root: never above
/// it and never through a symbolic link.
fn beneath() -> ResolveFlags {
    ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS
}

/// Opens the directory `name` in `dir`, creating it when it is missing,
/// and gives it mode `0755`. Fails with `ELOOP` or `ENOTDIR` for anything
/// that is not a directory, symbolic links included.
fn make_dir(dir: &OwnedFd, name: &OsStr) -> rustix::io::Result<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = match rustix::fs::openat2(dir, name, flags, Mode::empty(), beneath()) {
        Err(Errno::NOENT) => {
            rustix::fs::mkdirat(dir, name, Mode::from_raw_mode(0o755))?;
            rustix::fs::openat2(dir, name, flags, Mode::empty(), beneath())?
        }
        other => other?,
    };
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o755))?;
    Ok(fd)
}

/// Whether `name` in `dir` is a symbolic link.
fn is_link(dir: &OwnedFd, name: &OsStr) -> bool {
    rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
}

/// Writes the entries of one archive below `root`.
struct Extractor {
    archive: PathBuf,
    /// The extraction root, which every entry is opened beneath.
    root: OwnedFd,
    root_path: PathBuf,
    strip: usize,
    /// The regular files written so far that are still in place, which
    /// hard links may point to.
    files: BTreeSet<PathBuf>,
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

    fn io(&self, rel: &Path) -> impl Fn(Errno) -> SourceError + use<> {
        let path = self.root_path.join(rel);
        move |e| io_error(&path)(e.into())
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
            let mtime = entry.header().mtime().ok();
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
                        self.file(&raw, &rel, mode, mtime, &mut entry)?;
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
                (&mut entry)
                    .take(LINK_MAX + 1)
                    .read_to_string(&mut target)
                    .map_err(|e| self.unreadable(e))?;
                if target.len() as u64 > LINK_MAX {
                    return Err(self.unreadable(format!(
                        "{raw} is a symbolic link with a target longer than {LINK_MAX} bytes"
                    )));
                }
                self.symlink(&raw, &rel, &target)?;
            } else if mode & 0o170_000 == 0 || mode & 0o170_000 == 0o100_000 {
                let mtime = entry.last_modified().and_then(zip_time);
                self.file(&raw, &rel, mode, mtime, &mut entry)?;
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

    /// Opens the directory that holds `rel`, creating the missing ones and
    /// refusing to pass through a symbolic link or a file, and returns it
    /// with the last component of `rel`.
    fn parent<'r>(&self, raw: &str, rel: &'r Path) -> Result<(OwnedFd, &'r OsStr), SourceError> {
        let mut parts: Vec<&OsStr> = rel.iter().collect();
        let name = parts
            .pop()
            .expect("an entry that survives stripping has a component");
        let mut dir = rustix::fs::openat2(
            &self.root,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            beneath(),
        )
        .map_err(self.io(Path::new("")))?;
        let mut path = PathBuf::new();
        for part in parts {
            path.push(part);
            dir = match make_dir(&dir, part) {
                Ok(fd) => fd,
                Err(Errno::LOOP | Errno::NOTDIR | Errno::EXIST) if is_link(&dir, part) => {
                    return Err(self.unsafe_entry(raw, "would be written through a symbolic link"));
                }
                Err(Errno::LOOP | Errno::NOTDIR | Errno::EXIST) => {
                    return Err(self.unsafe_entry(raw, "is below a file"));
                }
                Err(e) => return Err(self.io(&path)(e)),
            };
        }
        Ok((dir, name))
    }

    /// Removes whatever is at `name` in `dir` unless it is a directory.
    fn clear(
        &mut self,
        raw: &str,
        dir: &OwnedFd,
        name: &OsStr,
        rel: &Path,
    ) -> Result<(), SourceError> {
        match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
                Err(self.unsafe_entry(raw, "replaces a directory"))
            }
            Ok(_) => {
                rustix::fs::unlinkat(dir, name, AtFlags::empty()).map_err(self.io(rel))?;
                self.files.remove(rel);
                Ok(())
            }
            Err(Errno::NOENT) => Ok(()),
            Err(e) => Err(self.io(rel)(e)),
        }
    }

    fn dir(&mut self, raw: &str, rel: &Path) -> Result<(), SourceError> {
        let (parent, name) = self.parent(raw, rel)?;
        match make_dir(&parent, name) {
            Ok(_) => Ok(()),
            Err(Errno::LOOP | Errno::NOTDIR | Errno::EXIST) => {
                Err(self.unsafe_entry(raw, "replaces a file with a directory"))
            }
            Err(e) => Err(self.io(rel)(e)),
        }
    }

    /// Writes a regular file with the executable bits of `mode` and, when
    /// the archive records one, its modification time. Build systems such as
    /// make compare those times, so a generated file must stay newer than
    /// its inputs, as it is in the archive.
    fn file(
        &mut self,
        raw: &str,
        rel: &Path,
        mode: u32,
        mtime: Option<u64>,
        content: &mut dyn Read,
    ) -> Result<(), SourceError> {
        let (dir, name) = self.parent(raw, rel)?;
        self.clear(raw, &dir, name, rel)?;
        let fd = rustix::fs::openat2(
            &dir,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
            beneath(),
        )
        .map_err(self.io(rel))?;
        let mut file = File::from(fd);
        io::copy(content, &mut file).map_err(|e| self.unreadable(e))?;
        rustix::fs::fchmod(&file, Mode::from_raw_mode(0o644 | (mode & EXEC_BITS)))
            .map_err(self.io(rel))?;
        if let Some(seconds) = mtime.and_then(|s| i64::try_from(s).ok()) {
            let time = rustix::fs::Timespec {
                tv_sec: seconds,
                tv_nsec: 0,
            };
            let times = rustix::fs::Timestamps {
                last_access: time,
                last_modification: time,
            };
            rustix::fs::futimens(&file, &times).map_err(self.io(rel))?;
        }
        self.files.insert(rel.to_path_buf());
        Ok(())
    }

    /// Creates a symbolic link. Where it leads is checked once the whole
    /// archive is extracted, because a later link can change it.
    fn symlink(&mut self, raw: &str, rel: &Path, target: &str) -> Result<(), SourceError> {
        if target.is_empty() || target.starts_with('/') {
            return Err(
                self.unsafe_entry(raw, format!("links to {target}, outside the destination"))
            );
        }
        let (dir, name) = self.parent(raw, rel)?;
        self.clear(raw, &dir, name, rel)?;
        rustix::fs::symlinkat(target, &dir, name).map_err(self.io(rel))
    }

    fn hard_link(&mut self, raw: &str, rel: &Path, target: &str) -> Result<(), SourceError> {
        let earlier = self.target(target)?;
        let (dir, name) = self.parent(raw, rel)?;
        self.clear(raw, &dir, name, rel)?;
        let Some(earlier) = earlier.filter(|t| self.files.contains(t)) else {
            return Err(self.unsafe_entry(
                raw,
                format!("is a hard link to {target}, which is not an earlier file of the archive"),
            ));
        };
        let (from, from_name) = self.parent(raw, &earlier)?;
        rustix::fs::linkat(&from, from_name, &dir, name, AtFlags::empty()).map_err(self.io(rel))?;
        self.files.insert(rel.to_path_buf());
        Ok(())
    }
}

/// Checks that every symbolic link below `root` resolves inside it.
fn check_links(archive: &Path, root: &Path) -> Result<(), SourceError> {
    let mut todo = vec![PathBuf::new()];
    while let Some(rel) = todo.pop() {
        let dir = root.join(&rel);
        for entry in fs::read_dir(&dir).map_err(io_error(&dir))? {
            let entry = entry.map_err(io_error(&dir))?;
            let kind = entry.file_type().map_err(io_error(&entry.path()))?;
            let rel = rel.join(entry.file_name());
            if kind.is_dir() {
                todo.push(rel);
            } else if kind.is_symlink()
                && let Some(reason) = escape(root, &rel).map_err(io_error(&root.join(&rel)))?
            {
                return Err(SourceError::Unsafe {
                    archive: archive.to_path_buf(),
                    entry: rel.display().to_string(),
                    reason,
                });
            }
        }
    }
    Ok(())
}

/// Resolves the symbolic link `link` below `root` one component at a time,
/// following the links it passes through, and returns why it leaves
/// `root`, or `None` when it stays inside.
///
/// A component that does not exist, or that a file stands in for, is taken
/// as a directory, so the link stays inside if one is created there later.
fn escape(root: &Path, link: &Path) -> io::Result<Option<String>> {
    let target = fs::read_link(root.join(link))?;
    let outside = || {
        Some(format!(
            "links to {}, outside the destination",
            target.display()
        ))
    };
    let mut at: Vec<OsString> = link
        .parent()
        .into_iter()
        .flat_map(Path::iter)
        .map(OsStr::to_os_string)
        .collect();
    // The components still to resolve, the next one last. `..` stands for
    // a parent directory component, which a normal one can never be.
    let mut todo = Vec::new();
    if !push_target(&mut todo, &target) {
        return Ok(outside());
    }
    let mut hops = 1;
    while let Some(part) = todo.pop() {
        if part == ".." {
            if at.pop().is_none() {
                return Ok(outside());
            }
            continue;
        }
        at.push(part);
        let path: PathBuf = root.join(at.iter().collect::<PathBuf>());
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() {
            hops += 1;
            if hops > MAX_HOPS {
                return Ok(Some(format!(
                    "links to {}, which does not resolve through {MAX_HOPS} links",
                    target.display()
                )));
            }
            at.pop();
            if !push_target(&mut todo, &fs::read_link(&path)?) {
                return Ok(outside());
            }
        }
    }
    Ok(None)
}

/// Pushes the components of the link target `target` onto `todo`, the
/// first one last. Returns `false` for an absolute target.
fn push_target(todo: &mut Vec<OsString>, target: &Path) -> bool {
    for part in target.components().rev() {
        match part {
            Component::Normal(name) => todo.push(name.to_os_string()),
            Component::ParentDir => todo.push("..".into()),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
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

/// The seconds since the Unix epoch of the time of a zip entry, which the
/// format records without a time zone, read as UTC.
fn zip_time(time: zip::DateTime) -> Option<u64> {
    let text = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        time.year(),
        time.month(),
        time.day(),
        time.hour(),
        time.minute(),
        time.second()
    );
    text.parse::<laft_core::Timestamp>()
        .ok()
        .map(laft_core::Timestamp::as_secs)
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
