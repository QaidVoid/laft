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

/// Payload paths whose components are all plain names.
fn plain_path() -> impl Strategy<Value = String> {
    (
        prop::sample::select(vec!["usr/share", "usr/bin", "etc/xdg"]),
        prop::collection::vec("[a-z0-9_][a-z0-9._-]{0,6}", 0..4),
    )
        .prop_map(|(root, rest)| {
            std::iter::once(root.to_owned())
                .chain(rest)
                .collect::<Vec<_>>()
                .join("/")
        })
}

proptest! {
    #[test]
    fn plain_paths_are_accepted(path in plain_path()) {
        prop_assert_eq!(check(&path), Ok(()));
    }

    #[test]
    fn empty_dot_and_dot_dot_components_are_rejected(
        path in plain_path(),
        at in any::<prop::sample::Index>(),
        inserted in prop::sample::select(vec!["/", "./", "../"]),
        appended in prop::sample::select(vec!["/", "/.", "/.."]),
    ) {
        let after_slashes: Vec<usize> = path.match_indices('/').map(|(i, _)| i + 1).collect();
        let mut broken = path.clone();
        broken.insert_str(after_slashes[at.index(after_slashes.len())], inserted);
        prop_assert!(check(&broken).is_err(), "{:?} is accepted", broken);
        let broken = format!("{path}{appended}");
        prop_assert!(check(&broken).is_err(), "{:?} is accepted", broken);
    }
}
