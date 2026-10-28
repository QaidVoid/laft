//! A local HTTP server, signed sample repositories and packages for tests.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use laft_core::{Blake3Hash, StoreId, Timestamp};
use laft_index::{Defaults, Index, IndexEntry, Keys, OnlineKey, PublicKey, Repo};
use laft_pkg::{Arch, Meta};

use crate::Config;

/// A signing time all sample documents are built around.
pub(crate) const T0: u64 = 1_761_652_800;
pub(crate) const DAY: u64 = 86_400;

/// What the server does with the next request for a path instead of
/// answering normally.
#[derive(Debug, Clone)]
pub(crate) enum Fault {
    /// Answers with this status and no body.
    Status(u16),
    /// Announces the whole file but closes the connection after this many
    /// bytes of it.
    Truncate(usize),
    /// Sends nothing for this long, then closes the connection.
    Stall(Duration),
    /// Answers with the whole file even when a range was asked for.
    IgnoreRange,
    /// Announces this `Content-Length` but sends nothing of the body.
    Length(u64),
}

/// A request the server received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Logged {
    pub(crate) path: String,
    pub(crate) range: Option<String>,
}

#[derive(Default)]
struct State {
    files: BTreeMap<String, Vec<u8>>,
    faults: BTreeMap<String, VecDeque<Fault>>,
    log: Vec<Logged>,
}

/// An HTTP/1.1 server on 127.0.0.1 that serves files from memory, honors
/// `Range: bytes=<n>-` and closes every connection after one response.
pub(crate) struct Server {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl Server {
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port is free");
        let addr = listener
            .local_addr()
            .expect("a bound socket has an address");
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                thread::spawn(move || handle(stream, &state));
            }
        });
        Self { addr, state }
    }

    /// The base URL of the server's root.
    pub(crate) fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub(crate) fn put_all(&self, files: &BTreeMap<String, Vec<u8>>) {
        self.lock().files.extend(files.clone());
    }

    pub(crate) fn put(&self, path: &str, bytes: &[u8]) {
        self.lock().files.insert(path.into(), bytes.to_vec());
    }

    pub(crate) fn remove(&self, path: &str) {
        self.lock().files.remove(path);
    }

    /// Queues `fault` for the next request of `path` that has no fault
    /// queued before it.
    pub(crate) fn fault(&self, path: &str, fault: Fault) {
        self.lock()
            .faults
            .entry(path.into())
            .or_default()
            .push_back(fault);
    }

    /// The requests received so far for `path`.
    pub(crate) fn requests(&self, path: &str) -> Vec<Logged> {
        self.lock()
            .log
            .iter()
            .filter(|l| l.path == path)
            .cloned()
            .collect()
    }

    /// The paths of every request so far, in the order they arrived.
    pub(crate) fn paths(&self) -> Vec<String> {
        self.lock().log.iter().map(|l| l.path.clone()).collect()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("the server never panics holding the lock")
    }
}

fn handle(stream: TcpStream, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream.try_clone().expect("sockets can be cloned"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_owned();
    let mut range = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_owned());
        }
    }
    let (file, fault) = {
        let mut state = state
            .lock()
            .expect("the server never panics holding the lock");
        state.log.push(Logged {
            path: path.clone(),
            range: range.clone(),
        });
        let fault = state.faults.get_mut(&path).and_then(VecDeque::pop_front);
        (state.files.get(&path).cloned(), fault)
    };
    let mut stream = stream;
    let (file, truncate, announced) = match fault {
        Some(Fault::Status(code)) => return respond(&mut stream, code, &[], 0, &[]),
        Some(Fault::Stall(time)) => return thread::sleep(time),
        Some(Fault::IgnoreRange) => {
            range = None;
            (file, None, None)
        }
        Some(Fault::Truncate(n)) => (file, Some(n), None),
        Some(Fault::Length(n)) => (file, Some(0), Some(n)),
        None => (file, None, None),
    };
    let Some(file) = file else {
        return respond(&mut stream, 404, &[], 0, &[]);
    };
    let start = range
        .as_deref()
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|n| n.parse::<usize>().ok());
    let (code, body, headers) = match start {
        Some(start) if start >= file.len() => (416, &[][..], vec![]),
        Some(start) => {
            let content_range = format!("bytes {start}-{}/{}", file.len() - 1, file.len());
            (206, &file[start..], vec![("Content-Range", content_range)])
        }
        None => (200, &file[..], vec![]),
    };
    let sent = truncate.unwrap_or(body.len()).min(body.len());
    let length = announced.unwrap_or(body.len() as u64);
    respond(&mut stream, code, &headers, length, &body[..sent]);
}

