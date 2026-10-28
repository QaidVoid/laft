use std::time::{Duration, Instant};

use super::*;
use crate::test_support::{Fault, Logged, Server, bytes, config, package, refusing_url};

fn http(url: &str) -> BaseUrl {
    BaseUrl::parse(url, true).expect("valid URL")
}

fn mirrors(urls: &[String]) -> Mirrors {
    Mirrors::new(&urls[0], &urls[1..], true).expect("valid URLs")
}

fn quiet() -> impl FnMut(Progress) {
    |_| {}
}

#[test]
fn small_files_are_fetched_over_http_and_from_local_repositories() {
    let server = Server::start();
    server.put("keys.json", b"{}");
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let fetched = fetcher.fetch(
        &http(&server.url()),
        "keys.json",
        ObjectKind::Keys,
        &mut quiet(),
    );
    assert_eq!(fetched.expect("served"), b"{}");

    let repo = laft_test::tempdir();
    std::fs::write(repo.path().join("keys.json"), b"[]").expect("writable");
    let base =
        BaseUrl::parse(&format!("file://{}", repo.path().display()), false).expect("valid URL");
    let fetched = fetcher.fetch(&base, "keys.json", ObjectKind::Keys, &mut quiet());
    assert_eq!(fetched.expect("read in place"), b"[]");
}

#[test]
fn a_missing_file_is_reported_as_not_found_without_retrying() {
    let server = Server::start();
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let error = fetcher
        .fetch(
            &http(&server.url()),
            "keys.json",
            ObjectKind::Keys,
            &mut quiet(),
        )
        .expect_err("nothing is served");
    assert!(matches!(error.kind(), ErrorKind::NotFound));
    assert_eq!(
        error.to_string(),
        format!("{}/keys.json: not found", server.url())
    );
    assert_eq!(server.requests("keys.json").len(), 1);

    let repo = laft_test::tempdir();
    let base =
        BaseUrl::parse(&format!("file://{}", repo.path().display()), false).expect("valid URL");
    let error = fetcher
        .fetch(&base, "keys.json", ObjectKind::Keys, &mut quiet())
        .expect_err("the file is missing");
    assert!(matches!(error.kind(), ErrorKind::NotFound));
}

#[test]
fn server_errors_are_retried_until_one_succeeds() {
    let server = Server::start();
    server.put("keys.json", b"{}");
    server.fault("keys.json", Fault::Status(503));
    server.fault("keys.json", Fault::Status(500));
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let fetched = fetcher.fetch(
        &http(&server.url()),
        "keys.json",
        ObjectKind::Keys,
        &mut quiet(),
    );
    assert_eq!(fetched.expect("the third attempt succeeds"), b"{}");
    assert_eq!(server.requests("keys.json").len(), 3);
}

#[test]
fn persistent_failures_name_the_url_and_the_reason() {
    let server = Server::start();
    server.put("keys.json", b"{}");
    for _ in 0..3 {
        server.fault("keys.json", Fault::Status(502));
    }
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let error = fetcher
        .fetch(
            &http(&server.url()),
            "keys.json",
            ObjectKind::Keys,
            &mut quiet(),
        )
        .expect_err("every attempt fails");
    assert_eq!(
        error.to_string(),
        format!(
            "{}/keys.json: the server answered with HTTP status 502",
            server.url()
        )
    );

    let refused = refusing_url();
    let error = fetcher
        .fetch(&http(&refused), "keys.json", ObjectKind::Keys, &mut quiet())
        .expect_err("nothing listens");
    assert!(matches!(error.kind(), ErrorKind::Transport { .. }));
    assert!(
        error
            .to_string()
            .starts_with(&format!("{refused}/keys.json: "))
    );
}

