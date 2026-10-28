use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use laft_core::Blake3Hash;
use laft_index::{IndexEntry, MAX_INDEX_SIZE};
use ureq::Agent;
use ureq::http::{Response, StatusCode, header};

use crate::error::io;
use crate::fs::{FILE_MODE, create_dir, part_path, publish, remove};
use crate::url::check_path;
use crate::{BaseUrl, Error, ErrorKind, Mirrors, Scheme};

/// How a [`Fetcher`] reaches repositories and where it caches files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The cache directory, `/laft/cache` on a running system. Packages go
    /// to `pkgs/` and accepted indexes to `index/` below it.
    pub cache: PathBuf,
    /// Never use the network. Files come from the cache and from `file://`
    /// repositories only, which are local.
    pub offline: bool,
    /// The longest time to resolve a host name and to connect, including
    /// the TLS handshake.
    pub connect_timeout: Duration,
    /// The longest time a server may stay silent, while sending the
    /// response headers or the body. A slow but steady download never
    /// times out.
    pub read_timeout: Duration,
    /// How often a request to one URL is repeated after a timeout, a broken
    /// connection, or an HTTP status that says the server is busy or
    /// failing (408, 429 and 5xx). Other failures move on to the next URL
    /// at once.
    pub retries: u32,
    /// The pause before the first repeated request. Each further pause is
    /// twice as long.
    pub retry_delay: Duration,
}

impl Config {
    /// The defaults for the cache directory `cache`: online, 30 seconds to
    /// connect, 30 seconds of silence, and 3 retries after pauses of 1, 2
    /// and 4 seconds.
    pub fn new(cache: impl Into<PathBuf>) -> Self {
        Self {
            cache: cache.into(),
            offline: false,
            connect_timeout: Duration::from_secs(30),
            read_timeout: Duration::from_secs(30),
            retries: 3,
            retry_delay: Duration::from_secs(1),
        }
    }
}

/// How far a download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// The bytes received so far, including those of an earlier partial
    /// download that is being resumed.
    pub done: u64,
    /// The size of the whole file, when it is known.
    pub total: Option<u64>,
}

/// The kinds of small repository files that are read into memory, each with
/// its own size limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    /// `keys.json`, at most 1 MiB.
    Keys,
    /// `mirrors.json`, at most 1 MiB.
    Mirrors,
    /// A `.minisig` signature, at most 64 KiB.
    Signature,
    /// A compressed index or snapshot, at most as large as an index may be
    /// once decompressed, [`MAX_INDEX_SIZE`].
    Index,
}

impl ObjectKind {
    /// The largest file of this kind, in bytes.
    pub const fn max_size(self) -> u64 {
        match self {
            Self::Keys | Self::Mirrors => 1 << 20,
            Self::Signature => 64 << 10,
            Self::Index => MAX_INDEX_SIZE,
        }
    }
}

/// Moves repository files to the client and caches them.
///
/// A fetcher speaks HTTP/1.1 over TLS (rustls with a compiled-in set of
/// web PKI root certificates), plain HTTP for repositories that allow it,
/// and reads `file://` repositories in place. Proxies are taken from
/// `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY`. A redirect never
/// leads from `https://` to `http://`.
///
/// It decides nothing about trust. Signed documents are handed to
/// `laft-index`, and packages are checked only against the size and BLAKE3
/// hash of their index entry. Nothing is printed. Progress is reported to a
/// callback.
///
/// A fetcher is shared between threads by reference. Different files may
/// be downloaded at the same time, but one file must not be downloaded by
/// two callers at once, because they would share its partial file.
///
/// ```no_run
/// use laft_fetch::{Config, Fetcher, Mirrors, ObjectKind};
///
/// let fetcher = Fetcher::new(Config::new("/laft/cache"));
/// let mirrors = Mirrors::new("https://pkgs.example.org/laft", &[] as &[&str], false)?;
/// let keys = fetcher.fetch(&mirrors.bases()[0], "keys.json", ObjectKind::Keys, &mut |_| {})?;
/// # Ok::<(), laft_fetch::Error>(())
/// ```
#[derive(Debug)]
pub struct Fetcher {
    config: Config,
    https: Agent,
    http: Agent,
}

