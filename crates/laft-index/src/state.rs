use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use laft_core::{Blake3Hash, Name, Timestamp};
use laft_pkg::Arch;
use serde::{Deserialize, Serialize};

use crate::error::io;
use crate::{
    Error, ErrorKind, Index, IndexComment, KeyId, Keys, KeysComment, OnlineKey, PublicKey,
    Signature,
};

/// The version of the state record formats.
const FORMAT: u32 = 1;

/// The trust-relevant configuration of one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The repository name, which signed documents must name too.
    pub name: Name,
    /// The channel the client follows.
    pub channel: Name,
    /// The client's architecture.
    pub arch: Arch,
    /// The root key.
    pub key: PublicKey,
    /// Further accepted root keys, used while a root key is replaced.
    pub keys: Vec<PublicKey>,
}

impl Repo {
    fn roots(&self) -> impl Iterator<Item = &PublicKey> {
        std::iter::once(&self.key).chain(&self.keys)
    }
}

/// The outcome of downloading a repository's `keys.json`.
#[derive(Debug, Clone, Copy)]
pub enum KeysFetch<'a> {
    /// The file and its signature were downloaded.
    Found {
        /// The bytes of `keys.json`.
        json: &'a [u8],
        /// The bytes of `keys.json.minisig`.
        signature: &'a [u8],
    },
    /// No URL served `keys.json`, and at least one reported that it does
    /// not exist (HTTP 404, or a missing file in a local repository).
    NotFound,
}

/// The keys an index of one repository may be signed with, as established
/// by [`IndexState::accept_keys`].
#[derive(Debug, Clone)]
pub struct Trust {
    roots: Vec<PublicKey>,
    keys: Option<Keys>,
}

impl Trust {
    /// The accepted `keys.json`, or `None` when the repository runs in
    /// direct mode and signs indexes with a root key.
    pub fn keys(&self) -> Option<&Keys> {
        self.keys.as_ref()
    }

    /// Finds the key that may have made `signature` and the window its
    /// signatures are valid in. Root keys are always accepted, so that a
    /// root key can sign an emergency index.
    fn signer(&self, signature: &Signature) -> Result<(&PublicKey, Option<&OnlineKey>), ErrorKind> {
        let id = signature.key_id();
        if let Some(keys) = &self.keys
            && keys.is_revoked(id)
        {
            return Err(ErrorKind::RevokedKey { signer: id });
        }
        if let Some(root) = self.roots.iter().find(|k| k.id() == id) {
            return Ok((root, None));
        }
        if let Some(online) = self.keys.as_ref().and_then(|k| k.key(id)) {
            return Ok((&online.public, Some(online)));
        }
        let online = self.keys.iter().flat_map(|k| &k.keys).map(|k| k.id);
        Err(ErrorKind::UnknownSigner {
            document: "index",
            signer: id,
            trusted: self.roots.iter().map(PublicKey::id).chain(online).collect(),
        })
    }
}

/// An index that passed every check, and the record of its acceptance.
#[derive(Debug, Clone)]
pub struct AcceptedIndex {
    /// The parsed index.
    pub index: Index,
    /// The state record written for it.
    pub record: IndexRecord,
    /// Whether the index is expired and was accepted only because expired
    /// indexes were allowed. Every command that uses it should warn.
    pub expired: bool,
}

/// The last accepted `keys.json` of a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeysRecord {
    /// The record format version.
    pub format: u32,
    /// The accepted version.
    pub version: u64,
    /// The BLAKE3 hash of the accepted file.
    pub blake3: Blake3Hash,
    /// The root key that verified the accepted file. The record holds while
    /// that key is a configured root key, as the main key or an additional
    /// one. A record whose root key is no longer configured is discarded,
    /// which is the only way it is ever cleared apart from a reset.
    pub root: KeyId,
}

/// The last accepted index of one channel and architecture of a
/// repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexRecord {
    /// The channel.
    pub channel: Name,
    /// The architecture.
    pub arch: Arch,
    /// The accepted generation. Lower generations are refused.
    pub generation: u64,
    /// The BLAKE3 hash of the accepted index file. An index with the same
    /// generation is accepted only with this hash. It is `None` after a
    /// generation floor replaced the generation, and then any index with
    /// that generation is accepted.
    pub blake3: Option<Blake3Hash>,
    /// The key that signed the accepted index.
    pub signer: KeyId,
}

/// The index records of one repository, one per channel and architecture.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexRecords {
    format: u32,
    records: Vec<IndexRecord>,
}

/// The accepted-state records of every repository, kept in one directory,
/// `/laft/state/index` on a running system.
///
/// For each repository `<repo>`, `<repo>.keys.json` records the accepted
/// `keys.json` and `<repo>.json` the accepted index of each channel and
/// architecture. Every record is replaced with one rename after it is
/// synced, so a crash leaves either the old record or the new one.
///
/// Callers hold the transaction lock of the root while accepting
/// documents, so that two syncs never interleave. The crate documentation
/// shows a whole sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexState {
    dir: PathBuf,
}