#[test]
fn a_silent_server_times_out() {
    let server = Server::start();
    server.put("keys.json", b"{}");
    for _ in 0..3 {
        server.fault("keys.json", Fault::Stall(Duration::from_secs(3)));
    }
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let started = Instant::now();
    let error = fetcher
        .fetch(
            &http(&server.url()),
            "keys.json",
            ObjectKind::Keys,
            &mut quiet(),
        )
        .expect_err("the server stalls");
    assert!(matches!(error.kind(), ErrorKind::Timeout), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn files_beyond_the_limit_of_their_kind_are_refused() {
    let server = Server::start();
    let limit = ObjectKind::Signature.max_size() as usize;
    server.put("index.json.zst.minisig", &bytes(limit + 1, 0));
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let base = http(&server.url());
    let error = fetcher
        .fetch(
            &base,
            "index.json.zst.minisig",
            ObjectKind::Signature,
            &mut quiet(),
        )
        .expect_err("too large");
    assert!(matches!(error.kind(), ErrorKind::TooLarge { .. }));
    let fetched = fetcher.fetch(
        &base,
        "index.json.zst.minisig",
        ObjectKind::Index,
        &mut quiet(),
    );
    assert_eq!(fetched.expect("within the index limit").len(), limit + 1);
}

#[test]
fn offline_fetches_never_use_the_network() {
    let server = Server::start();
    server.put("keys.json", b"{}");
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(Config {
        offline: true,
        ..config(cache.path())
    });
    let error = fetcher
        .fetch(
            &http(&server.url()),
            "keys.json",
            ObjectKind::Keys,
            &mut quiet(),
        )
        .expect_err("offline");
    assert!(matches!(error.kind(), ErrorKind::Offline));
    assert!(server.requests("keys.json").is_empty());
}

#[test]
fn packages_are_verified_cached_and_then_served_from_the_cache() {
    let server = Server::start();
    let content = bytes(200_000, 1);
    let entry = package(&content);
    server.put(&entry.file, &content);
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let mirrors = mirrors(&[server.url()]);
    let mut reports = Vec::new();
    let path = fetcher
        .download_package(&mirrors, &entry, &mut |p| reports.push(p))
        .expect("downloaded");
    assert_eq!(path, cache.path().join(format!("pkgs/{}.lpkg", entry.id)));
    assert_eq!(std::fs::read(&path).expect("cached"), content);
    assert!(!part_path(&path).exists());
    assert!(reports.windows(2).all(|w| w[0].done <= w[1].done));
    assert_eq!(
        reports.last(),
        Some(&Progress {
            done: 200_000,
            total: Some(200_000)
        })
    );

    let offline = Fetcher::new(Config {
        offline: true,
        ..config(cache.path())
    });
    let again = offline.download_package(&mirrors, &entry, &mut quiet());
    assert_eq!(again.expect("cached"), path);
    assert_eq!(server.requests(&entry.file).len(), 1);
}

#[test]
fn offline_downloads_of_uncached_packages_name_the_missing_file() {
    let content = bytes(100, 2);
    let entry = package(&content);
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(Config {
        offline: true,
        ..config(cache.path())
    });
    let mirrors =
        Mirrors::new("https://pkgs.example.org", &[] as &[&str], false).expect("valid URL");
    let error = fetcher
        .download_package(&mirrors, &entry, &mut quiet())
        .expect_err("not cached");
    let expected = cache.path().join(format!("pkgs/{}.lpkg", entry.id));
    assert!(matches!(error.kind(), ErrorKind::NotCached { path } if *path == expected));
    assert!(error.to_string().contains("offline mode"));
}

#[test]
fn an_interrupted_download_resumes_where_it_stopped() {
    let server = Server::start();
    let content = bytes(300_000, 3);
    let entry = package(&content);
    server.put(&entry.file, &content);
    server.fault(&entry.file, Fault::Truncate(120_000));
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let path = fetcher
        .download_package(&mirrors(&[server.url()]), &entry, &mut quiet())
        .expect("resumed");
    assert_eq!(std::fs::read(path).expect("cached"), content);
    let requests = server.requests(&entry.file);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].range, None);
    assert_eq!(requests[1].range.as_deref(), Some("bytes=120000-"));
}

#[test]
fn a_partial_file_from_an_earlier_run_is_resumed() {
    let server = Server::start();
    let content = bytes(100_000, 4);
    let entry = package(&content);
    server.put(&entry.file, &content);
    let cache = laft_test::tempdir();
    let dest = cache.path().join(format!("pkgs/{}.lpkg", entry.id));
    std::fs::create_dir_all(dest.parent().expect("in a directory")).expect("writable");
    std::fs::write(part_path(&dest), &content[..40_000]).expect("writable");
    let fetcher = Fetcher::new(config(cache.path()));
    let path = fetcher
        .download_package(&mirrors(&[server.url()]), &entry, &mut quiet())
        .expect("resumed");
    assert_eq!(std::fs::read(path).expect("cached"), content);
    assert_eq!(
        server.requests(&entry.file),
        [Logged {
            path: entry.file.clone(),
            range: Some("bytes=40000-".into())
        }]
    );
}

