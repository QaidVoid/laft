//! The content addressed cache of source archives and vendor tarballs.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use laft_core::Sha256Hash;
use laft_fetch::Fetcher;
use sha2::{Digest, Sha256};

use crate::{Fetch, Recipe, Source};

/// Why a source could not be fetched or placed in the source tree.
#[derive(Debug)]
pub enum SourceError {
    /// `B020`: a URL served a file with another hash. Other URLs are not
    /// tried, because a wrong file at a primary URL is worth stopping for.
    Mismatch {
        /// The source, for example `source "main" of zlib 1.3.1`.
        what: String,
        /// The URL that served the file.
        url: String,
        /// The hash the recipe declares.
        expected: Sha256Hash,
        /// The hash of what arrived.
        actual: Sha256Hash,
    },
    /// `B021`: an archive entry that would land outside the destination,
    /// or a link that leads there.
    Unsafe {
        /// The archive.
        archive: PathBuf,
        /// The entry.
        entry: String,
        /// What is wrong with it.
        reason: String,
    },
    /// `B022`: no URL and no mirror had the file. Every attempt is listed
    /// in order.
    Unavailable {
        /// The source.
        what: String,
        /// The failure of every location.
        attempts: Vec<laft_fetch::Error>,
    },
    /// `B023`: reading or writing a local file failed.
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The underlying error.
        error: io::Error,
    },
    /// `B024`: an archive that cannot be read, or a source this version
    /// cannot fetch.
    Unsupported {
        /// The source or the archive.
        what: String,
        /// Why.
        reason: String,
    },
}

impl SourceError {
    /// The stable code, `B020` to `B024`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Mismatch { .. } => "B020",
            Self::Unsafe { .. } => "B021",
            Self::Unavailable { .. } => "B022",
            Self::Io { .. } => "B023",
            Self::Unsupported { .. } => "B024",
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mismatch {
                what,
                url,
                expected,
                actual,
            } => write!(
                f,
                "{what}: checksum mismatch\n  url:      {url}\n  expected: {expected}\n  actual:   {actual}"
            ),
            Self::Unsafe {
                archive,
                entry,
                reason,
            } => write!(f, "{}: archive entry {entry} {reason}", archive.display()),
            Self::Unavailable { what, attempts } => {
                write!(f, "{what}: no location has the file")?;
                attempts.iter().try_for_each(|a| write!(f, "\n  {a}"))
            }
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Unsupported { what, reason } => write!(f, "{what}: {reason}"),
        }
    }
}

impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Wraps an I/O error from an operation on `path`.
pub(crate) fn io_error(path: &Path) -> impl Fn(io::Error) -> SourceError + '_ {
    move |error| SourceError::Io {
        path: path.to_path_buf(),
        error,
    }
}

/// Source files by their SHA-256 hash, below `sha256/<hex>`.
///
/// A file enters the cache only once its hash is verified, so the cache
/// never holds a file under a wrong name. Partial downloads live in
/// `tmp/` and are removed on every failure.
///
/// ```no_run
/// use laft_build::SourceCache;
/// use laft_fetch::{Config, Fetcher};
///
/// let cache = SourceCache::new(
///     "/var/lib/laft-build/sources",
///     Fetcher::new(Config::new("/var/lib/laft-build/cache")),
/// );
/// let hash = "sha256:9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23".parse()?;
/// let path = cache.fetch(
///     "source \"main\" of zlib 1.3.1",
///     &["https://zlib.net/zlib-1.3.1.tar.gz".to_owned()],
///     &[],
///     &hash,
/// )?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct SourceCache {
    dir: PathBuf,
    fetcher: Fetcher,
}

/// Numbers the temporary files of one process.
static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

impl SourceCache {
    /// A cache in `dir` that downloads with `fetcher`.
    pub fn new(dir: impl Into<PathBuf>, fetcher: Fetcher) -> Self {
        Self {
            dir: dir.into(),
            fetcher,
        }
    }

    /// Where the file with `sha256` is cached.
    pub fn path(&self, sha256: &Sha256Hash) -> PathBuf {
        let hex = sha256.to_string();
        let hex = hex.strip_prefix("sha256:").unwrap_or(&hex);
        self.dir.join("sha256").join(hex)
    }

