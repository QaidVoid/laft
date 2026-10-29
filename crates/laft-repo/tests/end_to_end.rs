//! A repository's whole life: packages are built, indexed into `testing`
//! with an online key that a root-signed `keys.json` lists, checked,
//! promoted to `current` and published. A client then accepts the
//! published index and downloads a package over `file://`.

mod support;

use std::fs;

use laft_core::Timestamp;
use laft_fetch::{Config, Fetcher, Mirrors, SyncRequest};
use laft_index::{IndexState, KeysFetch, Repo};
use laft_pkg::{Arch, Reader};
use support::{DAY, HOUR, T0, at, keygen, ok, pkg, public, s};

#[test]
fn published_repository_is_accepted_and_served_over_file_urls() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("pcre2", "10.46")
        .provides("so:libpcre2-8.so.0")
        .file("usr/lib/libpcre2-8.so.0", "pcre2")
        .write(&out);
    pkg("ripgrep", "14.1.1")
        .depends("so:libpcre2-8.so.0")
        .file("usr/bin/rg", "ripgrep")
        .write(&out);
    pkg("hello", "2.12")
        .file("usr/bin/hello", "hello")
        .write(&out);
    let repo = dir.path().join("repo");
    let www = dir.path().join("www");
    let root = keygen(dir.path(), "root");
    let online = keygen(dir.path(), "online");

    let online_pub = online.with_extension("pub");
    ok(&[
        "keys",
        "add",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--public",
        s(&online_pub),
        "--not-after",
        &at(365 * DAY),
        "--key",
        s(&root),
        "--now",
        &at(0),
    ]);
    ok(&[
        "index",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--channel",
        "testing",
        "--key",
        s(&online),
        "--now",
        &at(0),
        s(&out),
    ]);
    let report = ok(&[
        "check",
        "--repo",
        s(&repo),
        "--channel",
        "testing",
        "--json",
    ]);
    let report_file = dir.path().join("report.json");
    fs::write(&report_file, &report).unwrap();
    ok(&[
        "promote",
        "--repo",
        s(&repo),
        "--min-soak",
        "24h",
        "--root-key",
        s(&root.with_extension("pub")),
        "--check-report",
        s(&report_file),
        "--key",
        s(&online),
        "--now",
        &at(DAY),
    ]);
    let published = ok(&["publish", s(&repo), s(&www)]);
    assert!(published.contains(&format!(
        "published current/x86_64-v3 generation {}",
        T0 + DAY
    )));
    assert!(published.contains(&format!("published testing/x86_64-v3 generation {T0}")));

    let state = IndexState::new(dir.path().join("state"));
    let config = Repo {
        name: "core".parse().unwrap(),
        channel: "current".parse().unwrap(),
        arch: Arch::X86_64V3,
        key: public(&root),
        keys: vec![],
    };
    let json = fs::read(www.join("keys.json")).unwrap();
    let signature = fs::read(www.join("keys.json.minisig")).unwrap();
    let trust = state
        .accept_keys(
            &config,
            KeysFetch::Found {
                json: &json,
                signature: &signature,
            },
        )
        .unwrap();
    let index = fs::read(www.join("channels/current/x86_64-v3/index.json.zst")).unwrap();
    let signature =
        fs::read(www.join("channels/current/x86_64-v3/index.json.zst.minisig")).unwrap();
    let now = Timestamp::from_secs(T0 + DAY + HOUR);
    let accepted = state
        .accept_index(&config, &trust, &index, &signature, now, false)
        .unwrap();
    assert_eq!(accepted.index.generation, T0 + DAY);
    assert_eq!(accepted.record.signer, public(&online).id());
    assert!(!accepted.expired);
    let names: Vec<&str> = accepted
        .index
        .packages
        .iter()
        .map(|e| e.meta.name.as_str())
        .collect();
    assert_eq!(names, ["hello", "pcre2", "ripgrep"]);

    let fetcher = Fetcher::new(Config::new(dir.path().join("cache")));
    let url = format!("file://{}", www.display());
    let mirrors = Mirrors::new(&url, &[] as &[&str], false).unwrap();
    let client_state = IndexState::new(dir.path().join("client-state"));
    let synced = fetcher
        .sync(
            &SyncRequest {
                repo: &config,
                mirrors: &mirrors,
                state: &client_state,
                now,
                allow_expired: false,
            },
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(synced.index.index, accepted.index);
    let entry = synced
        .index
        .index
        .packages
        .iter()
        .find(|e| e.meta.name.as_str() == "ripgrep")
        .unwrap();
    let path = fetcher
        .download_package(&mirrors, entry, &mut |_| {})
        .unwrap();
    let reader = Reader::new(fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(reader.manifest().store_id(), entry.id);
    let unpacked = laft_test::tempdir();
    reader.unpack(unpacked.path()).unwrap();
    assert_eq!(
        fs::read(unpacked.path().join("usr/bin/rg")).unwrap(),
        b"ripgrep"
    );
}
