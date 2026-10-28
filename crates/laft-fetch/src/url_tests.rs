use super::*;

fn urls(mirrors: &Mirrors) -> Vec<&str> {
    mirrors.bases().iter().map(BaseUrl::as_str).collect()
}

#[test]
fn http_is_refused_unless_the_repository_allows_it() {
    let refused = BaseUrl::parse("http://10.0.2.2:8000", false).expect_err("http is refused");
    assert!(matches!(refused.kind(), ErrorKind::HttpNotAllowed));
    assert_eq!(refused.url(), Some("http://10.0.2.2:8000"));
    let allowed = BaseUrl::parse("http://10.0.2.2:8000", true).expect("http is allowed");
    assert_eq!(allowed.scheme(), Scheme::Http);
    let error = Mirrors::new("https://a.example.org", &["http://b.example.org"], false)
        .expect_err("an http mirror is refused too");
    assert!(matches!(error.kind(), ErrorKind::HttpNotAllowed));
}

#[test]
fn only_the_three_schemes_are_accepted() {
    for url in [
        "ftp://example.org",
        "HTTPS://example.org",
        "example.org",
        "",
    ] {
        let error = BaseUrl::parse(url, true).expect_err(url);
        assert!(
            matches!(error.kind(), ErrorKind::UnsupportedScheme),
            "{url}"
        );
    }
    assert_eq!(
        BaseUrl::parse("file:///srv/repo/", false)
            .expect("absolute file URL")
            .dir(),
        Some(PathBuf::from("/srv/repo"))
    );
}

#[test]
fn malformed_base_urls_are_rejected() {
    for url in [
        "https://",
        "https:///path",
        "https://exa mple.org",
        "https://example.org/?q=1",
        "https://example.org/#top",
        "file://relative/path",
    ] {
        let error = BaseUrl::parse(url, true).expect_err(url);
        assert!(
            matches!(error.kind(), ErrorKind::InvalidUrl { .. }),
            "{url}"
        );
    }
}

#[test]
fn mirrors_follow_the_configured_url_then_mirrors_then_the_signed_list() {
    let mirrors = Mirrors::new(
        "https://a.example.org/laft/",
        &["https://b.example.org", "https://a.example.org/laft"],
        false,
    )
    .expect("valid URLs")
    .with_signed(&[
        "https://c.example.org",
        "http://d.example.org",
        "file:///srv/repo",
        "not a url",
        "https://b.example.org",
    ]);
    assert_eq!(
        urls(&mirrors),
        [
            "https://a.example.org/laft",
            "https://b.example.org",
            "https://c.example.org"
        ]
    );
}

#[test]
fn a_preferred_mirror_moves_to_the_front_keeping_the_rest_in_order() {
    let mirrors = Mirrors::new(
        "https://a.example.org",
        &["https://b.example.org", "https://c.example.org"],
        false,
    )
    .expect("valid URLs");
    let c = mirrors.bases()[2].clone();
    assert_eq!(
        urls(&mirrors.preferring(&c)),
        [
            "https://c.example.org",
            "https://a.example.org",
            "https://b.example.org"
        ]
    );
    let other = BaseUrl::parse("https://z.example.org", false).expect("valid URL");
    assert_eq!(mirrors.preferring(&other), mirrors);
}

#[test]
fn repository_paths_cannot_leave_the_repository() {
    for path in [
        "",
        "/etc/passwd",
        "../keys.json",
        "pool/../../etc",
        "pool//x",
        "pool/./x",
        "pool/x?y",
        "pool/x%2f",
        "pool/x y",
    ] {
        assert!(check_path(path).is_err(), "{path:?}");
    }
    for path in ["keys.json", "channels/current/x86_64-v3/index.json.zst"] {
        assert!(check_path(path).is_ok(), "{path:?}");
    }
}
