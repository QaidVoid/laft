use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

use laft_core::{Blake3Hash, Name, Provide, StoreId, Timestamp, Variant, Version};
use laft_pkg::{Arch, Meta};
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::{Error, ErrorKind, IndexComment, MAX_SAFE_INTEGER};

/// The largest index a client decompresses: 256 MiB.
pub const MAX_INDEX_SIZE: u64 = 256 << 20;

/// The longest time from signing an index to its expiry: 30 days.
pub const MAX_VALIDITY_SECS: u64 = 30 * 86_400;

/// The largest zstd window an index may need: 128 MiB.
const MAX_WINDOW_LOG: u32 = 27;

/// The zstd level indexes are compressed with. An index is compressed once
/// and downloaded by every client, so compression time matters little.
const LEVEL: i32 = 19;

/// The package list of one channel and architecture of a repository.
///
/// It is published as `index.json.zst`, zstd-compressed canonical JSON
/// (RFC 8785), and signed with minisign. [`Self::encode`] produces the
/// published bytes and [`Self::comment`] the trusted comment to sign them
/// with. Fields are public so that a publisher can fill them in.
/// [`Self::validate`] checks every rule, and encoding and decoding always
/// validate.
///
/// ```
/// use laft_core::Timestamp;
/// use laft_index::{Defaults, Index};
///
/// let signed = Timestamp::from_secs(1_761_652_800);
/// let index = Index {
///     format: Index::FORMAT,
///     repo: "core".parse()?,
///     channel: "current".parse()?,
///     arch: laft_pkg::Arch::X86_64V3,
///     generation: laft_index::next_generation(signed, None)?,
///     timestamp: signed,
///     expires: Timestamp::from_secs(signed.as_secs() + 14 * 86_400),
///     defaults: Defaults::default(),
///     packages: vec![],
///     yanked: vec![],
/// };
/// let bytes = index.encode()?;
/// assert_eq!(Index::decode(&bytes)?, index);
/// assert_eq!(
///     index.comment().to_string(),
///     "laft-index repo=core channel=current arch=x86_64-v3 generation=1761652800"
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    /// The format version, always [`Self::FORMAT`].
    pub format: u32,
    /// The repository name.
    pub repo: Name,
    /// The channel, for example `current`.
    pub channel: Name,
    /// The architecture of the packages.
    pub arch: Arch,
    /// Strictly increasing for every newly signed index of the same
    /// repository, channel and architecture. At most 2^53.
    pub generation: u64,
    /// When the index was signed.
    pub timestamp: Timestamp,
    /// When clients stop accepting the index. Later than `timestamp` and at
    /// most [`MAX_VALIDITY_SECS`] after it.
    pub expires: Timestamp,
    /// Repository defaults for alternatives and providers.
    pub defaults: Defaults,
    /// The available packages, by name ascending, then by version and
    /// revision descending, then dynamic before static.
    pub packages: Vec<IndexEntry>,
    /// Withdrawn packages, in the order of `packages` and without
    /// duplicates. None has the store id or the name, version, revision and
    /// variant of an available package.
    pub yanked: Vec<Yank>,
}

/// Repository defaults that resolve choices between packages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    /// The default member of each alternatives group, by group.
    pub alternatives: BTreeMap<Name, Name>,
    /// The preferred package when several provide the same capability, by
    /// the capability's text form, for example `so:libjpeg.so.8`.
    pub providers: BTreeMap<String, Name>,
}

/// A package in an index: its metadata and where its file is.
///
/// It is written as one JSON object holding the metadata fields and the
/// four fields of the entry itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// The package metadata, as in the package file.
    pub meta: Meta,
    /// The store id.
    pub id: StoreId,
    /// The path of the package file relative to the repository base,
    /// always `pool/<id>.lpkg`.
    pub file: String,
    /// The size of the package file in bytes.
    pub size: u64,
    /// The BLAKE3 hash of the package file.
    pub blake3: Blake3Hash,
}

/// A withdrawn package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Yank {
    /// The store id of the withdrawn package.
    pub id: StoreId,
    /// The package name.
    pub name: Name,
    /// The upstream version.
    pub version: Version,
    /// The revision, starting at 1.
    pub rev: u32,
    /// The variant.
    pub variant: Variant,
    /// Why it was withdrawn, one line shown to users.
    pub reason: String,
    /// When it was withdrawn.
    pub time: Timestamp,
}