/// Writes a response that announces `length` bytes of body and sends
/// `body`, which may be shorter.
fn respond(
    stream: &mut TcpStream,
    code: u16,
    headers: &[(&str, String)],
    length: u64,
    body: &[u8],
) {
    let mut head =
        format!("HTTP/1.1 {code} X\r\nContent-Length: {length}\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

/// A loopback URL that refuses connections.
pub(crate) fn refusing_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port is free");
    let addr = listener
        .local_addr()
        .expect("a bound socket has an address");
    drop(listener);
    format!("http://{addr}")
}

/// A configuration with short timeouts and pauses, caching in `cache`.
pub(crate) fn config(cache: &Path) -> Config {
    Config {
        cache: cache.to_path_buf(),
        offline: false,
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_millis(500),
        retries: 2,
        retry_delay: Duration::from_millis(1),
    }
}

/// A minisign key pair.
pub(crate) struct TestKey {
    pub(crate) public: PublicKey,
    pk: minisign::PublicKey,
    sk: minisign::SecretKey,
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

    pub(crate) fn sign(&self, data: &[u8], comment: &str) -> Vec<u8> {
        minisign::sign(Some(&self.pk), &self.sk, data, Some(comment), None)
            .expect("an unencrypted key can sign")
            .into_string()
            .into_bytes()
    }

    /// The key as an online key valid for a year around [`T0`].
    pub(crate) fn online(&self) -> OnlineKey {
        OnlineKey {
            id: self.public.id(),
            public: self.public.clone(),
            not_before: Timestamp::from_secs(T0 - 365 * DAY),
            not_after: Timestamp::from_secs(T0 + 365 * DAY),
        }
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

/// The directory of the sample channel below the repository base.
pub(crate) const CHANNEL: &str = "channels/current/x86_64-v3";

/// An empty index of `core`, signed `generation` seconds after the epoch
/// and valid for 14 days.
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
        packages: vec![],
        yanked: vec![],
    }
}

/// The files of a repository publishing `index`. With `online`, a
/// `keys.json` signed by `root` lists that key, which signs the index.
/// Without, the repository runs in direct mode.
pub(crate) fn publish(
    root: &TestKey,
    online: Option<&TestKey>,
    index: &Index,
) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let signer = match online {
        Some(key) => {
            let keys = Keys {
                format: Keys::FORMAT,
                repo: "core".parse().expect("valid name"),
                version: 1,
                keys: vec![key.online()],
                revoked: vec![],
                generation_floor: None,
            };
            let json = keys.to_json().expect("sample keys are valid");
            let signature = root.sign(&json, &keys.comment().to_string());
            files.insert("keys.json".into(), json);
            files.insert("keys.json.minisig".into(), signature);
            key
        }
        None => root,
    };
    let bytes = index.encode().expect("sample indexes are valid");
    let signature = signer.sign(&bytes, &index.comment().to_string());
    let snapshot = format!("{CHANNEL}/snapshots/{}.json.zst", index.generation);
    files.insert(format!("{snapshot}.minisig"), signature.clone());
    files.insert(snapshot, bytes.clone());
    files.insert(format!("{CHANNEL}/index.json.zst.minisig"), signature);
    files.insert(format!("{CHANNEL}/index.json.zst"), bytes);
    files
}

/// Writes `files` below `dir`.
pub(crate) fn write_tree(dir: &Path, files: &BTreeMap<String, Vec<u8>>) {
    for (path, bytes) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("files are in a directory"))
            .expect("the test directory is writable");
        std::fs::write(path, bytes).expect("the test directory is writable");
    }
}

/// An index entry for a package file with `content`. The bytes need not be
/// a real package, because only their size and hash are checked here.
pub(crate) fn package(content: &[u8]) -> IndexEntry {
    let id = StoreId::from_manifest_hash(&Blake3Hash::of(content));
    IndexEntry {
        meta: Meta {
            format: laft_pkg::FORMAT,
            name: "hello".parse().expect("valid name"),
            version: "1.0".parse().expect("valid version"),
            rev: 1,
            variant: Default::default(),
            arch: Arch::X86_64V3,
            origin: "hello".parse().expect("valid name"),
            summary: "A sample package".into(),
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
        },
        id,
        file: format!("pool/{id}.lpkg"),
        size: content.len() as u64,
        blake3: Blake3Hash::of(content),
    }
}

/// Bytes that do not compress or repeat, `len` of them.
pub(crate) fn bytes(len: usize, seed: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut hash = Blake3Hash::of(&[seed]);
    while out.len() < len {
        out.extend_from_slice(hash.as_bytes());
        hash = Blake3Hash::of(hash.as_bytes());
    }
    out.truncate(len);
    out
}
