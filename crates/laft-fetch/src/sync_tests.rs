use super::*;
use crate::Config;
use crate::test_support::{
    CHANNEL, DAY, Fault, Server, T0, TestKey, config, index, now, publish, refusing_url, repo,
    write_tree,
};

/// A client of `core` with its own cache and state directories.
struct Client {
    dir: tempfile::TempDir,
    fetcher: Fetcher,
    state: IndexState,
    repo: Repo,
}

impl Client {
    fn new(root: &TestKey) -> Self {
        let dir = tempfile::tempdir().expect("temporary directory");
        Self {
            fetcher: Fetcher::new(config(&dir.path().join("cache"))),
            state: IndexState::new(dir.path().join("state")),
            repo: repo(root),
            dir,
        }
    }

    fn offline(mut self) -> Self {
        self.fetcher = Fetcher::new(Config {
            offline: true,
            ..self.fetcher.config().clone()
        });
        self
    }

    fn sync_at(
        &self,
        urls: &[String],
        now: Timestamp,
        allow_expired: bool,
    ) -> Result<Synced, Error> {
        let mirrors = Mirrors::new(&urls[0], &urls[1..], true).expect("valid URLs");
        let request = SyncRequest {
            repo: &self.repo,
            mirrors: &mirrors,
            state: &self.state,
            now,
            allow_expired,
        };
        self.fetcher.sync(&request, &mut |_| {})
    }

    fn sync(&self, urls: &[String]) -> Result<Synced, Error> {
        self.sync_at(urls, now(), false)
    }
}

fn mirror_base(synced: &Synced) -> &str {
    match &synced.source {
        Source::Mirror { base, .. } => base.as_str(),
        Source::Cache { failure } => panic!("expected a mirror, the sync failed: {failure}"),
    }
}

fn cache_failure(synced: &Synced) -> &Error {
    match &synced.source {
        Source::Cache { failure } => failure,
        Source::Mirror { base, .. } => panic!("expected the cache, got {base}"),
    }
}

#[test]
fn a_local_repository_without_keys_json_syncs_in_direct_mode() {
    let root = TestKey::generate();
    let dir = tempfile::tempdir().expect("temporary directory");
    write_tree(dir.path(), &publish(&root, None, &index(T0)));
    let client = Client::new(&root);
    let url = format!("file://{}", dir.path().display());
    let synced = client.sync(std::slice::from_ref(&url)).expect("synced");
    assert_eq!(synced.index.record.generation, T0);
    assert!(!synced.index.expired);
    let Source::Mirror { base, trust } = &synced.source else {
        panic!("expected a mirror");
    };
    assert_eq!(base.as_str(), url);
    assert!(trust.keys().is_none());
    let cached = client
        .fetcher
        .config()
        .cache
        .join(format!("index/core/{CHANNEL}/index.json.zst"));
    assert_eq!(
        std::fs::read(cached).expect("cached"),
        std::fs::read(dir.path().join(CHANNEL).join("index.json.zst")).expect("published")
    );
}

#[test]
fn keys_json_then_the_signature_then_the_index_are_fetched() {
    let (root, online) = (TestKey::generate(), TestKey::generate());
    let server = Server::start();
    server.put_all(&publish(&root, Some(&online), &index(T0)));
    let client = Client::new(&root);
    let synced = client.sync(&[server.url()]).expect("synced");
    let Source::Mirror { trust, .. } = &synced.source else {
        panic!("expected a mirror");
    };
    assert_eq!(trust.keys().map(|k| k.version), Some(1));
    assert_eq!(
        server.paths(),
        [
            "keys.json".to_owned(),
            "keys.json.minisig".to_owned(),
            format!("{CHANNEL}/index.json.zst.minisig"),
            format!("{CHANNEL}/index.json.zst"),
        ]
    );
}

