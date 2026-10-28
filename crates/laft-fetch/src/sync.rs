use std::path::PathBuf;

use laft_core::{Blake3Hash, Timestamp};
use laft_index::{
    AcceptedIndex, Index, IndexComment, IndexState, KeysFetch, MAX_INDEX_SIZE, Repo, Signature,
    Trust,
};

use crate::fetch::{all_failed, read_file};
use crate::fs::{remove, write_atomic};
use crate::{BaseUrl, Error, ErrorKind, Fetcher, Mirrors, ObjectKind, Progress};

const KEYS: &str = "keys.json";
const KEYS_SIGNATURE: &str = "keys.json.minisig";

/// What [`Fetcher::sync`] needs to know about one repository channel.
#[derive(Debug, Clone, Copy)]
pub struct SyncRequest<'a> {
    /// The repository's trust configuration.
    pub repo: &'a Repo,
    /// The URLs to try, in order.
    pub mirrors: &'a Mirrors,
    /// The accepted-state records.
    pub state: &'a IndexState,
    /// The current time, which accepted indexes must not have expired at.
    pub now: Timestamp,
    /// Accept an expired index. The caller should warn on every command
    /// that uses one.
    pub allow_expired: bool,
}

/// The index a sync produced.
#[derive(Debug)]
pub struct Synced {
    /// The accepted index. Its `expired` flag is set when it expired and
    /// expired indexes were allowed.
    pub index: AcceptedIndex,
    /// Where the index came from.
    pub source: Source,
}

/// Where a synced index came from.
#[derive(Debug)]
pub enum Source {
    /// The index was downloaded from `base` and accepted.
    Mirror {
        /// The URL the index was accepted from. Package downloads try it
        /// first, see [`Mirrors::preferring`]. A `http://` URL leaks what
        /// the machine installs, which deserves a warning.
        base: BaseUrl,
        /// The keys the repository's documents may be signed with.
        /// [`Trust::keys`] is `None` when the repository serves no
        /// `keys.json` and runs in direct mode.
        trust: Trust,
    },
    /// Every URL failed, or the fetcher is offline, and the previously
    /// accepted index from the cache is used instead. The caller should
    /// warn with the index's generation and age.
    Cache {
        /// Why the repository could not be synced.
        failure: Error,
    },
}

impl Fetcher {
    /// Syncs one channel of a repository and returns its accepted index.
    ///
    /// For each URL in order, `keys.json` and then its signature are
    /// downloaded and handed to [`IndexState::accept_keys`] until one is
    /// accepted. Any failure, including a 404 or a rejection, moves on to
    /// the next URL. When no URL serves a `keys.json` and at least one
    /// answers 404 for it, [`KeysFetch::NotFound`] is passed on, which means
    /// direct mode unless a `keys.json` was accepted before. The accepted
    /// `keys.json` is not cached, because only its record is needed later.
    /// Then, for each URL in order,
    /// the index signature and the index of the channel are downloaded and
    /// handed to [`IndexState::accept_index`] until one is accepted. When
    /// the two do not match, because the repository was published between
    /// the two requests, the pair is downloaded once more, and then the
    /// immutable snapshot named by the signature is used.
    ///
    /// Accepted indexes are written to `index/<repo>/` in the cache with the
    /// repository's layout, through a synced `.part` file and a rename.
    ///
    /// When every URL fails, the index accepted by an earlier sync is read
    /// from the cache, provided it is still the accepted one, its record
    /// still pins its hash, and it has not expired, and [`Source::Cache`]
    /// says why the sync failed. When this sync accepted a `keys.json` that
    /// revokes the cached index's signer, the cached index is refused and
    /// removed from the cache. Offline, network URLs are refused without a
    /// request, so only `file://` URLs are tried.
    ///
    /// # Errors
    ///
    /// Fails with [`ErrorKind::AllFailed`], listing every attempt and why
    /// the cache could not be used, when no index is available. A state
    /// record that cannot be read or written fails at once with
    /// [`ErrorKind::Trust`]. A file that cannot be written to the cache
    /// counts as a failed attempt.
    pub fn sync(
        &self,
        request: &SyncRequest<'_>,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Synced, Error> {
        let mut attempts = Vec::new();
        let trust = self.sync_keys(request, &mut attempts)?;
        if let Some(trust) = &trust {
            for base in request.mirrors.bases() {
                match self.index_from(base, request, trust, progress) {
                    Ok(index) => {
                        let base = base.clone();
                        let trust = trust.clone();
                        let source = Source::Mirror { base, trust };
                        return Ok(Synced { index, source });
                    }
                    Err(e) if is_fatal(&e) => return Err(e),
                    Err(e) => attempts.push(e),
                }
            }
        }
        let repo = request.repo;
        let what = format!("repo {} channel {}/{}", repo.name, repo.channel, repo.arch);
        match self.cached_index(request, trust.as_ref()) {
            Ok(index) => Ok(Synced {
                index,
                source: Source::Cache {
                    failure: all_failed(&what, attempts),
                },
            }),
            Err(e) if is_fatal(&e) => Err(e),
            Err(e) => {
                attempts.push(e);
                Err(ErrorKind::AllFailed { what, attempts }.into())
            }
        }
    }

