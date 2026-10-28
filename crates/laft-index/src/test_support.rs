//! Keys, signed documents and sample indexes for tests.

use laft_core::{Blake3Hash, StoreId, Timestamp};
use laft_pkg::{Arch, Meta};

use crate::{Defaults, Index, IndexEntry, KeyId, Keys, OnlineKey, PublicKey, Repo};

/// A signing time all sample documents are built around.
pub(crate) const T0: u64 = 1_761_652_800;
pub(crate) const DAY: u64 = 86_400;

/// Public keys of a few generated key pairs, for property tests that need
/// valid keys but sign nothing.
pub(crate) fn public_keys() -> &'static [PublicKey] {
    static KEYS: std::sync::LazyLock<Vec<PublicKey>> =
        std::sync::LazyLock::new(|| (0..4).map(|_| TestKey::generate().public).collect());
    &KEYS
}

/// A minisign key pair.
pub(crate) struct TestKey {
    pub(crate) public: PublicKey,
    pk: minisign::PublicKey,
    sk: minisign::SecretKey,
}

/// A document and its signature file.
pub(crate) struct Signed {
    pub(crate) bytes: Vec<u8>,
    pub(crate) signature: Vec<u8>,
}

impl TestKey {
    pub(crate) fn generate() -> Self {
        let pair =
            minisign::KeyPair::generate_unencrypted_keypair().expect("keys can be generated");
        Self {
            public: pair
                .pk
                .to_base64()
                .parse()
                .expect("minisign writes valid public keys"),
            pk: pair.pk,
            sk: pair.sk,
        }
    }

    pub(crate) fn id(&self) -> KeyId {
        self.public.id()
    }

    pub(crate) fn sign(&self, data: &[u8], comment: &str) -> Vec<u8> {
        minisign::sign(Some(&self.pk), &self.sk, data, Some(comment), None)
            .expect("an unencrypted key can sign")
            .into_string()
            .into_bytes()
    }

    pub(crate) fn sign_index(&self, index: &Index) -> Signed {
        let bytes = index.encode().expect("sample indexes are valid");
        let signature = self.sign(&bytes, &index.comment().to_string());
        Signed { bytes, signature }
    }

    pub(crate) fn sign_keys(&self, keys: &Keys) -> Signed {
        let bytes = keys.to_json().expect("sample keys are valid");
        let signature = self.sign(&bytes, &keys.comment().to_string());
        Signed { bytes, signature }
    }

    /// The key as an online key valid for a year around [`T0`].
    pub(crate) fn online(&self) -> OnlineKey {
        OnlineKey {
            id: self.id(),
            public: self.public.clone(),
            not_before: Timestamp::from_secs(T0 - 365 * DAY),
            not_after: Timestamp::from_secs(T0 + 365 * DAY),
        }
    }
}

pub(crate) fn meta(name: &str, version: &str) -> Meta {
    Meta {
        format: laft_pkg::FORMAT,
        name: name.parse().expect("valid name"),
        version: version.parse().expect("valid version"),
        rev: 1,
        variant: Default::default(),
        arch: Arch::X86_64V3,
        origin: name.parse().expect("valid name"),
        summary: format!("The {name} package"),
        homepage: None,
        license: vec!["MIT".into()],
        depends: vec![],
        provides: vec![],
        conflicts: vec![],
        replaces: vec![],
        alternatives: vec![],
        triggers: vec![],
        static_deps: vec![],
        abi: Default::default(),
        abi_epoch: 0,
        requested_privileges: vec![],
        installed_size: 10,
    }
}

pub(crate) fn entry(name: &str, version: &str) -> IndexEntry {
    let id = StoreId::from_manifest_hash(&Blake3Hash::of(format!("{name}-{version}").as_bytes()));
    IndexEntry {
        meta: meta(name, version),
        id,
        file: format!("pool/{id}.lpkg"),
        size: 1000,
        blake3: Blake3Hash::of(id.to_string().as_bytes()),
    }
}

/// An index of `core`, channel `current`, signed `generation` seconds after
/// the epoch and valid for 14 days.
pub(crate) fn index(generation: u64) -> Index {
    Index {
        format: Index::FORMAT,
        repo: "core".parse().expect("valid name"),
        channel: "current".parse().expect("valid name"),
        arch: Arch::X86_64V3,
        generation,
        timestamp: Timestamp::from_secs(generation),
        expires: Timestamp::from_secs(generation + 14 * DAY),
        defaults: Defaults::default(),
        packages: vec![entry("hello", "1.0"), entry("zlib", "1.3.1")],
        yanked: vec![],
    }
}

pub(crate) fn keys(version: u64, keys: Vec<OnlineKey>, revoked: Vec<KeyId>) -> Keys {
    Keys {
        format: Keys::FORMAT,
        repo: "core".parse().expect("valid name"),
        version,
        keys,
        revoked,
        generation_floor: None,
    }
}

/// The configuration of `core` on channel `current` with root key `root`.
pub(crate) fn repo(root: &TestKey) -> Repo {
    Repo {
        name: "core".parse().expect("valid name"),
        channel: "current".parse().expect("valid name"),
        arch: Arch::X86_64V3,
        key: root.public.clone(),
        keys: vec![],
    }
}

/// A moment while every sample index of generation around [`T0`] is valid.
pub(crate) fn now() -> Timestamp {
    Timestamp::from_secs(T0 + DAY)
}
