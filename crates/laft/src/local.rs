//! The local repository: the packages this machine built, at
//! `/laft/local`, with the signing key `/laft/state/local.key`.
//!
//! The layout is the one of every repository: package files in
//! `pool/<id>.lpkg`, their buildinfo in `buildinfo/<id>.json`, and one
//! signed index generation per channel and architecture in
//! `channels/<channel>/<arch>/` with immutable snapshots. The key is
//! generated on the first local build and stays unencrypted, so that
//! builds never pause for a password.
//!
//! Packages are added only after they verify: the metadata and the
//! manifest are read with the streaming checks every package file gets,
//! and the store id is recomputed from the manifest. The step that adds
//! results runs as root; the builds themselves never do.

use std::fs;
use std::io::BufReader;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use laft_core::{Name, Timestamp};
use laft_index::{Index, PublicKey, Signature};
use laft_pkg::Reader;
use minisign::{KeyPair, SecretKey, SecretKeyBox};

use crate::env::{ARCH, RepoIndex};
use crate::failure::{Failure, exit};

/// The name of the repository and of the channel of its indexes.
const CHANNEL: &str = "local";

/// How many index generations of the local repository are kept.
const KEEP_SNAPSHOTS: usize = 5;

/// The suffix of a signature file.
const SIG: &str = ".minisig";

/// The local repository of a system root.
#[derive(Debug, Clone)]
pub(crate) struct Local {
    /// The repository directory.
    repo: PathBuf,
    /// The secret key file.
    key: PathBuf,
    /// The public key file.
    public: PathBuf,
}

impl Local {
    /// The local repository of the system at `root`.
    pub(crate) fn of(root: &Path) -> Self {
        Self {
            repo: root.join("laft/local"),
            key: root.join("laft/state/local.key"),
            public: root.join("laft/state/local.key.pub"),
        }
    }