    fn sync_keys(
        &self,
        request: &SyncRequest<'_>,
        attempts: &mut Vec<Error>,
    ) -> Result<Option<Trust>, Error> {
        let quiet = &mut |_: Progress| {};
        let mut not_found = None;
        let mut served = false;
        for base in request.mirrors.bases() {
            let json = match self.fetch(base, KEYS, ObjectKind::Keys, quiet) {
                Ok(json) => json,
                Err(e) => {
                    if matches!(e.kind(), ErrorKind::NotFound) {
                        not_found.get_or_insert(base);
                    }
                    attempts.push(e);
                    continue;
                }
            };
            served = true;
            let signature = match self.fetch(base, KEYS_SIGNATURE, ObjectKind::Signature, quiet) {
                Ok(signature) => signature,
                Err(e) => {
                    attempts.push(e);
                    continue;
                }
            };
            let fetch = KeysFetch::Found {
                json: &json,
                signature: &signature,
            };
            match self.accept_keys(request, fetch, base) {
                Ok(trust) => return Ok(Some(trust)),
                Err(e) if is_fatal(&e) => return Err(e),
                Err(e) => attempts.push(e),
            }
        }
        let Some(base) = not_found.filter(|_| !served) else {
            return Ok(None);
        };
        match self.accept_keys(request, KeysFetch::NotFound, base) {
            Ok(trust) => Ok(Some(trust)),
            Err(e) if is_fatal(&e) => Err(e),
            Err(e) => {
                attempts.push(e);
                Ok(None)
            }
        }
    }

    fn accept_keys(
        &self,
        request: &SyncRequest<'_>,
        fetch: KeysFetch<'_>,
        base: &BaseUrl,
    ) -> Result<Trust, Error> {
        request
            .state
            .accept_keys(request.repo, fetch)
            .map_err(|e| Error::new(base.join(KEYS), ErrorKind::Trust(Box::new(e))))
    }