impl Fetcher {
    /// A fetcher with `config`.
    pub fn new(config: Config) -> Self {
        Self {
            https: agent(&config, true),
            http: agent(&config, false),
            config,
        }
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Reads the small file `path` below `base` into memory, retrying
    /// transient failures. Nothing is cached, because these files are
    /// verified by the caller first.
    ///
    /// # Errors
    ///
    /// Fails with [`ErrorKind::NotFound`] when the file does not exist,
    /// which callers may need to tell from other failures,
    /// [`ErrorKind::TooLarge`] beyond the limit of `kind`,
    /// [`ErrorKind::Offline`] for a network URL in offline mode, and
    /// otherwise with the error of the last attempt. Every error names the
    /// URL.
    pub fn fetch(
        &self,
        base: &BaseUrl,
        path: &str,
        kind: ObjectKind,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Vec<u8>, Error> {
        let url = base.join(path);
        check_path(path).map_err(|kind| Error::new(&url, kind))?;
        let limit = kind.max_size();
        if let Some(dir) = base.dir() {
            return read_file(&dir.join(path), &url, limit, progress);
        }
        if self.config.offline {
            return Err(Error::new(url, ErrorKind::Offline));
        }
        self.retrying(|| {
            let response = self.get(base, &url, 0)?;
            match response.status() {
                StatusCode::OK => {}
                status => return Err(status_error(&url, status)),
            }
            let total = content_length(&response);
            if total.is_some_and(|n| n > limit) {
                return Err(Error::new(&url, ErrorKind::TooLarge { limit }));
            }
            let mut bytes = Vec::new();
            let mut reader = response.into_body().into_reader();
            pump(
                &mut reader,
                &url,
                net(&url),
                limit,
                Progress { done: 0, total },
                progress,
                |chunk| {
                    bytes.extend_from_slice(chunk);
                    Ok(())
                },
            )?;
            Ok(bytes)
        })
    }

    /// Makes the package of `entry` available as a verified local file and
    /// returns its path.
    ///
    /// A cached copy at `pkgs/<id>.lpkg` is used when its size and BLAKE3
    /// hash match the entry. Otherwise the mirrors are tried in order. A
    /// `file://` repository is read in place, so the returned path is the
    /// file inside it. From a network mirror, the file is downloaded to
    /// `pkgs/<id>.lpkg.part`, synced and renamed into place. A partial file
    /// left by an interrupted download, from any mirror, is resumed with an
    /// HTTP range request when the server supports it. A file whose size or
    /// hash does not match is deleted, and the next mirror is tried.
    ///
    /// In offline mode only the cache and `file://` repositories are used.
    ///
    /// # Errors
    ///
    /// Fails with [`ErrorKind::NotCached`] in offline mode when no copy is
    /// available, and otherwise with [`ErrorKind::AllFailed`] listing the
    /// failure of every mirror, or with the failure itself when there was
    /// only one.
    pub fn download_package(
        &self,
        mirrors: &Mirrors,
        entry: &IndexEntry,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<PathBuf, Error> {
        let dest = self.config.cache.join(format!("pkgs/{}.lpkg", entry.id));
        let dest_url = format!("file://{}", dest.display());
        let expected = Expected {
            size: entry.size,
            blake3: entry.blake3,
        };
        match check_file(&dest, &dest_url, &expected, &mut |_| {}) {
            Ok(()) => return Ok(dest),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound) => {}
            Err(_) => remove(&dest)?,
        }
        let mut attempts = Vec::new();
        for base in mirrors.bases() {
            let url = base.join(&entry.file);
            let result = if let Err(kind) = check_path(&entry.file) {
                Err(Error::new(&url, kind))
            } else if let Some(dir) = base.dir() {
                let path = dir.join(&entry.file);
                check_file(&path, &url, &expected, progress).map(|()| path)
            } else if self.config.offline {
                Err(Error::new(
                    &url,
                    ErrorKind::NotCached { path: dest.clone() },
                ))
            } else {
                self.download(base, &url, &dest, &expected, progress)
                    .map(|()| dest.clone())
            };
            match result {
                Ok(path) => return Ok(path),
                Err(e) => attempts.push(e),
            }
        }
        Err(all_failed(&entry.file, attempts))
    }

    /// Downloads `url` to `dest` through its partial file, retrying and
    /// resuming after transient failures.
    fn download(
        &self,
        base: &BaseUrl,
        url: &str,
        dest: &Path,
        expected: &Expected,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(), Error> {
        let dir = dest.parent().unwrap_or(Path::new("."));
        create_dir(dir)?;
        let part = part_path(dest);
        // A partial file left by an earlier run may hold bytes that this
        // mirror did not send, so a mismatch after resuming it is not the
        // mirror's fault and the download starts over once.
        let resumed = std::fs::metadata(&part).is_ok_and(|m| m.len() > 0);
        match self.download_part(base, url, &part, expected, progress) {
            Err(e)
                if resumed
                    && matches!(
                        e.kind(),
                        ErrorKind::SizeMismatch { .. } | ErrorKind::HashMismatch { .. }
                    ) =>
            {
                self.download_part(base, url, &part, expected, progress)?;
            }
            result => result?,
        }
        publish(&part, dest)
    }

    /// Completes the partial file `part` and checks it. A partial file that
    /// does not match is deleted.
    fn download_part(
        &self,
        base: &BaseUrl,
        url: &str,
        part: &Path,
        expected: &Expected,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(), Error> {
        let downloaded =
            self.retrying(|| self.download_once(base, url, part, expected.size, progress));
        let (size, hash) = match downloaded {
            Ok(done) => done,
            Err(e) => {
                // A partial file is kept for resuming, unless it holds
                // nothing or the server sent more than the entry allows.
                let empty = std::fs::metadata(part).is_ok_and(|m| m.len() == 0);
                if empty || matches!(e.kind(), ErrorKind::TooLarge { .. }) {
                    remove(part)?;
                }
                return Err(e);
            }
        };
        expected.check(size, hash).or_else(|kind| {
            remove(part)?;
            Err(Error::new(url, kind))
        })
    }

    /// Makes one request that continues the partial file `part`, and
    /// returns the size and hash of the file it leaves.
    fn download_once(
        &self,
        base: &BaseUrl,
        url: &str,
        part: &Path,
        limit: u64,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(u64, Blake3Hash), Error> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(FILE_MODE)
            .open(part)
            .map_err(io(part))?;
        let mut hasher = blake3::Hasher::new();
        let mut offset =
            io::copy(&mut (&mut file).take(limit + 1), &mut hasher).map_err(io(part))?;
        if offset > limit {
            truncate(&mut file, part, &mut hasher)?;
            offset = 0;
        }
        if offset == limit {
            file.sync_all().map_err(io(part))?;
            return Ok((
                offset,
                Blake3Hash::from_bytes(*hasher.finalize().as_bytes()),
            ));
        }
        let response = self.get(base, url, offset)?;
        match response.status() {
            StatusCode::OK if offset > 0 => {
                truncate(&mut file, part, &mut hasher)?;
                offset = 0;
            }
            StatusCode::OK => {}
            StatusCode::PARTIAL_CONTENT if range_start(&response) == Some(offset) => {}
            StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE => {
                truncate(&mut file, part, &mut hasher)?;
                let reason = format!("the server cannot resume the download at byte {offset}");
                return Err(Error::new(url, ErrorKind::Transport { reason }));
            }
            status => return Err(status_error(url, status)),
        }
        if content_length(&response).is_some_and(|n| offset + n > limit) {
            return Err(Error::new(url, ErrorKind::TooLarge { limit }));
        }
        let mut reader = response.into_body().into_reader();
        let total = Some(limit);
        let size = pump(
            &mut reader,
            url,
            net(url),
            limit,
            Progress {
                done: offset,
                total,
            },
            progress,
            |chunk| {
                hasher.update(chunk);
                file.write_all(chunk).map_err(io(part))
            },
        );
        // Whatever arrived before a failure is kept for the next attempt.
        file.sync_all().map_err(io(part))?;
        Ok((size?, Blake3Hash::from_bytes(*hasher.finalize().as_bytes())))
    }

    /// Calls `attempt` until it succeeds, fails for a reason that a repeat
    /// cannot fix, or the retries are used up.
    fn retrying<T>(&self, mut attempt: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
        let mut delay = self.config.retry_delay;
        for _ in 0..self.config.retries {
            match attempt() {
                Err(e) if e.is_transient() => {
                    thread::sleep(delay);
                    delay = delay.saturating_mul(2);
                }
                result => return result,
            }
        }
        attempt()
    }

    /// Sends a GET request for `url`, asking for the bytes from `offset` on
    /// when it is not zero.
    fn get(&self, base: &BaseUrl, url: &str, offset: u64) -> Result<Response<ureq::Body>, Error> {
        let agent = match base.scheme() {
            Scheme::Http => &self.http,
            _ => &self.https,
        };
        let mut request = agent.get(url);
        if offset > 0 {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
        }
        request.call().map_err(|e| transport(url, e))
    }
}

/// The size and hash a downloaded file must have.
struct Expected {
    size: u64,
    blake3: Blake3Hash,
}

impl Expected {
    fn check(&self, size: u64, blake3: Blake3Hash) -> Result<(), ErrorKind> {
        if size != self.size {
            return Err(ErrorKind::SizeMismatch {
                expected: self.size,
                actual: size,
            });
        }
        if blake3 != self.blake3 {
            return Err(ErrorKind::HashMismatch {
                expected: self.blake3,
                actual: blake3,
            });
        }
        Ok(())
    }
}

fn agent(config: &Config, https_only: bool) -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .https_only(https_only)
        .user_agent(concat!("laft-fetch/", env!("CARGO_PKG_VERSION")))
        .timeout_resolve(Some(config.connect_timeout))
        .timeout_connect(Some(config.connect_timeout))
        .timeout_per_read(Some(config.read_timeout))
        .timeout_per_write(Some(config.read_timeout))
        .build()
        .new_agent()
}

/// Checks that the local file `path` has the expected size and hash.
fn check_file(
    path: &Path,
    url: &str,
    expected: &Expected,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), Error> {
    let mut file = open(path, url)?;
    let mut hasher = blake3::Hasher::new();
    let total = Some(expected.size);
    let size = pump(
        &mut file,
        url,
        io(path),
        expected.size,
        Progress { done: 0, total },
        progress,
        |chunk| {
            hasher.update(chunk);
            Ok(())
        },
    )?;
    expected
        .check(size, Blake3Hash::from_bytes(*hasher.finalize().as_bytes()))
        .map_err(|kind| Error::new(url, kind))
}

/// Reads the local file `path`, which is at most `limit` bytes long.
pub(crate) fn read_file(
    path: &Path,
    url: &str,
    limit: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<Vec<u8>, Error> {
    let mut file = open(path, url)?;
    let total = file.metadata().ok().map(|m| m.len());
    let mut bytes = Vec::new();
    pump(
        &mut file,
        url,
        io(path),
        limit,
        Progress { done: 0, total },
        progress,
        |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        },
    )?;
    Ok(bytes)
}

fn open(path: &Path, url: &str) -> Result<File, Error> {
    File::open(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => Error::new(url, ErrorKind::NotFound),
        _ => io(path)(e),
    })
}

/// Streams `reader` into `sink`, failing beyond `limit` bytes in total.
/// `start` counts the bytes received before. Returns the total.
fn pump(
    reader: &mut dyn Read,
    url: &str,
    read_error: impl Fn(io::Error) -> Error,
    limit: u64,
    start: Progress,
    progress: &mut dyn FnMut(Progress),
    mut sink: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<u64, Error> {
    let Progress { mut done, total } = start;
    let mut buf = vec![0; 64 << 10];
    progress(start);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(done),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(read_error(e)),
        };
        done += n as u64;
        if done > limit {
            return Err(Error::new(url, ErrorKind::TooLarge { limit }));
        }
        sink(&buf[..n])?;
        progress(Progress { done, total });
    }
}

fn truncate(file: &mut File, path: &Path, hasher: &mut blake3::Hasher) -> Result<(), Error> {
    file.set_len(0).map_err(io(path))?;
    file.rewind().map_err(io(path))?;
    hasher.reset();
    Ok(())
}

fn content_length(response: &Response<ureq::Body>) -> Option<u64> {
    response
        .headers()
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// The first byte of a `Content-Range: bytes <first>-<last>/<size>` answer.
fn range_start(response: &Response<ureq::Body>) -> Option<u64> {
    let value = response
        .headers()
        .get(header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    let (first, _) = value.strip_prefix("bytes ")?.split_once('-')?;
    first.parse().ok()
}

/// Maps an error from reading a response body.
fn net(url: &str) -> impl Fn(io::Error) -> Error + '_ {
    move |e| transport(url, ureq::Error::from(e))
}

fn status_error(url: &str, status: StatusCode) -> Error {
    let kind = match status {
        StatusCode::NOT_FOUND => ErrorKind::NotFound,
        _ => ErrorKind::Status {
            code: status.as_u16(),
        },
    };
    Error::new(url, kind)
}

fn transport(url: &str, error: ureq::Error) -> Error {
    let kind = match error {
        ureq::Error::Timeout(_) => ErrorKind::Timeout,
        ureq::Error::Io(e)
            if matches!(
                e.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) =>
        {
            ErrorKind::Timeout
        }
        e => ErrorKind::Transport {
            reason: e.to_string(),
        },
    };
    Error::new(url, kind)
}

/// The error for `what` after every URL failed with `attempts`. A single
/// attempt is returned as it is.
pub(crate) fn all_failed(what: &str, mut attempts: Vec<Error>) -> Error {
    if attempts.len() == 1
        && let Some(only) = attempts.pop()
    {
        return only;
    }
    ErrorKind::AllFailed {
        what: what.into(),
        attempts,
    }
    .into()
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