impl Index {
    /// The index format version this crate reads and writes.
    pub const FORMAT: u32 = 1;

    /// The trusted comment to sign the encoded index with.
    pub fn comment(&self) -> IndexComment {
        IndexComment {
            repo: self.repo.clone(),
            channel: self.channel.clone(),
            arch: self.arch,
            generation: self.generation,
        }
    }

    /// Checks every rule of the format.
    ///
    /// # Errors
    ///
    /// Fails, naming the rule, when one is broken.
    pub fn validate(&self) -> Result<(), Error> {
        let err = |reason: String| Error::invalid("index", reason);
        if self.format != Self::FORMAT {
            return Err(err(format!(
                "format {} is not supported, only {} is",
                self.format,
                Self::FORMAT
            )));
        }
        if self.generation > MAX_SAFE_INTEGER {
            return Err(err("`generation` is at most 2^53".into()));
        }
        if self.expires <= self.timestamp
            || self.expires.as_secs() - self.timestamp.as_secs() > MAX_VALIDITY_SECS
        {
            return Err(err(
                "`expires` is later than `timestamp` and at most 30 days after it".into(),
            ));
        }
        for provide in self.defaults.providers.keys() {
            provide.parse::<Provide>().map_err(|e| err(e.to_string()))?;
        }
        let mut ids = BTreeSet::new();
        let mut identities = BTreeSet::new();
        for entry in &self.packages {
            let package = entry.meta.identity();
            entry.meta.validate().map_err(|e| err(e.to_string()))?;
            if entry.meta.arch != self.arch && entry.meta.arch != Arch::Any {
                return Err(err(format!(
                    "package {package} is for {}, not {}",
                    entry.meta.arch, self.arch
                )));
            }
            if entry.file != format!("pool/{}.lpkg", entry.id) {
                return Err(err(format!(
                    "package {package} has file {:?} instead of pool/{}.lpkg",
                    entry.file, entry.id
                )));
            }
            if entry.size > MAX_SAFE_INTEGER {
                return Err(err(format!("package {package} has a size above 2^53")));
            }
            if !ids.insert(entry.id) {
                return Err(err(format!("store id {} is listed twice", entry.id)));
            }
            let m = &entry.meta;
            identities.insert((&m.name, &m.version, m.rev, m.variant));
        }
        fn order(e: &IndexEntry) -> (&Name, Reverse<(&Version, u32)>, Variant) {
            let m = &e.meta;
            (&m.name, Reverse((&m.version, m.rev)), m.variant)
        }
        fn yank_order(y: &Yank) -> (&Name, Reverse<(&Version, u32)>, Variant) {
            (&y.name, Reverse((&y.version, y.rev)), y.variant)
        }
        if self
            .packages
            .windows(2)
            .any(|pair| order(&pair[0]) >= order(&pair[1]))
        {
            return Err(err(
                "`packages` must be sorted by name, then by version and revision descending, then by variant, without duplicates".into(),
            ));
        }
        for yank in &self.yanked {
            if yank.rev == 0 {
                return Err(err(format!("yanked package {} has revision 0", yank.name)));
            }
            if yank.reason.is_empty() || yank.reason.chars().any(char::is_control) {
                return Err(err(format!(
                    "yanked package {} needs a reason of one line",
                    yank.name
                )));
            }
            if !ids.insert(yank.id) {
                return Err(err(format!(
                    "store id {} is yanked twice or both available and yanked",
                    yank.id
                )));
            }
            if identities.contains(&(&yank.name, &yank.version, yank.rev, yank.variant)) {
                return Err(err(format!(
                    "yanked package {} {}-r{} is also available",
                    yank.name, yank.version, yank.rev
                )));
            }
        }
        if self
            .yanked
            .windows(2)
            .any(|pair| yank_order(&pair[0]) >= yank_order(&pair[1]))
        {
            return Err(err(
                "`yanked` must be sorted like `packages`, without duplicates".into(),
            ));
        }
        Ok(())
    }