#[test]
fn an_unreachable_mirror_is_skipped() {
    let root = TestKey::generate();
    let server = Server::start();
    server.put_all(&publish(&root, None, &index(T0)));
    let client = Client::new(&root);
    let synced = client
        .sync(&[refusing_url(), server.url()])
        .expect("the second mirror works");
    assert_eq!(mirror_base(&synced), server.url());
}

#[test]
fn a_mirror_serving_an_index_signed_by_an_unknown_key_is_skipped() {
    let root = TestKey::generate();
    let bad = Server::start();
    let good = Server::start();
    bad.put_all(&publish(&TestKey::generate(), None, &index(T0 + 10)));
    good.put_all(&publish(&root, None, &index(T0)));
    let client = Client::new(&root);
    let synced = client.sync(&[bad.url(), good.url()]).expect("synced");
    assert_eq!(mirror_base(&synced), good.url());
    assert_eq!(synced.index.record.generation, T0);
}

#[test]
fn a_republished_pair_falls_back_to_the_snapshot_named_by_the_signature() {
    let root = TestKey::generate();
    let old = publish(&root, None, &index(T0));
    let new = publish(&root, None, &index(T0 + 60));
    let server = Server::start();
    server.put_all(&new);
    let entry = format!("{CHANNEL}/index.json.zst");
    server.put(&entry, &old[&entry]);
    let client = Client::new(&root);
    let synced = client
        .sync(&[server.url()])
        .expect("synced from the snapshot");
    assert_eq!(synced.index.record.generation, T0 + 60);
    assert_eq!(server.requests(&entry).len(), 2);
    let snapshot = format!("{CHANNEL}/snapshots/{}.json.zst", T0 + 60);
    assert_eq!(server.requests(&snapshot).len(), 1);
}

#[test]
fn the_cached_index_is_used_when_every_mirror_fails() {
    let root = TestKey::generate();
    let server = Server::start();
    server.put_all(&publish(&root, None, &index(T0)));
    let client = Client::new(&root);
    client.sync(&[server.url()]).expect("first sync");
    for _ in 0..3 {
        server.fault("keys.json", Fault::Status(503));
    }
    let refused = refusing_url();
    let synced = client
        .sync(&[server.url(), refused.clone()])
        .expect("the cache is used");
    assert_eq!(synced.index.record.generation, T0);
    let failure = cache_failure(&synced);
    let ErrorKind::AllFailed { attempts, .. } = failure.kind() else {
        panic!("expected every mirror to fail, got {failure}");
    };
    assert_eq!(
        attempts[0].url(),
        Some(format!("{}/keys.json", server.url()).as_str())
    );
    assert_eq!(
        attempts[1].url(),
        Some(format!("{refused}/keys.json").as_str())
    );
}

#[test]
fn a_keys_json_that_disappears_after_acceptance_is_not_direct_mode() {
    let (root, online) = (TestKey::generate(), TestKey::generate());
    let server = Server::start();
    server.put_all(&publish(&root, Some(&online), &index(T0)));
    let client = Client::new(&root);
    client.sync(&[server.url()]).expect("first sync");
    server.remove("keys.json");
    let synced = client.sync(&[server.url()]).expect("the cache is used");
    let failure = cache_failure(&synced);
    assert!(
        matches!(
            last_attempt(failure),
            ErrorKind::Trust(e) if matches!(e.kind(), laft_index::ErrorKind::KeysMissing)
        ),
        "{failure}"
    );
}

#[test]
fn an_expired_cached_index_is_used_only_when_expired_indexes_are_allowed() {
    let root = TestKey::generate();
    let server = Server::start();
    server.put_all(&publish(&root, None, &index(T0)));
    let client = Client::new(&root);
    client.sync(&[server.url()]).expect("first sync");
    let client = client.offline();
    let later = Timestamp::from_secs(T0 + 15 * DAY);
    let error = client
        .sync_at(&[server.url()], later, false)
        .expect_err("expired");
    assert!(error.to_string().contains("expired"), "{error}");
    let synced = client
        .sync_at(&[server.url()], later, true)
        .expect("expired indexes are allowed");
    assert!(synced.index.expired);
}

