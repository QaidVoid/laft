use super::*;

#[test]
fn comments_round_trip() {
    let index = "laft-index repo=core channel=testing arch=x86_64-v3 generation=1761652800";
    assert_eq!(
        index.parse::<IndexComment>().expect("valid").to_string(),
        index
    );
    let profile =
        "laft-index repo=core channel=testing arch=x86_64-v3 generation=1761652800 profile=minimal";
    let comment: IndexComment = profile.parse().expect("valid");
    assert_eq!(comment.to_string(), profile);
    assert_eq!(comment.profile.as_ref().map(Name::as_str), Some("minimal"));
    let keys = "laft-keys repo=core version=3";
    assert_eq!(
        keys.parse::<KeysComment>().expect("valid").to_string(),
        keys
    );
}

#[test]
fn comments_without_a_profile_have_none() {
    let comment: IndexComment = "laft-index repo=core channel=testing arch=x86_64-v3 generation=1"
        .parse()
        .expect("valid");
    assert!(comment.profile.is_none());
}

#[test]
fn malformed_index_comments_are_rejected() {
    for text in [
        "",
        "laft-index repo=core channel=current arch=x86_64-v3",
        "laft-index repo=core channel=current arch=x86_64-v3 generation=1 extra=2",
        "laft-index channel=current repo=core arch=x86_64-v3 generation=1",
        "laft-index repo=core channel=current arch=x86_64-v3 generation=01",
        "laft-index repo=core channel=current arch=x86_64-v3 generation=+1",
        "laft-index repo=core channel=current arch=aarch64 generation=1",
        "laft-index repo=Core channel=current arch=x86_64-v3 generation=1",
        "laft-index  repo=core channel=current arch=x86_64-v3 generation=1",
        "laft-index repo=core channel=current arch=x86_64-v3 generation=1 profile=",
        "laft-index repo=core channel=current arch=x86_64-v3 generation=1 profile=minimal extra=2",
        "laft-index repo=core channel=current arch=x86_64-v3 generation=1 profile=min imal",
        "laft-index repo=Core channel=current arch=x86_64-v3 generation=1 profile=minimal",
        "laft-keys repo=core version=1",
    ] {
        assert!(text.parse::<IndexComment>().is_err(), "{text:?}");
    }
}

#[test]
fn malformed_keys_comments_are_rejected() {
    for text in [
        "laft-keys repo=core",
        "laft-keys repo=core version=",
        "laft-keys repo=core version=1 ",
        "laft-index repo=core version=1",
    ] {
        assert!(text.parse::<KeysComment>().is_err(), "{text:?}");
    }
}