impl IndexState {
    /// The records in `dir`, which is created when the first record is
    /// written.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The accepted `keys.json` record of `repo`, if any.
    ///
    /// # Errors
    ///
    /// Fails when the record exists but cannot be read.
    pub fn keys_record(&self, repo: &Name) -> Result<Option<KeysRecord>, Error> {
        let path = self.keys_path(repo);
        let record: Option<KeysRecord> = read_record(&path)?;
        if record.as_ref().is_some_and(|r| r.format != FORMAT) {
            return Err(corrupt(&path, "unsupported format"));
        }
        Ok(record)
    }

    /// The accepted index records of `repo`, sorted by channel and
    /// architecture.
    ///
    /// # Errors
    ///
    /// Fails when the records exist but cannot be read.
    pub fn index_records(&self, repo: &Name) -> Result<Vec<IndexRecord>, Error> {
        let path = self.index_path(repo);
        match read_record::<IndexRecords>(&path)? {
            None => Ok(Vec::new()),
            Some(file) if file.format == FORMAT => Ok(file.records),
            Some(_) => Err(corrupt(&path, "unsupported format")),
        }
    }

    /// Accepts the `keys.json` of `repo` and returns the keys its indexes
    /// may be signed with.
    ///
    /// A missing `keys.json` means direct mode, where indexes are signed
    /// with a root key, unless one was accepted before and verified by a
    /// root key that is still configured. A `keys.json` must be signed by a root key, name the repository
    /// in its trusted comment and content, and have a version no lower than
    /// the accepted one, with identical bytes for an equal version.
    ///
    /// When a newer `keys.json` revokes the key that signed an accepted
    /// index and sets a generation floor below that index's generation, the
    /// record's generation is lowered to the floor and its hash is cleared,
    /// so that honest indexes above the floor are accepted again. A floor
    /// never raises an accepted generation.
    ///
    /// The record holds the version, the hash and the root key that verified
    /// the file, which is all that later checks need, so the caller need not
    /// keep the accepted bytes.
    ///
    /// # Errors
    ///
    /// Fails with the [`ErrorKind`] of the first check that rejects the
    /// file, or when a record cannot be read or written.
    pub fn accept_keys(&self, repo: &Repo, fetch: KeysFetch<'_>) -> Result<Trust, Error> {
        self.accept_keys_inner(repo, fetch)
            .map_err(|e| e.in_repo(&repo.name))
    }

    fn accept_keys_inner(&self, repo: &Repo, fetch: KeysFetch<'_>) -> Result<Trust, Error> {
        let mut trust = Trust {
            roots: repo.roots().cloned().collect(),
            keys: None,
        };
        let path = self.keys_path(&repo.name);
        let recorded = self.keys_record(&repo.name)?;
        let stored = recorded
            .as_ref()
            .filter(|r| repo.roots().any(|k| k.id() == r.root));
        let (json, signature) = match fetch {
            KeysFetch::NotFound if stored.is_some() => return Err(ErrorKind::KeysMissing.into()),
            KeysFetch::NotFound => {
                if recorded.is_some() {
                    remove(&path)?;
                    sync_dir(&self.dir)?;
                }
                return Ok(trust);
            }
            KeysFetch::Found { json, signature } => (json, signature),
        };
        let signature = Signature::decode(signature)?;
        let signer = signature.key_id();
        let root =
            repo.roots()
                .find(|k| k.id() == signer)
                .ok_or_else(|| ErrorKind::UnknownSigner {
                    document: "keys.json",
                    signer,
                    trusted: repo.roots().map(PublicKey::id).collect(),
                })?;
        if !root.verifies(json, &signature) {
            return Err(ErrorKind::BadSignature {
                document: "keys.json",
                signer,
            }
            .into());
        }
        let comment: KeysComment = signature.trusted_comment().parse()?;
        if comment.repo != repo.name {
            return Err(wrong_target("keys.json", "repo", &repo.name, &comment.repo));
        }
        let keys = Keys::from_json(json)?;
        if keys.repo != comment.repo {
            return Err(inconsistent("keys.json", "repo", &comment.repo, &keys.repo));
        }
        if keys.version != comment.version {
            return Err(inconsistent(
                "keys.json",
                "version",
                comment.version,
                keys.version,
            ));
        }
        let blake3 = Blake3Hash::of(json);
        if let Some(stored) = stored {
            if keys.version < stored.version {
                return Err(ErrorKind::KeysRollback {
                    version: keys.version,
                    accepted: stored.version,
                }
                .into());
            }
            if keys.version == stored.version {
                if blake3 != stored.blake3 {
                    return Err(ErrorKind::KeysReplaced {
                        version: keys.version,
                    }
                    .into());
                }
                trust.keys = Some(keys);
                return Ok(trust);
            }
        }
        // The index records are lowered before the keys record is written,
        // so that a crash in between repeats the recovery on the next sync.
        self.apply_generation_floor(&repo.name, &keys)?;
        let record = KeysRecord {
            format: FORMAT,
            version: keys.version,
            blake3,
            root: signer,
        };
        self.write_record(&path, &record)?;
        trust.keys = Some(keys);
        Ok(trust)
    }

