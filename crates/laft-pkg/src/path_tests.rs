use proptest::prelude::*;

use super::*;

#[test]
fn accepts_payload_paths() {
    for path in [
        "usr",
        "etc",
        "usr/bin/rg",
        "etc/ripgreprc",
        "usr/lib/sbinfo",
        "usr/localedata",
    ] {
        assert_eq!(check(path), Ok(()), "{path}");
    }
}

#[test]
fn rejects_paths_outside_the_payload_rules() {
    let long = format!("usr/{}", "a".repeat(4092));
    for path in [
        "",
        "/usr/bin",
        "usr/",
        "usr//bin",
        "usr/./bin",
        "usr/../etc",
        "usr/bin/a\0b",
        "var/lib/x",
        "home",
        "bin/sh",
        ".laft/meta.json",
        "usr/sbin",
        "usr/sbin/init",
        "usr/lib64/libc.so",
        "usr/local",
        long.as_str(),
    ] {
        assert!(check(path).is_err(), "{path:?} should be rejected");
    }
    assert_eq!(check(&long[..4095]), Ok(()));
}

proptest! {
    #[test]
    fn accepted_paths_have_only_plain_components(path in "(usr|etc)(/[a-z.]{1,4}){0,4}") {
        if check(&path).is_ok() {
            prop_assert!(path.split('/').all(|c| !c.is_empty() && c != "." && c != ".."));
        }
    }
}