    /// Makes the file with `sha256` available and returns its path.
    ///
    /// A cached file is used as it is. Otherwise each of `urls` is tried in
    /// order, then `<mirror>/sha256/<hex>` for each of `mirrors`. A
    /// download goes to a temporary file whose hash is compared with
    /// `sha256`. A match is renamed into the cache. `what` names the file
    /// in errors, for example `source "main" of zlib 1.3.1`.
    ///
    /// # Errors
    ///
    /// Fails with [`SourceError::Mismatch`] as soon as one location serves
    /// a file with another hash, which is deleted, with
    /// [`SourceError::Unavailable`] when no location has the file, and
    /// with [`SourceError::Io`] when the cache cannot be written.
    pub fn fetch(
        &self,
        what: &str,
        urls: &[String],
        mirrors: &[String],
        sha256: &Sha256Hash,
    ) -> Result<PathBuf, SourceError> {
        let dest = self.path(sha256);
        if dest.is_file() {
            return Ok(dest);
        }
        let hex = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mirrored = mirrors
            .iter()
            .map(|m| format!("{}/sha256/{hex}", m.trim_end_matches('/')));
        let tmp = self.tmp_path()?;
        let mut attempts = Vec::new();
        for url in urls.iter().cloned().chain(mirrored) {
            if let Err(error) = self.fetcher.download(&url, false, &tmp, &mut |_| {}) {
                remove(&tmp)?;
                attempts.push(error);
                continue;
            }
            let actual = hash_file(&tmp)?;
            if actual != *sha256 {
                remove(&tmp)?;
                return Err(SourceError::Mismatch {
                    what: what.to_owned(),
                    url,
                    expected: *sha256,
                    actual,
                });
            }
            self.publish(&tmp, &dest)?;
            return Ok(dest);
        }
        Err(SourceError::Unavailable {
            what: what.to_owned(),
            attempts,
        })
    }

    /// Makes the file of the URL source `source` of `recipe` available,
    /// trying `mirrors` after its own URLs, and returns its path.
    ///
    /// # Errors
    ///
    /// Fails as [`SourceCache::fetch`] does, and with
    /// [`SourceError::Unsupported`] for a git source, whose tarball this
    /// version cannot produce.
    pub fn fetch_source(
        &self,
        recipe: &Recipe,
        source: &Source,
        mirrors: &[String],
    ) -> Result<PathBuf, SourceError> {
        let what = format!(
            "source \"{}\" of {} {}",
            source.label, recipe.name, recipe.version
        );
        match &source.fetch {
            Fetch::Url { urls, sha256, .. } => self.fetch(&what, urls, mirrors, sha256),
            Fetch::Git { .. } => Err(SourceError::Unsupported {
                what,
                reason: "git sources need a tarball made from the checkout, which this version \
                         cannot produce yet"
                    .to_owned(),
            }),
        }
    }

    /// Adds the local file `file`, for example a vendor tarball the builder
    /// produced, to the cache under its hash, and returns the hash. The
    /// file itself is left in place.
    ///
    /// # Errors
    ///
    /// Fails with [`SourceError::Io`] when the file cannot be read or the
    /// cache cannot be written.
    pub fn insert(&self, file: &Path) -> Result<Sha256Hash, SourceError> {
        let sha256 = hash_file(file)?;
        let dest = self.path(&sha256);
        if !dest.is_file() {
            let tmp = self.tmp_path()?;
            fs::copy(file, &tmp).map_err(io_error(&tmp))?;
            File::open(&tmp)
                .and_then(|f| f.sync_all())
                .map_err(io_error(&tmp))?;
            self.publish(&tmp, &dest)?;
        }
        Ok(sha256)
    }

    /// A new temporary path below `tmp/`.
    fn tmp_path(&self) -> Result<PathBuf, SourceError> {
        let dir = self.dir.join("tmp");
        fs::create_dir_all(&dir).map_err(io_error(&dir))?;
        let n = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
        Ok(dir.join(format!("{}-{n}.part", std::process::id())))
    }

    /// Renames the synced file `tmp` to `dest` and syncs the directory.
    fn publish(&self, tmp: &Path, dest: &Path) -> Result<(), SourceError> {
        let dir = dest.parent().unwrap_or(&self.dir);
        fs::create_dir_all(dir).map_err(io_error(dir))?;
        fs::rename(tmp, dest).map_err(io_error(dest))?;
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(io_error(dir))
    }
}

/// Removes a file that may not exist.
fn remove(path: &Path) -> Result<(), SourceError> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_error(path)(e)),
        _ => Ok(()),
    }
}

/// Returns the SHA-256 hash of the file `path`.
pub(crate) fn hash_file(path: &Path) -> Result<Sha256Hash, SourceError> {
    let mut file = File::open(path).map_err(io_error(path))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 << 10];
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(path)(e)),
        };
        hasher.update(&buf[..n]);
    }
    Ok(Sha256Hash::from_bytes(hasher.finalize().into()))
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