    fn apply_generation_floor(&self, repo: &Name, keys: &Keys) -> Result<(), Error> {
        let Some(floor) = keys.generation_floor else {
            return Ok(());
        };
        let mut records = self.index_records(repo)?;
        let mut lowered = false;
        for record in records
            .iter_mut()
            .filter(|r| keys.is_revoked(r.signer) && floor < r.generation)
        {
            record.generation = floor;
            record.blake3 = None;
            lowered = true;
        }
        if lowered {
            self.write_index_records(repo, records)?;
        }
        Ok(())
    }

    /// Accepts a downloaded index of `repo` and records it.
    ///
    /// The checks run in this order, and the first one that fails rejects
    /// the index:
    ///
    /// 1. The signature is made by a key `trust` allows and verifies over
    ///    the exact bytes of `index` and over its trusted comment.
    /// 2. The trusted comment names the configured repository, channel and
    ///    architecture.
    /// 3. Its generation is not lower than the accepted one, and an equal
    ///    generation comes with the accepted bytes.
    /// 4. The index decompresses within the size limit and is valid, and its
    ///    repository, channel, architecture and generation equal the trusted
    ///    comment.
    /// 5. An online key's validity window contains the index's signing time.
    /// 6. The index has not expired at `now`, unless `allow_expired` is set.
    ///
    /// The comment and generation are checked before decompressing, so a
    /// stale or substituted index is rejected cheaply. The caller moves the
    /// downloaded files into its cache after this returns. A crash before
    /// that leaves the previous cached index, and the next sync accepts the
    /// same index again as an equal generation.
    ///
    /// # Errors
    ///
    /// Fails with the [`ErrorKind`] of the first check that rejects the
    /// index, or when a record cannot be read or written.
    pub fn accept_index(
        &self,
        repo: &Repo,
        trust: &Trust,
        index: &[u8],
        signature: &[u8],
        now: Timestamp,
        allow_expired: bool,
    ) -> Result<AcceptedIndex, Error> {
        self.accept_index_inner(repo, trust, index, signature, now, allow_expired)
            .map_err(|e| e.in_repo(&repo.name))
    }

    fn accept_index_inner(
        &self,
        repo: &Repo,
        trust: &Trust,
        bytes: &[u8],
        signature: &[u8],
        now: Timestamp,
        allow_expired: bool,
    ) -> Result<AcceptedIndex, Error> {
        let signature = Signature::decode(signature)?;
        let (key, online) = trust.signer(&signature)?;
        if !key.verifies(bytes, &signature) {
            return Err(ErrorKind::BadSignature {
                document: "index",
                signer: signature.key_id(),
            }
            .into());
        }
        let comment: IndexComment = signature.trusted_comment().parse()?;
        if comment.repo != repo.name {
            return Err(wrong_target("index", "repo", &repo.name, &comment.repo));
        }
        if comment.channel != repo.channel {
            return Err(wrong_target(
                "index",
                "channel",
                &repo.channel,
                &comment.channel,
            ));
        }
        if comment.arch != repo.arch {
            return Err(wrong_target("index", "arch", repo.arch, comment.arch));
        }
        let blake3 = Blake3Hash::of(bytes);
        let mut records = self.index_records(&repo.name)?;
        let slot = records
            .iter()
            .position(|r| r.channel == repo.channel && r.arch == repo.arch);
        if let Some(accepted) = slot.map(|i| &records[i]) {
            if comment.generation < accepted.generation {
                return Err(ErrorKind::Rollback {
                    channel: repo.channel.clone(),
                    arch: repo.arch,
                    generation: comment.generation,
                    accepted: accepted.generation,
                }
                .into());
            }
            if comment.generation == accepted.generation
                && accepted.blake3.is_some_and(|h| h != blake3)
            {
                return Err(ErrorKind::Replaced {
                    channel: repo.channel.clone(),
                    arch: repo.arch,
                    generation: comment.generation,
                }
                .into());
            }
        }
        let index = Index::decode(bytes)?;
        let content = index.comment();
        if content.repo != comment.repo {
            return Err(inconsistent("index", "repo", &comment.repo, &content.repo));
        }
        if content.channel != comment.channel {
            return Err(inconsistent(
                "index",
                "channel",
                &comment.channel,
                &content.channel,
            ));
        }
        if content.arch != comment.arch {
            return Err(inconsistent("index", "arch", comment.arch, content.arch));
        }
        if content.generation != comment.generation {
            return Err(inconsistent(
                "index",
                "generation",
                comment.generation,
                content.generation,
            ));
        }
        if let Some(online) = online
            && !(online.not_before <= index.timestamp && index.timestamp <= online.not_after)
        {
            return Err(ErrorKind::KeyNotValid {
                signer: online.id,
                timestamp: index.timestamp,
                not_before: online.not_before,
                not_after: online.not_after,
            }
            .into());
        }
        let expired = now > index.expires;
        if expired && !allow_expired {
            return Err(ErrorKind::Expired {
                generation: index.generation,
                expires: index.expires,
            }
            .into());
        }
        let record = IndexRecord {
            channel: repo.channel.clone(),
            arch: repo.arch,
            generation: index.generation,
            blake3: Some(blake3),
            signer: signature.key_id(),
        };
        match slot {
            Some(i) => records[i] = record.clone(),
            None => records.push(record.clone()),
        }
        self.write_index_records(&repo.name, records)?;
        Ok(AcceptedIndex {
            index,
            record,
            expired,
        })
    }