#[test]
fn offline_syncs_use_only_the_cache() {
    let root = TestKey::generate();
    let server = Server::start();
    server.put_all(&publish(&root, None, &index(T0)));
    let client = Client::new(&root).offline();
    let error = client.sync(&[server.url()]).expect_err("nothing is cached");
    let ErrorKind::AllFailed { attempts, .. } = error.kind() else {
        panic!("expected every source to fail, got {error}");
    };
    assert!(matches!(attempts[0].kind(), ErrorKind::Offline));
    assert!(matches!(
        attempts.last().map(Error::kind),
        Some(ErrorKind::NotFound)
    ));
    assert!(server.paths().is_empty());
}

#[test]
fn a_cached_index_that_differs_from_the_accepted_one_is_refused() {
    let root = TestKey::generate();
    let server = Server::start();
    server.put_all(&publish(&root, None, &index(T0)));
    let client = Client::new(&root);
    client.sync(&[server.url()]).expect("first sync");
    let cached = client
        .fetcher
        .config()
        .cache
        .join(format!("index/core/{CHANNEL}/index.json.zst"));
    let other = publish(&root, None, &index(T0 - 60));
    std::fs::write(&cached, &other[&format!("{CHANNEL}/index.json.zst")]).expect("writable");
    let error = client
        .sync(&[refusing_url()])
        .expect_err("the cache was replaced");
    let ErrorKind::AllFailed { attempts, .. } = error.kind() else {
        panic!("expected every source to fail, got {error}");
    };
    assert!(matches!(
        attempts.last().map(Error::kind),
        Some(ErrorKind::HashMismatch { .. })
    ));
}

/// The `keys.json` files of `core` at `version`, signed by `root`, listing
/// `online` and revoking `revoked`.
fn keys_files(
    root: &TestKey,
    version: u64,
    online: &TestKey,
    revoked: &TestKey,
    generation_floor: Option<u64>,
) -> std::collections::BTreeMap<String, Vec<u8>> {
    let keys = laft_index::Keys {
        format: laft_index::Keys::FORMAT,
        repo: "core".parse().expect("valid name"),
        version,
        keys: vec![online.online()],
        revoked: vec![revoked.public.id()],
        generation_floor,
    };
    let json = keys.to_json().expect("sample keys are valid");
    let signature = root.sign(&json, &keys.comment().to_string());
    [
        ("keys.json".to_owned(), json),
        ("keys.json.minisig".to_owned(), signature),
    ]
    .into()
}

fn last_attempt(error: &Error) -> &ErrorKind {
    let ErrorKind::AllFailed { attempts, .. } = error.kind() else {
        panic!("expected every source to fail, got {error}");
    };
    attempts.last().map(Error::kind).expect("an attempt")
}

#[test]
fn a_mirror_without_keys_json_does_not_decide_direct_mode_alone() {
    let (root, online) = (TestKey::generate(), TestKey::generate());
    let empty = Server::start();
    let full = Server::start();
    full.put_all(&publish(&root, Some(&online), &index(T0)));
    let client = Client::new(&root);
    let synced = client.sync(&[empty.url(), full.url()]).expect("synced");
    assert_eq!(mirror_base(&synced), full.url());
    let Source::Mirror { trust, .. } = &synced.source else {
        panic!("expected a mirror");
    };
    assert_eq!(trust.keys().map(|k| k.version), Some(1));
}

#[test]
fn every_mirror_without_keys_json_means_direct_mode() {
    let root = TestKey::generate();
    let first = Server::start();
    let second = Server::start();
    second.put_all(&publish(&root, None, &index(T0)));
    let client = Client::new(&root);
    let synced = client
        .sync(&[first.url(), refusing_url(), second.url()])
        .expect("synced");
    assert_eq!(mirror_base(&synced), second.url());
    assert_eq!(first.requests("keys.json").len(), 1);
    assert_eq!(second.requests("keys.json").len(), 1);
}

