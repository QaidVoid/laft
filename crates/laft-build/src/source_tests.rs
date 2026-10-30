use laft_fetch::Config;
use sha2::Digest;

use super::*;

fn sha256(bytes: &[u8]) -> Sha256Hash {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into())
}

/// A cache in `dir` that never uses the network.
fn cache(dir: &Path) -> SourceCache {
    let config = Config {
        offline: true,
        ..Config::new(dir.join("fetch"))
    };
    SourceCache::new(dir.join("sources"), Fetcher::new(config))
}

fn url(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn put(dir: &Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    fs::write(&path, bytes).expect("write");
    url(&path)
}

fn listing(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| {
                    e.expect("readable")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_verified_download_enters_the_cache_under_its_hash() {
    let dir = laft_test::tempdir();
    let cache = cache(dir.path());
    let hash = sha256(b"zlib source");
    let first = put(dir.path(), "upstream/zlib-1.3.1.tar.gz", b"zlib source");
    let path = cache
        .fetch("source \"main\" of zlib 1.3.1", &[first], &[], &hash)
        .expect("the hash matches");
    assert_eq!(path, cache.path(&hash));
    assert_eq!(fs::read(&path).expect("cached"), b"zlib source");
    assert_eq!(
        listing(&dir.path().join("sources/tmp")),
        Vec::<String>::new()
    );

    fs::remove_file(dir.path().join("upstream/zlib-1.3.1.tar.gz")).expect("remove");
    let again = cache.fetch("source", &[], &[], &hash);
    assert_eq!(again.ok(), Some(path), "a cached file needs no location");
}

#[test]
fn a_wrong_hash_stops_the_fetch_and_leaves_nothing_behind() {
    let dir = laft_test::tempdir();
    let cache = cache(dir.path());
    let expected = sha256(b"zlib source");
    let wrong = put(dir.path(), "a/zlib.tar.gz", b"something else");
    let right = put(dir.path(), "b/zlib.tar.gz", b"zlib source");
    let error = cache
        .fetch(
            "source \"main\" of zlib 1.3.1",
            &[wrong.clone(), right],
            &[],
            &expected,
        )
        .expect_err("the first URL serves another file");
    assert_eq!(error.code(), "B020");
    assert_eq!(
        error.to_string(),
        format!(
            "source \"main\" of zlib 1.3.1: checksum mismatch\n  url:      {wrong}\n  \
             expected: {expected}\n  actual:   {}",
            sha256(b"something else")
        )
    );
    assert_eq!(
        listing(&dir.path().join("sources/sha256")),
        Vec::<String>::new()
    );
    assert_eq!(
        listing(&dir.path().join("sources/tmp")),
        Vec::<String>::new()
    );
}

#[test]
fn missing_files_fall_through_to_the_next_url_and_the_mirrors() {
    let dir = laft_test::tempdir();
    let cache = cache(dir.path());
    let hash = sha256(b"pcre2 source");
    let hex = hash.to_string().replace("sha256:", "");
    let mirror = dir.path().join("mirror");
    put(&mirror, &format!("sha256/{hex}"), b"pcre2 source");
    let missing = url(&dir.path().join("missing.tar.gz"));
    let path = cache
        .fetch(
            "source",
            &[missing.clone(), "https://example.org/pcre2.tar.gz".into()],
            &[url(&mirror)],
            &hash,
        )
        .expect("the mirror has the file");
    assert_eq!(fs::read(path).expect("cached"), b"pcre2 source");

    let other = sha256(b"other");
    let Err(SourceError::Unavailable { what, attempts }) = cache.fetch(
        "source \"docs\" of foo 1.0",
        &[missing],
        &[url(&mirror)],
        &other,
    ) else {
        panic!("no location has the file");
    };
    assert_eq!(what, "source \"docs\" of foo 1.0");
    let urls: Vec<String> = attempts
        .iter()
        .map(|a| a.url().unwrap_or_default().to_owned())
        .collect();
    let other_hex = other.to_string().replace("sha256:", "");
    assert_eq!(
        urls,
        [
            url(&dir.path().join("missing.tar.gz")),
            format!("{}/sha256/{other_hex}", url(&mirror)),
        ]
    );
}

#[test]
fn local_files_are_added_under_their_hash() {
    let dir = laft_test::tempdir();
    let cache = cache(dir.path());
    let tarball = dir.path().join("vendor.tar");
    fs::write(&tarball, b"vendored crates").expect("write");
    let hash = cache.insert(&tarball).expect("insertable");
    assert_eq!(hash, sha256(b"vendored crates"));
    assert_eq!(
        fs::read(cache.path(&hash)).expect("cached"),
        b"vendored crates"
    );
    assert!(tarball.exists());
}