    /// Clears every record of `repo`, so that its next sync behaves like a
    /// first sync. This is the manual recovery for a client that cannot
    /// sync a `keys.json` that lowers a frozen generation. It never happens
    /// implicitly.
    ///
    /// # Errors
    ///
    /// Fails when a record exists but cannot be removed.
    pub fn reset(&self, repo: &Name) -> Result<(), Error> {
        let removed = remove(&self.index_path(repo))? | remove(&self.keys_path(repo))?;
        if removed {
            sync_dir(&self.dir)?;
        }
        Ok(())
    }

    fn keys_path(&self, repo: &Name) -> PathBuf {
        self.dir.join(format!("{repo}.keys.json"))
    }

    fn index_path(&self, repo: &Name) -> PathBuf {
        self.dir.join(format!("{repo}.json"))
    }

    fn write_index_records(&self, repo: &Name, mut records: Vec<IndexRecord>) -> Result<(), Error> {
        records.sort_by(|a, b| (&a.channel, a.arch).cmp(&(&b.channel, b.arch)));
        let file = IndexRecords {
            format: FORMAT,
            records,
        };
        self.write_record(&self.index_path(repo), &file)
    }

    /// Writes a record so that a crash leaves either the old file or the new
    /// one: a temporary file in the same directory is written, synced and
    /// renamed into place, then the directory is synced.
    fn write_record<T: Serialize>(&self, path: &Path, record: &T) -> Result<(), Error> {
        self.ensure_dir()?;
        // A record read back from a tampered file can hold an integer that
        // canonical JSON cannot represent.
        let bytes = laft_core::to_canonical_json(record).map_err(|e| corrupt(path, e))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let tmp = self.dir.join(format!(".{}.tmp", name.unwrap_or_default()));
        remove(&tmp)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&tmp)
            .map_err(io(&tmp))?;
        file.write_all(&bytes).map_err(io(&tmp))?;
        file.sync_all().map_err(io(&tmp))?;
        fs::rename(&tmp, path).map_err(io(path))?;
        sync_dir(&self.dir)
    }

    fn ensure_dir(&self) -> Result<(), Error> {
        if self.dir.is_dir() {
            return Ok(());
        }
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(io(&self.dir))?;
        match self.dir.parent() {
            Some(parent) => sync_dir(parent),
            None => Ok(()),
        }
    }
}

fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Error> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| corrupt(path, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path)(e)),
    }
}

/// Removes a file. Returns whether it existed.
fn remove(path: &Path) -> Result<bool, Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io(path)(e)),
    }
}

fn sync_dir(dir: &Path) -> Result<(), Error> {
    File::open(dir).and_then(|d| d.sync_all()).map_err(io(dir))
}

fn corrupt(path: &Path, reason: impl std::fmt::Display) -> Error {
    ErrorKind::CorruptState {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
    .into()
}

fn wrong_target(
    document: &'static str,
    field: &'static str,
    expected: impl std::fmt::Display,
    actual: impl std::fmt::Display,
) -> Error {
    ErrorKind::WrongTarget {
        document,
        field,
        expected: expected.to_string(),
        actual: actual.to_string(),
    }
    .into()
}

fn inconsistent(
    document: &'static str,
    field: &'static str,
    signed: impl std::fmt::Display,
    actual: impl std::fmt::Display,
) -> Error {
    ErrorKind::Inconsistent {
        document,
        field,
        signed: signed.to_string(),
        actual: actual.to_string(),
    }
    .into()
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