    /// The index as canonical JSON (RFC 8785), before compression.
    ///
    /// # Errors
    ///
    /// Fails when the index is not valid.
    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        Ok(laft_core::to_canonical_json(self)
            .expect("a valid index has no integer above 2^53, so it is canonical JSON"))
    }

    /// Parses an index from uncompressed JSON and validates it.
    ///
    /// # Errors
    ///
    /// Fails when `bytes` are not the JSON of a valid index.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let index: Self = serde_json::from_slice(bytes).map_err(|e| Error::invalid("index", e))?;
        index.validate()?;
        Ok(index)
    }

    /// The published form: canonical JSON compressed with zstd, with the
    /// frame checksum set.
    ///
    /// # Errors
    ///
    /// Fails when the index is not valid.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let json = self.to_json()?;
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), LEVEL)
            .and_then(|mut e| e.include_checksum(true).map(|()| e))
            .expect("the compression level and checksum flag are valid parameters");
        encoder
            .write_all(&json)
            .expect("compressing into memory cannot fail");
        Ok(encoder
            .finish()
            .expect("compressing into memory cannot fail"))
    }

    /// Decompresses and parses a published index.
    ///
    /// Decompression stops at [`MAX_INDEX_SIZE`] bytes and refuses frames
    /// that need a window above 128 MiB, so a hostile file cannot exhaust
    /// memory.
    ///
    /// # Errors
    ///
    /// Fails when `bytes` are not zstd data, decompress to more than
    /// [`MAX_INDEX_SIZE`] bytes, or are not a valid index.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_json(&decompress(bytes, MAX_INDEX_SIZE)?)
    }
}

/// Decompresses zstd data that may expand to at most `limit` bytes.
fn decompress(bytes: &[u8], limit: u64) -> Result<Vec<u8>, Error> {
    let err = |e: std::io::Error| Error::invalid("index", format!("cannot decompress: {e}"));
    let mut decoder = zstd::stream::read::Decoder::with_buffer(bytes).map_err(err)?;
    decoder.window_log_max(MAX_WINDOW_LOG).map_err(err)?;
    let mut out = Vec::new();
    decoder.take(limit + 1).read_to_end(&mut out).map_err(err)?;
    if out.len() as u64 > limit {
        return Err(ErrorKind::TooLarge { limit }.into());
    }
    Ok(out)
}

/// The generation of an index signed at `signed`: the signing time in
/// seconds since the Unix epoch.
///
/// # Errors
///
/// Fails when the result is not greater than `last`, the last published
/// generation, for example because the clock went backwards or two indexes
/// are signed within one second, or when it exceeds 2^53.
pub fn next_generation(signed: Timestamp, last: Option<u64>) -> Result<u64, Error> {
    let generation = signed.as_secs();
    if last.is_some_and(|last| generation <= last) {
        return Err(Error::invalid(
            "index",
            format!(
                "generation {generation} from the signing time is not greater than the last published generation {}",
                last.unwrap_or_default()
            ),
        ));
    }
    if generation > MAX_SAFE_INTEGER {
        return Err(Error::invalid("index", "`generation` is at most 2^53"));
    }
    Ok(generation)
}

const ENTRY_FIELDS: [&str; 4] = ["id", "file", "size", "blake3"];

impl Serialize for IndexEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Value::Object(mut map) = serde_json::to_value(&self.meta).map_err(S::Error::custom)?
        else {
            return Err(S::Error::custom("package metadata is not a JSON object"));
        };
        let fields = [
            serde_json::to_value(self.id),
            serde_json::to_value(&self.file),
            serde_json::to_value(self.size),
            serde_json::to_value(self.blake3),
        ];
        for (name, value) in ENTRY_FIELDS.into_iter().zip(fields) {
            map.insert(name.to_owned(), value.map_err(S::Error::custom)?);
        }
        map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IndexEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map = Map::deserialize(deserializer)?;
        let mut take = |name: &'static str| {
            map.remove(name)
                .ok_or_else(|| D::Error::missing_field(name))
        };
        let id = take("id")?;
        let file = take("file")?;
        let size = take("size")?;
        let blake3 = take("blake3")?;
        fn field<T: serde::de::DeserializeOwned, E: serde::de::Error>(
            value: Value,
        ) -> Result<T, E> {
            serde_json::from_value(value).map_err(E::custom)
        }
        Ok(Self {
            id: field::<_, D::Error>(id)?,
            file: field::<_, D::Error>(file)?,
            size: field::<_, D::Error>(size)?,
            blake3: field::<_, D::Error>(blake3)?,
            meta: field::<_, D::Error>(Value::Object(map))?,
        })
    }
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