#[test]
fn a_server_without_range_support_sends_the_whole_file_again() {
    let server = Server::start();
    let content = bytes(100_000, 5);
    let entry = package(&content);
    server.put(&entry.file, &content);
    server.fault(&entry.file, Fault::Truncate(30_000));
    server.fault(&entry.file, Fault::IgnoreRange);
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let path = fetcher
        .download_package(&mirrors(&[server.url()]), &entry, &mut quiet())
        .expect("downloaded again");
    assert_eq!(std::fs::read(path).expect("cached"), content);
}

#[test]
fn a_corrupt_package_is_discarded_and_the_next_mirror_is_tried() {
    let bad = Server::start();
    let good = Server::start();
    let content = bytes(50_000, 6);
    let entry = package(&content);
    let mut corrupt = content.clone();
    corrupt[10] ^= 1;
    bad.put(&entry.file, &corrupt);
    good.put(&entry.file, &content);
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let path = fetcher
        .download_package(&mirrors(&[bad.url(), good.url()]), &entry, &mut quiet())
        .expect("the second mirror serves the package");
    assert_eq!(std::fs::read(&path).expect("cached"), content);
    assert_eq!(bad.requests(&entry.file).len(), 1);
}

#[test]
fn when_every_mirror_fails_each_attempt_is_listed() {
    let bad = Server::start();
    let content = bytes(1000, 7);
    let entry = package(&content);
    bad.put(&entry.file, &content[..999]);
    let refused = refusing_url();
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let error = fetcher
        .download_package(
            &mirrors(&[bad.url(), refused.clone()]),
            &entry,
            &mut quiet(),
        )
        .expect_err("no mirror has the package");
    let ErrorKind::AllFailed { what, attempts } = error.kind() else {
        panic!("expected every mirror to fail, got {error}");
    };
    assert_eq!(*what, entry.file);
    assert_eq!(attempts.len(), 2);
    assert!(matches!(
        attempts[0].kind(),
        ErrorKind::SizeMismatch {
            expected: 1000,
            actual: 999
        }
    ));
    assert_eq!(
        attempts[1].url(),
        Some(format!("{refused}/{}", entry.file).as_str())
    );
    let dest = cache.path().join(format!("pkgs/{}.lpkg", entry.id));
    assert!(!dest.exists());
    assert!(!part_path(&dest).exists());
}

#[test]
fn a_larger_file_than_the_index_entry_says_is_cut_off() {
    let server = Server::start();
    let content = bytes(1000, 8);
    let entry = package(&content);
    server.put(&entry.file, &bytes(5000, 8));
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let error = fetcher
        .download_package(&mirrors(&[server.url()]), &entry, &mut quiet())
        .expect_err("too large");
    assert!(matches!(error.kind(), ErrorKind::TooLarge { limit: 1000 }));
}

#[test]
fn local_repositories_are_verified_and_read_in_place() {
    let repo = laft_test::tempdir();
    let content = bytes(10_000, 9);
    let entry = package(&content);
    let file = repo.path().join(&entry.file);
    std::fs::create_dir_all(file.parent().expect("in a directory")).expect("writable");
    std::fs::write(&file, &content).expect("writable");
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(Config {
        offline: true,
        ..config(cache.path())
    });
    let url = format!("file://{}", repo.path().display());
    let mirrors = Mirrors::new(&url, &[] as &[&str], false).expect("valid URL");
    let path = fetcher
        .download_package(&mirrors, &entry, &mut quiet())
        .expect("verified in place");
    assert_eq!(path, file);
    assert!(!cache.path().join("pkgs").exists());

    std::fs::write(&file, bytes(10_000, 10)).expect("writable");
    let error = fetcher
        .download_package(&mirrors, &entry, &mut quiet())
        .expect_err("modified in place");
    assert!(matches!(error.kind(), ErrorKind::HashMismatch { .. }));
}

