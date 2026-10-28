use proptest::prelude::*;

use super::*;
use crate::test_support::{dir, file, meta, package, symlink};

fn identity() -> Identity {
    meta(0).identity()
}

fn manifest(entries: Vec<Entry>) -> Result<Manifest, Error> {
    Manifest::new(identity(), Blake3Hash::of(b"meta"), entries)
}

fn reason(result: Result<Manifest, Error>) -> String {
    result.expect_err("manifest should be rejected").to_string()
}

#[test]
fn entries_sort_by_bytes_so_directories_precede_their_contents() {
    let m = manifest(vec![
        file("usr/a/x", b"x"),
        file("usr/a-b", b"y"),
        dir("usr/a"),
        dir("usr"),
        dir("etc"),
    ])
    .expect("valid manifest");
    let paths: Vec<_> = m.entries().iter().map(Entry::path).collect();
    assert_eq!(paths, ["etc", "usr", "usr/a", "usr/a-b", "usr/a/x"]);
}

#[test]
fn every_parent_must_be_a_directory_entry() {
    let err = reason(manifest(vec![dir("usr"), file("usr/bin/rg", b"rg")]));
    assert!(err.contains("parent `usr/bin`"), "{err}");
    assert!(err.contains("path:     usr/bin/rg"), "{err}");
    assert!(err.contains("package ripgrep-14.1.1-r1"), "{err}");
    let err = reason(manifest(vec![
        dir("usr"),
        symlink("usr/bin", "lib"),
        file("usr/bin/rg", b"rg"),
    ]));
    assert!(err.contains("parent `usr/bin`"), "{err}");
}

#[test]
fn top_level_entries_are_directories() {
    let err = reason(manifest(vec![symlink("usr", "/usr")]));
    assert!(err.contains("top-level"), "{err}");
}

#[test]
fn duplicate_paths_are_rejected() {
    let err = reason(manifest(vec![dir("usr"), dir("usr")]));
    assert!(err.contains("without duplicates"), "{err}");
}

#[test]
fn paths_outside_the_payload_rules_are_rejected() {
    for path in ["var", "usr/sbin", "usr/./bin"] {
        let entries = vec![dir("usr"), dir(path)];
        assert!(manifest(entries).is_err(), "{path}");
    }
}

#[test]
fn store_files_never_carry_write_or_privilege_bits() {
    let with_mode = |mode| Entry::File {
        path: "usr/rg".into(),
        mode,
        size: 0,
        hash: Blake3Hash::of(b""),
        owner: None,
        group: None,
    };
    for mode in [0o644, 0o4555, 0o2555, 0o10444] {
        assert!(
            manifest(vec![dir("usr"), with_mode(mode)]).is_err(),
            "{mode:o}"
        );
    }
    let setgid_dir = Entry::Dir {
        path: "usr".into(),
        mode: 0o2755,
        owner: None,
        group: None,
    };
    assert!(manifest(vec![setgid_dir]).is_err());
}

#[test]
fn owners_and_groups_come_from_the_registry() {
    let owned = |owner: Option<&str>, group: Option<&str>| Entry::Dir {
        path: "usr".into(),
        mode: 0o755,
        owner: owner.map(Into::into),
        group: group.map(Into::into),
    };
    assert!(manifest(vec![owned(Some("polkitd"), Some("polkitd"))]).is_ok());
    assert!(manifest(vec![owned(None, Some("wheel"))]).is_ok());
    let err = reason(manifest(vec![owned(Some("alice"), None)]));
    assert!(err.contains("owner `alice`"), "{err}");
    assert!(manifest(vec![owned(None, Some("nobody"))]).is_err());
    assert!(manifest(vec![owned(Some("wheel"), None)]).is_err());
    let err = reason(manifest(vec![owned(Some("root"), None)]));
    assert!(err.contains("must be omitted"), "{err}");
}

#[test]
fn symlink_targets_are_kept_verbatim_but_not_empty() {
    assert!(manifest(vec![dir("usr"), symlink("usr/lib64", "lib")]).is_err());
    let m = manifest(vec![dir("usr"), symlink("usr/x", "../../does/not/exist")])
        .expect("dangling symlinks are legal");
    assert_eq!(m.entries()[1], symlink("usr/x", "../../does/not/exist"));
    assert!(manifest(vec![dir("usr"), symlink("usr/x", "")]).is_err());
}

#[test]
fn zero_revision_is_rejected() {
    let mut identity = identity();
    identity.rev = 0;
    assert!(Manifest::new(identity, Blake3Hash::of(b"meta"), vec![dir("usr")]).is_err());
}