    /// The public key of the repository, generated on the first call.
    /// The key file is created exclusively and never replaced, so two
    /// processes that generate at the same time agree on the key of the
    /// winner, and a key that cannot be read is never rotated.
    ///
    /// # Errors
    ///
    /// Fails when the key cannot be generated, read or written.
    pub(crate) fn public_key(&self) -> Result<PublicKey, Failure> {
        if let Some(listed) = self.read_public()? {
            return Ok(listed);
        }
        if let Some(secret) = self.read_secret()? {
            let (public, listed) = self.listed_of(&secret)?;
            self.write_public(&public)?;
            return Ok(listed);
        }
        let pair = KeyPair::generate_unencrypted_keypair()
            .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))?;
        let (public, listed) = self.listed_of(&pair.sk)?;
        let comment = format!("laft local repository key {}", listed.id());
        let boxed = pair
            .sk
            .to_box(Some(&comment))
            .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))?;
        if self.write_secret_exclusive(&boxed)? {
            self.write_public(&public)?;
            return Ok(listed);
        }
        // Another process generated the key first; use its key.
        let secret = self
            .read_secret()?
            .ok_or_else(|| self.failed("the local key does not exist yet"))?;
        let (public, listed) = self.listed_of(&secret)?;
        if self.read_public()?.is_none() {
            self.write_public(&public)?;
        }
        Ok(listed)
    }

    /// The public key that belongs to `secret`, as the index records it.
    fn listed_of(&self, secret: &SecretKey) -> Result<(minisign::PublicKey, PublicKey), Failure> {
        let public = public_of(secret)?;
        let listed: PublicKey = public
            .to_base64()
            .parse()
            .map_err(|e| self.failed(format!("{e}")))?;
        Ok((public, listed))
    }

    /// The signed index of the local repository, or `None` when nothing
    /// was built yet.
    ///
    /// # Errors
    ///
    /// Fails when the index exists but cannot be decoded.
    pub(crate) fn index(&self) -> Result<Option<Index>, Failure> {
        let bytes = match self.read_entry()? {
            Some(bytes) => bytes,
            None => return Ok(None),
        };
        Index::decode(&bytes)
            .map(Some)
            .map_err(|e| self.failed(format!("{}: {e}", self.entry().display())))
    }

    /// The local repository as a source of packages with the highest
    /// priority, when it has an index. Packages are read in place through
    /// a `file://` URL, so nothing is uploaded or copied to serve them.
    ///
    /// # Errors
    ///
    /// Fails when the index or the key cannot be used.
    pub(crate) fn source(&self) -> Result<Option<RepoIndex>, Failure> {
        let Some(bytes) = self.read_entry()? else {
            return Ok(None);
        };
        let index = Index::decode(&bytes)
            .map_err(|e| self.failed(format!("{}: {e}", self.entry().display())))?;
        let key = self.public_key()?;
        let url = format!("file://{}", self.repo.display());
        let mirrors = laft_fetch::Mirrors::new(&url, &Vec::<String>::new(), true)
            .map_err(|e| self.failed(format!("{url}: {e}")))?;
        let name = CHANNEL.parse::<Name>().expect("a valid name");
        let record = laft_index::IndexRecord {
            channel: index.channel.clone(),
            profile: None,
            arch: ARCH,
            generation: index.generation,
            blake3: Some(laft_core::Blake3Hash::of(&bytes)),
            signer: key.id(),
        };
        let synced = fs::metadata(self.entry()).and_then(|m| m.modified()).ok();
        let repo = laft_config::Repo {
            url: url.clone(),
            mirrors: Vec::new(),
            key,
            keys: Vec::new(),
            channel: index.channel.clone(),
            profile: None,
            priority: i64::MAX,
            enable: true,
            allow_http: true,
        };
        Ok(Some(RepoIndex {
            name,
            repo,
            accepted: laft_index::AcceptedIndex {
                index,
                record,
                expired: false,
            },
            mirrors,
            synced,
        }))
    }

    /// The bytes of the entry point of the local index, or `None` when
    /// nothing was built yet.
    fn read_entry(&self) -> Result<Option<Vec<u8>>, Failure> {
        read_optional(&self.entry())
    }

    /// Verifies every package file of `packages` and adds the new ones to
    /// the pool and to a new signed index generation. A package the index
    /// already lists is left as it is, and so is its file in the pool.
    ///
    /// Returns how many packages were added.
    ///
    /// # Errors
    ///
    /// Fails when a file is not a valid package, when the same package is
    /// listed with another store id, or when the index or the key cannot
    /// be used.
    pub(crate) fn add(&self, packages: &[PathBuf]) -> Result<usize, Failure> {
        let mut index = self.index()?.unwrap_or_else(empty_index);
        let mut staged = Vec::new();
        for path in packages {
            let bytes =
                fs::read(path).map_err(|e| self.failed(format!("{}: {e}", path.display())))?;
            let reader = Reader::new(BufReader::new(bytes.as_slice()))
                .map_err(|e| self.failed(format!("{}: {e}", path.display())))?;
            let meta = reader.meta().clone();
            let id = reader.manifest().store_id();
            let same = |e: &laft_index::IndexEntry| {
                let m = &e.meta;
                (&m.name, &m.version, m.rev, m.variant, &m.options)
                    == (
                        &meta.name,
                        &meta.version,
                        meta.rev,
                        meta.variant,
                        &meta.options,
                    )
            };
            match index.packages.iter().find(|e| same(e)) {
                Some(e) if e.id == id => continue,
                Some(e) => {
                    return Err(self.failed(format!(
                        "{} {} is already listed as {}, but the new build has the id {id}; \
                         publish a new revision instead",
                        meta.name, meta.version, e.id
                    )));
                }
                None => {}
            }
            staged.push((
                path.clone(),
                id,
                bytes.len(),
                laft_core::Blake3Hash::of(&bytes),
                meta,
            ));
        }
        let added = staged.len();
        for (path, id, size, blake3, meta) in &staged {
            let pool = self.pool(*id);
            fs::create_dir_all(pool.parent().expect("the pool has a parent"))
                .map_err(|e| self.failed(format!("{}: {e}", pool.display())))?;
            fs::copy(path, &pool).map_err(|e| self.failed(format!("{}: {e}", path.display())))?;
            if let Some(source) = buildinfo_of(path) {
                let target = self.repo.join(format!("buildinfo/{id}.json"));
                fs::create_dir_all(target.parent().expect("buildinfo has a parent"))
                    .map_err(|e| self.failed(format!("{}: {e}", target.display())))?;
                fs::copy(&source, &target)
                    .map_err(|e| self.failed(format!("{}: {e}", source.display())))?;
            }
            index.packages.push(laft_index::IndexEntry {
                meta: meta.clone(),
                id: *id,
                file: format!("pool/{id}.lpkg"),
                size: *size as u64,
                blake3: *blake3,
            });
        }
        if added == 0 {
            return Ok(0);
        }
        index.packages.sort_by(|a, b| {
            (&a.meta.name, &a.meta.version, a.meta.rev, a.meta.variant).cmp(&(
                &b.meta.name,
                &b.meta.version,
                b.meta.rev,
                b.meta.variant,
            ))
        });
        index.generation += 1;
        index.timestamp = Timestamp::now();
        index.expires =
            Timestamp::from_secs(index.timestamp.as_secs() + laft_index::MAX_VALIDITY_SECS);
        self.write(index)?;
        Ok(added)
    }

    /// The path of the entry point of the local index.
    fn entry(&self) -> PathBuf {
        self.repo
            .join(format!("channels/{CHANNEL}/{}/index.json.zst", ARCH))
    }

    /// The path of the package file of `id`.
    fn pool(&self, id: laft_core::StoreId) -> PathBuf {
        self.repo.join(format!("pool/{id}.lpkg"))
    }

    /// The path of the snapshot of `generation`.
    fn snapshot(&self, generation: u64) -> PathBuf {
        self.repo.join(format!(
            "channels/{CHANNEL}/{}/snapshots/{generation}.json.zst",
            ARCH
        ))
    }

    /// Writes `index` as a new generation, signs it and prunes old
    /// snapshots. The key is generated when this is the first build.
    fn write(&self, index: Index) -> Result<(), Failure> {
        self.public_key()?;
        let bytes = index
            .encode()
            .map_err(|e| self.failed(format!("the local index cannot be encoded: {e}")))?;
        let secret = self
            .read_secret()?
            .ok_or_else(|| self.failed("the local key does not exist yet"))?;
        let public = public_of(&secret)?;
        let listed: PublicKey = public
            .to_base64()
            .parse()
            .map_err(|e| self.failed(format!("{e}")))?;
        let comment = index.comment().to_string();
        let signature = minisign::sign(
            Some(&public),
            &secret,
            bytes.as_slice(),
            Some(&comment),
            Some(&format!(
                "signature from laft local repository key {}",
                listed.id()
            )),
        )
        .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))?
        .into_string()
        .into_bytes();
        let decoded = Signature::decode(&signature)
            .map_err(|e| self.failed(format!("the signature cannot be read: {e}")))?;
        if !listed.verifies(&bytes, &decoded) {
            return Err(self.failed("the signature of the local index does not verify"));
        }
        let entry = self.entry();
        let snapshot = self.snapshot(index.generation);
        write_new(&snapshot, &bytes)?;
        write_new(&signed(&snapshot), &signature)?;
        write_new(&entry, &bytes)?;
        write_new(&signed(&entry), &signature)?;
        self.prune()
    }

    /// Removes snapshots until [`KEEP_SNAPSHOTS`] remain.
    fn prune(&self) -> Result<(), Failure> {
        let dir = self
            .snapshot(0)
            .parent()
            .expect("the snapshot has a parent")
            .to_owned();
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok(());
        };
        let mut generations: Vec<u64> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_string_lossy().into_owned().parse().ok())
            .collect();
        generations.sort_unstable();
        let excess = generations.len().saturating_sub(KEEP_SNAPSHOTS);
        for old in &generations[..excess] {
            let snapshot = self.snapshot(*old);
            let _ = fs::remove_file(signed(&snapshot));
            let _ = fs::remove_file(&snapshot);
        }
        Ok(())
    }

    /// The unencrypted secret key of the repository, when it exists.
    /// Only a key that does not exist means absent; one that cannot be
    /// read fails.
    fn read_secret(&self) -> Result<Option<SecretKey>, Failure> {
        let text = match fs::read_to_string(&self.key) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(self.failed(format!("{}: {e}", self.key.display()))),
        };
        let boxed = SecretKeyBox::from_string(&text)
            .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))?;
        SecretKey::from_unencrypted_box(boxed)
            .map(Some)
            .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))
    }

    /// The public key listed in the `.pub` file, when it exists. Only a
    /// file that does not exist means absent; one that cannot be read
    /// fails.
    fn read_public(&self) -> Result<Option<PublicKey>, Failure> {
        let text = match fs::read_to_string(&self.public) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(self.failed(format!("{}: {e}", self.public.display()))),
        };
        let line = text
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim();
        line.parse()
            .map(Some)
            .map_err(|e| self.failed(format!("{}: {e}", self.public.display())))
    }

    /// Writes the public key box of `public` to the `.pub` file.
    fn write_public(&self, public: &minisign::PublicKey) -> Result<(), Failure> {
        let bytes = public
            .to_box()
            .map_err(|e| self.failed(format!("{}: {e}", self.public.display())))?
            .into_string()
            .into_bytes();
        write_new(&self.public, &bytes)?;
        Ok(())
    }

    /// Writes the secret key box to the key file, unless the file exists
    /// already, and says whether it wrote it. The key file appears
    /// complete or not at all, readable only by the owner, and an
    /// existing key is never replaced.
    fn write_secret_exclusive(&self, boxed: &SecretKeyBox) -> Result<bool, Failure> {
        let bytes = boxed.to_bytes();
        if let Some(dir) = self.key.parent() {
            fs::create_dir_all(dir).map_err(|e| self.failed(format!("{}: {e}", dir.display())))?;
        }
        let tmp = self.key.with_file_name(format!(
            ".{}.laft-tmp",
            self.key
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        ));
        let write = || -> std::io::Result<()> {
            {
                let mut file = fs::File::create(&tmp)?;
                std::io::Write::write_all(&mut file, &bytes)?;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
                file.sync_all()?;
            }
            fs::hard_link(&tmp, &self.key)?;
            Ok(())
        };
        let linked = write();
        let _ = fs::remove_file(&tmp);
        match linked {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(self.failed(format!("{}: {e}", self.key.display()))),
        }
    }

    /// A [`Failure`] about the local repository.
    fn failed(&self, message: impl Into<String>) -> Failure {
        Failure::new(exit::FAILURE, message.into())
    }
}