#[test]
fn different_packages_download_concurrently() {
    let server = Server::start();
    let entries: Vec<_> = (0..8)
        .map(|i| {
            let content = bytes(150_000, 20 + i);
            let entry = package(&content);
            server.put(&entry.file, &content);
            server.fault(&entry.file, Fault::Truncate(50_000));
            (entry, content)
        })
        .collect();
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let mirrors = mirrors(&[server.url()]);
    thread::scope(|scope| {
        for (entry, content) in &entries {
            let (fetcher, mirrors) = (&fetcher, &mirrors);
            scope.spawn(move || {
                let path = fetcher
                    .download_package(mirrors, entry, &mut quiet())
                    .expect("downloaded");
                assert_eq!(&std::fs::read(path).expect("cached"), content);
            });
        }
    });
}

#[test]
fn a_damaged_cached_package_is_downloaded_again() {
    let server = Server::start();
    let content = bytes(20_000, 11);
    let entry = package(&content);
    server.put(&entry.file, &content);
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let mirrors = mirrors(&[server.url()]);
    let path = fetcher
        .download_package(&mirrors, &entry, &mut quiet())
        .expect("downloaded");
    std::fs::write(&path, b"damaged").expect("writable");
    let again = fetcher
        .download_package(&mirrors, &entry, &mut quiet())
        .expect("downloaded again");
    assert_eq!(std::fs::read(again).expect("cached"), content);
    assert_eq!(server.requests(&entry.file).len(), 2);
}

#[test]
fn a_stale_partial_file_that_does_not_match_is_downloaded_again() {
    let server = Server::start();
    let content = bytes(10_000, 12);
    let entry = package(&content);
    server.put(&entry.file, &content);
    let cache = laft_test::tempdir();
    let dest = cache.path().join(format!("pkgs/{}.lpkg", entry.id));
    std::fs::create_dir_all(dest.parent().expect("in a directory")).expect("writable");
    std::fs::write(part_path(&dest), bytes(4_000, 13)).expect("writable");
    let fetcher = Fetcher::new(config(cache.path()));
    let path = fetcher
        .download_package(&mirrors(&[server.url()]), &entry, &mut quiet())
        .expect("downloaded from the start");
    assert_eq!(std::fs::read(path).expect("cached"), content);
    let ranges: Vec<_> = server
        .requests(&entry.file)
        .into_iter()
        .map(|l| l.range)
        .collect();
    assert_eq!(ranges, [Some("bytes=4000-".to_owned()), None]);
}

#[test]
fn a_huge_length_on_a_resumed_answer_is_too_large() {
    let server = Server::start();
    let content = bytes(100_000, 11);
    let entry = package(&content);
    server.put(&entry.file, &content);
    server.fault(&entry.file, Fault::Length(u64::MAX - 10));
    let cache = laft_test::tempdir();
    let dest = cache.path().join(format!("pkgs/{}.lpkg", entry.id));
    std::fs::create_dir_all(dest.parent().expect("in a directory")).expect("writable");
    std::fs::write(part_path(&dest), &content[..40_000]).expect("writable");
    let fetcher = Fetcher::new(config(cache.path()));
    let error = fetcher
        .download_package(&mirrors(&[server.url()]), &entry, &mut quiet())
        .expect_err("the announced length is beyond the entry's size");
    assert!(
        matches!(error.kind(), ErrorKind::TooLarge { limit: 100_000 }),
        "{error}"
    );
    assert!(!part_path(&dest).exists());
}

#[test]
fn a_transfer_cut_off_mid_body_is_retried() {
    let server = Server::start();
    let keys = bytes(5000, 12);
    server.put("keys.json", &keys);
    server.fault("keys.json", Fault::Truncate(1000));
    let cache = laft_test::tempdir();
    let fetcher = Fetcher::new(config(cache.path()));
    let fetched = fetcher.fetch(
        &http(&server.url()),
        "keys.json",
        ObjectKind::Keys,
        &mut quiet(),
    );
    assert_eq!(fetched.expect("the second attempt succeeds"), keys);
    assert_eq!(server.requests("keys.json").len(), 2);
}
