//! The local repository: the packages this machine built, at
//! `/laft/local`, with the signing key `/laft/state/local.key`.
//!
//! The layout is the one of every repository: package files in
//! `pool/<id>.lpkg`, their buildinfo in `buildinfo/<id>.json`, and one
//! signed index generation per channel and architecture in
//! `channels/<channel>/<arch>/` with immutable snapshots. The key is
//! generated on the first local build and stays unencrypted, so that
//! builds never pause for a password.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use laft_core::Name;
use laft_index::{Index, PublicKey};
use minisign::{KeyPair, SecretKey, SecretKeyBox};

use crate::env::{ARCH, RepoIndex};
use crate::failure::{Failure, exit};

/// The name of the repository and of the channel of its indexes.
const CHANNEL: &str = "local";

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
    ///
    /// # Errors
    ///
    /// Fails when the key cannot be generated, read or written.
    pub(crate) fn public_key(&self) -> Result<PublicKey, Failure> {
        if let Some(listed) = self.read_public()? {
            return Ok(listed);
        }
        if let Some(secret) = self.read_secret()? {
            let public = public_of(&secret)?;
            self.write_public(&public)?;
            let listed: PublicKey = public
                .to_base64()
                .parse()
                .map_err(|e| self.failed(format!("{e}")))?;
            return Ok(listed);
        }
        let pair = KeyPair::generate_unencrypted_keypair()
            .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))?;
        let public = public_of(&pair.sk)?;
        let listed: PublicKey = public
            .to_base64()
            .parse()
            .map_err(|e| self.failed(format!("{e}")))?;
        let comment = format!("laft local repository key {}", listed.id());
        let boxed = pair
            .sk
            .to_box(Some(&comment))
            .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))?;
        self.write_secret(&boxed)?;
        self.write_public(&public)?;
        Ok(listed)
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

    /// The path of the entry point of the local index.
    fn entry(&self) -> PathBuf {
        self.repo
            .join(format!("channels/{CHANNEL}/{}/index.json.zst", ARCH))
    }

    /// The unencrypted secret key of the repository, when it exists.
    fn read_secret(&self) -> Result<Option<SecretKey>, Failure> {
        let Ok(text) = fs::read_to_string(&self.key) else {
            return Ok(None);
        };
        let boxed = SecretKeyBox::from_string(&text)
            .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))?;
        SecretKey::from_unencrypted_box(boxed)
            .map(Some)
            .map_err(|e| self.failed(format!("{}: {e}", self.key.display())))
    }

    /// The public key listed in the `.pub` file, when it exists.
    fn read_public(&self) -> Result<Option<PublicKey>, Failure> {
        let Ok(text) = fs::read_to_string(&self.public) else {
            return Ok(None);
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

    /// Writes the secret key box to the key file, readable only by the
    /// owner.
    fn write_secret(&self, boxed: &SecretKeyBox) -> Result<(), Failure> {
        write_file(&self.key, &boxed.to_bytes(), 0o600)
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