/// The minisign public key that belongs to `secret`.
fn public_of(secret: &SecretKey) -> Result<minisign::PublicKey, Failure> {
    minisign::PublicKey::from_secret_key(secret)
        .map_err(|e| Failure::new(exit::FAILURE, format!("the local key: {e}")))
}

/// The buildinfo file written next to the package `path`, when it exists.
fn buildinfo_of(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let buildinfo = path.with_file_name(format!("{name}.buildinfo.json"));
    buildinfo.is_file().then_some(buildinfo)
}

/// The path of the signature file of `path`.
fn signed(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}{SIG}", path.display()))
}

/// The index of a repository that has nothing in it yet.
fn empty_index() -> Index {
    let name = CHANNEL.parse::<Name>().expect("a valid name");
    Index {
        format: Index::FORMAT,
        repo: name.clone(),
        channel: name,
        arch: ARCH,
        generation: 0,
        timestamp: Timestamp::now(),
        expires: Timestamp::now(),
        defaults: Default::default(),
        packages: Vec::new(),
        yanked: Vec::new(),
        options: Default::default(),
        recipes: None,
        profile: None,
    }
}

/// Writes `bytes` to `path`, which appears complete or not at all, with
/// mode `0o644`.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    write_file(path, bytes, 0o644)
}

/// Writes `bytes` to `path` with the mode `mode`, creating its parent
/// directories.
fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), Failure> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|e| Failure::new(exit::FAILURE, format!("{}: {e}", dir.display())))?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.laft-tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
    ));
    let write = || -> std::io::Result<()> {
        {
            let mut file = fs::File::create(&tmp)?;
            std::io::Write::write_all(&mut file, bytes)?;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
            file.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Failure::new(exit::FAILURE, format!("{}: {e}", path.display()))
    })
}

/// Reads `path`, or `None` when it does not exist.
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, Failure> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Failure::new(
            exit::FAILURE,
            format!("{}: {e}", path.display()),
        )),
    }
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