#[test]
fn a_corrupt_state_record_fails_the_sync_instead_of_using_the_cache() {
    let root = TestKey::generate();
    let server = Server::start();
    server.put_all(&publish(&root, None, &index(T0)));
    let client = Client::new(&root);
    client.sync(&[server.url()]).expect("first sync");
    let record = client.dir.path().join("state/core.json");
    std::fs::write(record, b"not json").expect("writable");
    for urls in [vec![server.url()], vec![refusing_url()]] {
        let error = client.sync(&urls).expect_err("the state is unusable");
        assert!(
            matches!(
                error.kind(),
                ErrorKind::Trust(e) if matches!(e.kind(), laft_index::ErrorKind::CorruptState { .. })
            ),
            "{error}"
        );
    }
}

#[test]
fn keys_acceptance_does_not_write_to_the_cache() {
    use std::os::unix::fs::PermissionsExt;

    let (root, online) = (TestKey::generate(), TestKey::generate());
    let server = Server::start();
    server.put_all(&publish(&root, Some(&online), &index(T0)));
    let client = Client::new(&root);
    let repo_cache = client.fetcher.config().cache.join("index/core");
    std::fs::create_dir_all(repo_cache.join(CHANNEL)).expect("writable");
    std::fs::set_permissions(&repo_cache, std::fs::Permissions::from_mode(0o555))
        .expect("the test owns the directory");
    if std::fs::write(repo_cache.join("probe"), b"").is_ok() {
        println!("skipped: read-only directories are writable for this user");
        return;
    }
    let synced = client.sync(&[server.url()]);
    std::fs::set_permissions(&repo_cache, std::fs::Permissions::from_mode(0o755))
        .expect("the test owns the directory");
    let synced = synced.expect("the index is cached below the read-only directory");
    assert_eq!(mirror_base(&synced), server.url());
    assert!(!repo_cache.join("keys.json").exists());
}

#[test]
fn a_cached_index_signed_by_a_key_revoked_since_is_refused_and_dropped() {
    let (root, old, new) = (
        TestKey::generate(),
        TestKey::generate(),
        TestKey::generate(),
    );
    let server = Server::start();
    server.put_all(&publish(&root, Some(&old), &index(T0)));
    let client = Client::new(&root);
    client.sync(&[server.url()]).expect("first sync");
    server.put_all(&keys_files(&root, 2, &new, &old, None));
    let error = client
        .sync(&[server.url()])
        .expect_err("the cached index is signed by a revoked key");
    assert!(
        matches!(last_attempt(&error), ErrorKind::CachedIndexRevoked { signer } if *signer == old.public.id()),
        "{error}"
    );
    assert!(error.to_string().contains("cached index"), "{error}");
    let error = client
        .offline()
        .sync(&[server.url()])
        .expect_err("the cached index is gone");
    assert!(
        matches!(last_attempt(&error), ErrorKind::NotFound),
        "{error}"
    );
}

#[test]
fn a_cached_index_whose_pin_a_revocation_cleared_is_refused() {
    let (root, old, new) = (
        TestKey::generate(),
        TestKey::generate(),
        TestKey::generate(),
    );
    let server = Server::start();
    server.put_all(&publish(&root, Some(&old), &index(T0)));
    let client = Client::new(&root);
    client.sync(&[server.url()]).expect("first sync");
    server.put_all(&keys_files(&root, 2, &new, &old, Some(T0 - 60)));
    let error = client
        .sync(&[server.url()])
        .expect_err("the cached index is no longer pinned");
    assert!(
        matches!(
            last_attempt(&error),
            ErrorKind::CachedIndexUnpinned { generation } if *generation == T0 - 60
        ),
        "{error}"
    );
    assert!(error.to_string().contains("cached index"), "{error}");
}
