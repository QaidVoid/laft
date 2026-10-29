//! A recipe's sources are fetched into the cache, verified and unpacked,
//! without touching the network.

mod support;

use std::fs;
use std::io::Write;

use laft_build::{Repos, SourceCache, unpack};
use laft_fetch::{Config, Fetcher};
use sha2::{Digest, Sha256};

fn tarball() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(6);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, "hello-1.0/configure", &b"exit 0"[..])
        .expect("appendable");
    let tar = builder.into_inner().expect("finished");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar).expect("compressible");
    gz.finish().expect("compressible")
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn setup(served: &[u8], declared: &str) -> (laft_test::TempDir, Repos) {
    let dir = laft_test::tempdir();
    let mirror = dir.path().join("mirror");
    fs::create_dir_all(mirror.join("sha256")).expect("mkdir");
    fs::write(mirror.join("sha256").join(declared), served).expect("write");
    let body = format!(
        "  source main {{\n    url = [\"https://example.org/hello-#{{self.version}}.tar.gz\"]\n    \
         sha256 = \"sha256:{declared}\"\n  }}\n  build configure {{}}"
    );
    let repo = support::write_repo(
        &dir.path().join("core"),
        "core",
        &format!("  source_mirrors = [\"file://{}\"]\n", mirror.display()),
        &[(
            "recipes/hello/package.stav",
            &support::recipe("hello", &body),
        )],
    );
    let repos = Repos::open(&[repo]).expect("the repository opens");
    (dir, repos)
}

fn offline_cache(dir: &std::path::Path) -> SourceCache {
    let config = Config {
        offline: true,
        ..Config::new(dir.join("cache"))
    };
    SourceCache::new(dir.join("sources"), Fetcher::new(config))
}

#[test]
fn a_recipe_source_comes_from_a_mirror_and_is_unpacked() {
    let bytes = tarball();
    let (dir, repos) = setup(&bytes, &hex(&bytes));
    let recipe = support::evaluate(&repos, "hello").remove(0);
    let cache = offline_cache(dir.path());
    let mirrors = &repos.repos()[0].source_mirrors;
    let file = cache
        .fetch_source(&recipe, &recipe.sources[0], mirrors)
        .expect("the mirror has the file");
    let src = dir.path().join("work/src");
    let scratch = dir.path().join("work/tmp");
    fs::create_dir_all(&scratch).expect("mkdir");
    unpack(&recipe.sources[0], &file, &src, &scratch).expect("a safe archive");
    assert_eq!(
        fs::read(src.join("configure")).expect("unpacked"),
        b"exit 0"
    );
}

#[test]
fn a_source_with_a_bad_hash_is_rejected() {
    let bytes = tarball();
    let declared = hex(b"what the recipe expects");
    let (dir, repos) = setup(&bytes, &declared);
    let recipe = support::evaluate(&repos, "hello").remove(0);
    let cache = offline_cache(dir.path());
    let error = cache
        .fetch_source(
            &recipe,
            &recipe.sources[0],
            &repos.repos()[0].source_mirrors,
        )
        .expect_err("the mirror serves another file");
    assert_eq!(error.code(), "B020");
    assert!(
        error
            .to_string()
            .starts_with("source \"main\" of hello 1.0: checksum mismatch"),
        "{error}"
    );
    let cached = fs::read_dir(dir.path().join("sources/sha256"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(cached, 0);
}
