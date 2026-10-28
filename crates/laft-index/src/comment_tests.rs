use super::*;

#[test]
fn comments_round_trip() {
    let index = "laft-index repo=core channel=testing arch=x86_64-v3 generation=1761652800";
    assert_eq!(
        index.parse::<IndexComment>().expect("valid").to_string(),
        index
    );
    let keys = "laft-keys repo=core version=3";
    assert_eq!(
        keys.parse::<KeysComment>().expect("valid").to_string(),
        keys
    );
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