#[test]
fn stored_form_is_canonical_json_with_modes_in_decimal() {
    let m = manifest(vec![dir("usr")]).expect("valid manifest");
    let json = String::from_utf8(m.to_json()).expect("JSON is UTF-8");
    assert!(
        json.starts_with(r#"{"entries":[{"mode":493,"path":"usr","type":"dir"}],"format":1,"#),
        "{json}"
    );
}

#[test]
fn parsing_rejects_anything_but_the_canonical_form() {
    let m = manifest(vec![dir("usr"), file("usr/a", b"a"), file("usr/b", b"b")])
        .expect("valid manifest");
    let canonical = m.to_json();
    assert_eq!(
        Manifest::from_json(&canonical).expect("canonical form parses"),
        m
    );

    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("valid JSON");
    let pretty = serde_json::to_vec_pretty(&value).expect("serializable");
    let err = Manifest::from_json(&pretty).expect_err("pretty JSON is not canonical");
    assert!(err.to_string().contains("not canonical"), "{err}");

    let mut extra = value.clone();
    extra["extra"] = serde_json::json!(1);
    let extra = laft_core::to_canonical_json(&extra).expect("canonicalizable");
    assert!(Manifest::from_json(&extra).is_err());

    let mut unsorted = value.clone();
    unsorted["entries"]
        .as_array_mut()
        .expect("entries are an array")
        .swap(1, 2);
    let unsorted = laft_core::to_canonical_json(&unsorted).expect("canonicalizable");
    let err = Manifest::from_json(&unsorted).expect_err("unsorted entries are invalid");
    assert!(err.to_string().contains("sorted"), "{err}");

    let mut format = value;
    format["format"] = serde_json::json!(2);
    let format = laft_core::to_canonical_json(&format).expect("canonicalizable");
    assert!(Manifest::from_json(&format).is_err());
}

#[test]
fn file_names_follow_the_package_naming_scheme() {
    let mut identity = identity();
    assert_eq!(identity.file_name(), "ripgrep-14.1.1-r1.x86_64-v3.lpkg");
    identity.variant = Variant::Static;
    assert_eq!(
        identity.file_name(),
        "ripgrep-14.1.1-r1+static.x86_64-v3.lpkg"
    );
    identity.variant = Variant::Dynamic;
    identity.name = "adwaita-icon-theme".parse().expect("valid name");
    identity.version = "47.0".parse().expect("valid version");
    identity.arch = Arch::Any;
    assert_eq!(identity.file_name(), "adwaita-icon-theme-47.0-r1.any.lpkg");
}

#[test]
fn metadata_must_match_the_manifest() {
    let (meta, m) = package(vec![dir("usr"), file("usr/rg", b"rg")]);
    let bytes = meta.to_json().expect("valid metadata");
    assert!(m.check_meta(&meta, &bytes).is_ok());

    let mut other = meta.clone();
    other.summary = "Something else".into();
    let other_bytes = other.to_json().expect("valid metadata");
    let err = m
        .check_meta(&other, &other_bytes)
        .expect_err("hash differs");
    assert!(err.to_string().contains("metadata hash"), "{err}");

    let mut sized = meta;
    sized.installed_size = 3;
    let m = Manifest::new(
        sized.identity(),
        Blake3Hash::of(&sized.to_json().expect("valid metadata")),
        vec![dir("usr"), file("usr/rg", b"rg")],
    )
    .expect("valid manifest");
    let err = m
        .check_meta(&sized, &sized.to_json().expect("valid metadata"))
        .expect_err("installed size differs");
    assert!(err.to_string().contains("installed size"), "{err}");
}

fn arb_entries() -> impl Strategy<Value = Vec<Entry>> {
    proptest::collection::btree_map(
        "[a-c]{1,2}(/[a-c]{1,2}){0,2}",
        (0u8..3, proptest::collection::vec(any::<u8>(), 0..8)),
        0..12,
    )
    .prop_map(|leaves| {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert("usr".to_owned(), dir("usr"));
        for (leaf, (kind, content)) in leaves {
            let path = format!("usr/{leaf}");
            let mut parent = "usr".to_owned();
            for component in leaf
                .split('/')
                .collect::<Vec<_>>()
                .split_last()
                .map_or(&[][..], |(_, p)| p)
            {
                parent = format!("{parent}/{component}");
                entries.insert(parent.clone(), dir(&parent));
            }
            let entry = match kind {
                0 => file(&path, &content),
                1 => symlink(&path, "target"),
                _ => dir(&path),
            };
            entries.entry(path).or_insert(entry);
        }
        entries.into_values().collect()
    })
}

proptest! {
    #[test]
    fn stored_form_round_trips_and_fixes_the_store_id(entries in arb_entries()) {
        let Ok(m) = manifest(entries) else {
            // A leaf generated as a file can also be another leaf's parent.
            return Ok(());
        };
        let parsed = Manifest::from_json(&m.to_json()).expect("stored form parses");
        prop_assert_eq!(parsed.store_id(), m.store_id());
        prop_assert_eq!(parsed, m);
    }

    #[test]
    fn any_change_to_a_file_changes_the_store_id(content in proptest::collection::vec(any::<u8>(), 0..16), mode in prop_oneof![Just(0o444u32), Just(0o555)]) {
        let base = manifest(vec![dir("usr"), file("usr/f", b"base")]).expect("valid manifest");
        let changed = manifest(vec![
            dir("usr"),
            Entry::File {
                path: "usr/f".into(),
                mode,
                size: content.len() as u64,
                hash: Blake3Hash::of(&content),
                owner: None,
                group: None,
            },
        ])
        .expect("valid manifest");
        prop_assert_eq!(changed.store_id() == base.store_id(), changed == base);
    }

    #[test]
    fn parsing_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = Manifest::from_json(&bytes);
    }
}

#[test]
fn architectures_parse_from_their_text_form() {
    for arch in [Arch::X86_64V3, Arch::Any] {
        assert_eq!(arch.as_str().parse::<Arch>(), Ok(arch));
    }
    for text in ["", "aarch64", "x86_64", "ANY", "x86_64-v3 "] {
        assert!(text.parse::<Arch>().is_err(), "{text:?}");
    }
}
