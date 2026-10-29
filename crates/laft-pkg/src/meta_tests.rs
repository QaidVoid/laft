use super::*;
use crate::test_support::meta;

fn rejected(change: impl FnOnce(&mut Meta)) -> String {
    let mut m = meta(0);
    change(&mut m);
    m.validate()
        .expect_err("metadata should be rejected")
        .to_string()
}

#[test]
fn example_metadata_is_canonical_and_round_trips() {
    let m = meta(5498112);
    let json = m.to_json().expect("valid metadata");
    let text = String::from_utf8(json.clone()).expect("JSON is UTF-8");
    assert!(
        text.starts_with(r#"{"abi":{},"abi_epoch":0,"alternatives":[],"arch":"x86_64-v3","#),
        "{text}"
    );
    assert_eq!(Meta::from_json(&json).expect("stored form parses"), m);
}

#[test]
fn parsing_rejects_non_canonical_and_unknown_fields() {
    let json = meta(0).to_json().expect("valid metadata");
    let value: serde_json::Value = serde_json::from_slice(&json).expect("valid JSON");
    let pretty = serde_json::to_vec_pretty(&value).expect("serializable");
    let err = Meta::from_json(&pretty).expect_err("pretty JSON is not canonical");
    assert!(err.to_string().contains("not canonical"), "{err}");

    let mut extra = value.clone();
    extra["built_at"] = serde_json::json!("2026-01-01");
    let extra = laft_core::to_canonical_json(&extra).expect("canonicalizable");
    assert!(Meta::from_json(&extra).is_err());

    let mut missing = value;
    missing.as_object_mut().expect("object").remove("triggers");
    let missing = laft_core::to_canonical_json(&missing).expect("canonicalizable");
    assert!(Meta::from_json(&missing).is_err());
}

#[test]
fn lists_must_be_sorted_without_duplicates() {
    let err = rejected(|m| m.depends.reverse());
    assert!(err.contains("`depends`"), "{err}");
    let err = rejected(|m| m.license.reverse());
    assert!(err.contains("`license`"), "{err}");
    let err = rejected(|m| {
        let first = m.depends[0].clone();
        m.depends.insert(0, first);
    });
    assert!(err.contains("without duplicates"), "{err}");
    let err = rejected(|m| {
        m.triggers = vec![
            "mime".parse().expect("valid"),
            "desktop".parse().expect("valid"),
        ];
    });
    assert!(err.contains("`triggers`"), "{err}");
}

#[test]
fn conflicts_and_replaces_use_package_forms() {
    let mut m = meta(0);
    m.conflicts = vec![
        "busybox".parse().expect("valid"),
        "virtual:sh".parse().expect("valid"),
    ];
    m.replaces = vec!["ripgrep-bin<14".parse().expect("valid")];
    assert!(m.validate().is_ok());
    let err = rejected(|m| m.conflicts = vec!["so:libz.so.1".parse().expect("valid")]);
    assert!(err.contains("`conflicts`"), "{err}");
    let err = rejected(|m| m.replaces = vec!["virtual:sh".parse().expect("valid")]);
    assert!(err.contains("`replaces`"), "{err}");
}

#[test]
fn text_fields_are_checked() {
    assert!(rejected(|m| m.summary = "a".repeat(121)).contains("summary"));
    assert!(rejected(|m| m.summary = "two\nlines".into()).contains("summary"));
    let mut m = meta(0);
    m.summary = "é".repeat(120);
    assert!(
        m.validate().is_ok(),
        "the limit counts characters, not bytes"
    );
    assert!(rejected(|m| m.homepage = Some("ftp://example.org".into())).contains("homepage"));
    assert!(rejected(|m| m.homepage = Some("https://".into())).contains("homepage"));
    assert!(rejected(|m| m.homepage = Some("https://a b".into())).contains("homepage"));
}

#[test]
fn license_entries_are_single_identifiers() {
    for license in [
        "GPL-2.0+",
        "GPL-2.0-or-later",
        "LicenseRef-Proprietary",
        "bzip2-1.0.6",
    ] {
        let mut m = meta(0);
        m.license = vec![license.into()];
        assert!(m.validate().is_ok(), "{license}");
    }
    for license in [
        "",
        "+",
        "MIT OR Apache-2.0",
        "Apache 2",
        "GPL+2",
        "MIT++",
        "MIT,BSD",
    ] {
        let err = rejected(|m| m.license = vec![license.into()]);
        assert!(err.contains("license identifier"), "{license:?}: {err}");
    }
}

#[test]
fn identity_and_format_are_checked() {
    assert!(rejected(|m| m.rev = 0).contains("revisions start at 1"));
    assert!(rejected(|m| m.format = 2).contains("format 2"));
    assert!(rejected(|m| m.installed_size = (1 << 53) + 1).contains("installed_size"));
}

#[test]
fn paths_in_metadata_follow_the_payload_rules() {
    let err = rejected(|m| {
        m.alternatives = vec![Alternative {
            group: "cc".parse().expect("valid"),
            paths: vec!["/usr/bin/cc".into()],
            priority: 0,
        }];
    });
    assert!(err.contains("/usr/bin/cc"), "{err}");
    let err = rejected(|m| {
        m.alternatives = vec![Alternative {
            group: "cc".parse().expect("valid"),
            paths: vec![],
            priority: 0,
        }];
    });
    assert!(err.contains("no paths"), "{err}");
    let err = rejected(|m| {
        m.abi.insert(
            "lib/libz.so.1".into(),
            Abi {
                soname: "libz.so.1".parse().expect("valid"),
                digest: Blake3Hash::of(b"abi"),
            },
        );
    });
    assert!(err.contains("lib/libz.so.1"), "{err}");
}

#[test]
fn requested_privileges_are_checked() {
    let request = |path: &str| PrivRequest {
        path: path.into(),
        mode: 0o4755,
        owner: None,
        group: Some("wheel".into()),
        caps: None,
    };
    let mut m = meta(0);
    m.requested_privileges = vec![request("usr/bin/doas"), request("usr/bin/ping")];
    m.requested_privileges[1].caps = Some("cap_net_raw=ep".into());
    assert!(m.validate().is_ok(), "setuid bits are allowed in requests");

    let err =
        rejected(|m| m.requested_privileges = vec![request("usr/bin/su"), request("usr/bin/doas")]);
    assert!(err.contains("requested_privileges"), "{err}");
    let err = rejected(|m| {
        let mut r = request("usr/bin/doas");
        r.owner = Some("alice".into());
        m.requested_privileges = vec![r];
    });
    assert!(
        err.contains("alice") && err.contains("usr/bin/doas"),
        "{err}"
    );
    let err = rejected(|m| {
        let mut r = request("usr/bin/doas");
        r.caps = Some(String::new());
        m.requested_privileges = vec![r];
    });
    assert!(err.contains("capabilities"), "{err}");
    let err = rejected(|m| {
        let mut r = request("usr/bin/doas");
        r.mode = 0o10755;
        m.requested_privileges = vec![r];
    });
    assert!(err.contains("permission bits"), "{err}");
}

#[test]
fn static_deps_are_sorted_by_name_and_have_revisions() {
    let dep = |name: &str, rev| StaticDep {
        name: name.parse().expect("valid"),
        version: "1.0".parse().expect("valid"),
        rev,
        id: StoreId::from_manifest_hash(&Blake3Hash::of(name.as_bytes())),
    };
    let mut m = meta(0);
    m.static_deps = vec![dep("musl", 1), dep("zlib", 2)];
    assert!(m.validate().is_ok());
    assert!(
        rejected(|m| m.static_deps = vec![dep("zlib", 1), dep("musl", 1)]).contains("static_deps")
    );
    assert!(rejected(|m| m.static_deps = vec![dep("zlib", 0)]).contains("revision 0"));
}

#[test]
fn alternative_priorities_round_trip_and_zero_keeps_the_old_form() {
    let mut m = meta(0);
    m.alternatives = vec![Alternative {
        group: "vi".parse().expect("valid"),
        paths: vec!["usr/bin/vi".into()],
        priority: 0,
    }];
    let json = String::from_utf8(m.to_json().expect("valid")).expect("UTF-8");
    assert!(
        json.contains(r#""alternatives":[{"group":"vi","paths":["usr/bin/vi"]}]"#),
        "{json}"
    );
    m.alternatives[0].priority = -20;
    let json = m.to_json().expect("valid");
    assert_eq!(Meta::from_json(&json).expect("parses"), m);

    let err = rejected(|m| {
        m.alternatives = vec![Alternative {
            group: "vi".parse().expect("valid"),
            paths: vec!["usr/bin/vi".into()],
            priority: i64::MAX,
        }];
    });
    assert!(err.contains("priority"), "{err}");
}