    /// Downloads and accepts the index of the channel from `base`, and
    /// caches it.
    fn index_from(
        &self,
        base: &BaseUrl,
        request: &SyncRequest<'_>,
        trust: &Trust,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<AcceptedIndex, Error> {
        let repo = request.repo;
        let dir = channel_dir(repo);
        let index_path = format!("{dir}/index.json.zst");
        let signature_path = format!("{index_path}.minisig");
        let accept = |path: &str, index: &[u8], signature: &[u8]| {
            let accepted = request
                .state
                .accept_index(
                    repo,
                    trust,
                    index,
                    signature,
                    request.now,
                    request.allow_expired,
                )
                .map_err(|e| Error::new(base.join(path), ErrorKind::Trust(Box::new(e))))?;
            let cache = self.repo_cache(repo).join(&dir);
            write_atomic(&cache.join("index.json.zst"), index)?;
            write_atomic(&cache.join("index.json.zst.minisig"), signature)?;
            Ok(accepted)
        };
        let mut retried = false;
        let (signature, mismatch) = loop {
            let signature =
                self.fetch(base, &signature_path, ObjectKind::Signature, &mut |_| {})?;
            let index = self.fetch(base, &index_path, ObjectKind::Index, progress)?;
            match accept(&index_path, &index, &signature) {
                Err(e) if is_bad_signature(&e) && !retried => retried = true,
                Err(e) if is_bad_signature(&e) => break (signature, e),
                result => return result,
            }
        };
        let Some(generation) = Signature::decode(&signature)
            .ok()
            .and_then(|s| s.trusted_comment().parse::<IndexComment>().ok())
            .map(|c| c.generation)
        else {
            return Err(mismatch);
        };
        let snapshot_path = format!("{dir}/snapshots/{generation}.json.zst");
        let snapshot = self.fetch(base, &snapshot_path, ObjectKind::Index, progress)?;
        accept(&snapshot_path, &snapshot, &signature)
    }

    /// The index accepted by an earlier sync, read back from the cache, and
    /// checked against `trust` when this sync accepted a `keys.json`.
    fn cached_index(
        &self,
        request: &SyncRequest<'_>,
        trust: Option<&Trust>,
    ) -> Result<AcceptedIndex, Error> {
        let repo = request.repo;
        let path = self
            .repo_cache(repo)
            .join(channel_dir(repo))
            .join("index.json.zst");
        let url = format!("file://{}", path.display());
        let trust_error = |e| Error::new(&url, ErrorKind::Trust(Box::new(e)));
        let record = request
            .state
            .index_records(&repo.name)
            .map_err(trust_error)?
            .into_iter()
            .find(|r| r.channel == repo.channel && r.arch == repo.arch)
            .ok_or_else(|| Error::new(&url, ErrorKind::NotFound))?;
        let expected = record.blake3.ok_or_else(|| {
            let generation = record.generation;
            Error::new(&url, ErrorKind::CachedIndexUnpinned { generation })
        })?;
        if trust
            .and_then(Trust::keys)
            .is_some_and(|keys| keys.is_revoked(record.signer))
        {
            remove(&path)?;
            remove(&path.with_extension("zst.minisig"))?;
            let signer = record.signer;
            return Err(Error::new(&url, ErrorKind::CachedIndexRevoked { signer }));
        }
        let bytes = read_file(&path, &url, MAX_INDEX_SIZE, &mut |_| {})?;
        let actual = Blake3Hash::of(&bytes);
        if actual != expected {
            return Err(Error::new(
                &url,
                ErrorKind::HashMismatch { expected, actual },
            ));
        }
        let index = Index::decode(&bytes).map_err(trust_error)?;
        let expired = request.now > index.expires;
        if expired && !request.allow_expired {
            let kind = laft_index::ErrorKind::Expired {
                generation: index.generation,
                expires: index.expires,
            };
            return Err(trust_error(laft_index::Error::from(kind)));
        }
        Ok(AcceptedIndex {
            index,
            record,
            expired,
        })
    }

    fn repo_cache(&self, repo: &Repo) -> PathBuf {
        self.config().cache.join("index").join(repo.name.as_str())
    }
}

/// The directory of a channel's index, relative to the repository base.
fn channel_dir(repo: &Repo) -> String {
    format!("channels/{}/{}", repo.channel, repo.arch)
}

fn trust_kind(error: &Error) -> Option<&laft_index::ErrorKind> {
    match error.kind() {
        ErrorKind::Trust(e) => Some(e.kind()),
        _ => None,
    }
}

/// Whether the accepted-state records are unusable, so that no URL can
/// succeed and the cache cannot be checked either.
fn is_fatal(error: &Error) -> bool {
    matches!(
        trust_kind(error),
        Some(laft_index::ErrorKind::Io { .. } | laft_index::ErrorKind::CorruptState { .. })
    )
}

fn is_bad_signature(error: &Error) -> bool {
    matches!(
        trust_kind(error),
        Some(laft_index::ErrorKind::BadSignature { .. })
    )
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
