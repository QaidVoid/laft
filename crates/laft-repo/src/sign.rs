//! Minisign keys and signatures.
//!
//! Secret keys are minisign key files. An encrypted key is decrypted with
//! a password the caller passes in, or with one typed at the terminal. An
//! unencrypted key is used as it is.

use std::fs;
use std::path::Path;

use laft_index::{KeyId, PublicKey, Signature};
use minisign::{KeyPair, SecretKey, SecretKeyBox};

use crate::error::{Error, Result, io};
use crate::fsx;

/// The environment variable that holds the password of an encrypted
/// secret key unless another one is named.
pub(crate) const PASSWORD_ENV: &str = "LAFT_SIGNING_KEY_PASSWORD";

/// A secret key that can sign.
pub(crate) struct Signer {
    pk: minisign::PublicKey,
    sk: SecretKey,
    public: PublicKey,
}

impl Signer {
    /// Loads the secret key file `path`. An encrypted key is decrypted with
    /// `password`, or with a password read from the terminal when it is
    /// `None`.
    pub(crate) fn load(path: &Path, password: Option<String>) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(io(path))?;
        let boxed = SecretKeyBox::from_string(&text)?;
        let sk = match SecretKey::from_unencrypted_box(boxed.clone()) {
            Ok(sk) => sk,
            Err(_) => SecretKey::from_box(boxed, password)
                .map_err(|e| Error::new(format!("{}: {e}", path.display())))?,
        };
        let pk = minisign::PublicKey::from_secret_key(&sk)?;
        let public = pk.to_base64().parse()?;
        Ok(Self { pk, sk, public })
    }

    /// The id of the key.
    pub(crate) fn id(&self) -> KeyId {
        self.public.id()
    }

    /// Signs `data` with the trusted comment `comment` and returns the
    /// bytes of the signature file. The signature is verified before it is
    /// returned.
    pub(crate) fn sign(&self, data: &[u8], comment: &str) -> Result<Vec<u8>> {
        let untrusted = format!("signature from laft-repo key {}", self.id());
        let signature = minisign::sign(
            Some(&self.pk),
            &self.sk,
            data,
            Some(comment),
            Some(&untrusted),
        )?
        .into_string()
        .into_bytes();
        let decoded = Signature::decode(&signature)?;
        if !self.public.verifies(data, &decoded) || decoded.trusted_comment() != comment {
            return Err(Error::new(format!(
                "the signature made with key {} does not verify",
                self.id()
            )));
        }
        Ok(signature)
    }
}

/// Creates a key pair: the secret key `<prefix>.key` and the public key
/// `<prefix>.pub`. The secret key is encrypted with `password`, or with a
/// password typed twice at the terminal when it is `None`, unless `encrypt`
/// is false. Existing files are never overwritten.
pub(crate) fn keygen(prefix: &Path, encrypt: bool, password: Option<String>) -> Result<KeyId> {
    let secret = prefix.with_extension("key");
    let public = prefix.with_extension("pub");
    for path in [&secret, &public] {
        if fsx::size(path)?.is_some() {
            return Err(Error::new(format!("{} already exists", path.display())));
        }
    }
    let pair = if encrypt {
        KeyPair::generate_encrypted_keypair(password)?
    } else {
        KeyPair::generate_unencrypted_keypair()?
    };
    let id: PublicKey = pair.pk.to_base64().parse()?;
    let comment = format!("laft-repo secret key {}", id.id());
    fsx::write_atomic(&secret, &pair.sk.to_box(Some(&comment))?.to_bytes(), 0o600)?;
    fsx::write_atomic(&public, &pair.pk.to_box()?.to_bytes(), fsx::FILE_MODE)?;
    Ok(id.id())
}

/// Reads a public key: a minisign `.pub` file, whose last line is the key.
pub(crate) fn read_public(path: &Path) -> Result<PublicKey> {
    let text = fs::read_to_string(path).map_err(io(path))?;
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    line.trim()
        .parse()
        .map_err(|e| Error::new(format!("{}: {e}", path.display())))
}

#[cfg(test)]
#[path = "sign_tests.rs"]
mod tests;
