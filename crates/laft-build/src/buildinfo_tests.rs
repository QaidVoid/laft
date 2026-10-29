use super::*;
use crate::test_support::recipe;
use crate::{Fetch, Source, Vendor};

fn host(started: &str) -> BuildinfoHost {
    BuildinfoHost {
        laft_build: "0.1.0".into(),
        kernel: "6.17.5".into(),
        jobs: 16,
        started: started.parse().expect("a timestamp"),
        finished: "2026-10-28T10:03:12Z".parse().expect("a timestamp"),
    }
}

fn ripgrep() -> Buildinfo {
    let mut r = recipe("ripgrep:static");
    r.version = "14.1.1".parse().expect("a version");
    r.toolchain_epoch = 1;
    r.sources = vec![Source {
        label: "main".into(),
        fetch: Fetch::Url {
            urls: vec!["https://example.org/ripgrep-14.1.1.tar.gz".into()],
            sha256: Sha256Hash::from_bytes([0xab; 32]),
            file: "ripgrep-14.1.1.tar.gz".into(),
            extract: true,
        },
        dest: String::new(),
        strip_components: None,
    }];
    r.vendors = vec![Vendor {
        kind: VendorKind::Cargo,
        sha256: Sha256Hash::from_bytes([0xcd; 32]),
        dir: String::new(),
        source: "main".into(),
    }];
    let id = |b| StoreId::from_manifest_hash(&Blake3Hash::from_bytes([b; 32]));
    let mut info = Buildinfo::for_recipe(&r, Blake3Hash::of(b"key"), host("2026-10-28T10:00:00Z"));
    info.recipe_repos = vec![RepoState {
        name: "core".parse().expect("a name"),
        commit: "4be1".into(),
        dirty: false,
    }];
    info.build_view = vec![ViewPackage {
        name: "pcre2".parse().expect("a name"),
        version: "10.46".parse().expect("a version"),
        rev: 1,
        variant: Variant::Static,
        id: id(1),
    }];
    info.env = BTreeMap::from([("SOURCE_DATE_EPOCH".into(), "315532800".into())]);
    info.outputs = vec![Output {
        name: "ripgrep".parse().expect("a name"),
        variant: Variant::Static,
        id: id(2),
        file: "ripgrep-14.1.1-r1+static.x86_64-v3.lpkg".into(),
    }];
    info.abi = BTreeMap::from([(
        "libpcre2-8.so.0".parse().expect("a soname"),
        Blake3Hash::of(b"abi"),
    )]);
    info
}

#[test]
fn buildinfo_is_written_as_canonical_json() {
    let info = ripgrep();
    let json = String::from_utf8(info.to_json()).expect("UTF-8");
    insta::assert_snapshot!(json);
    assert_eq!(info.file_name(), "ripgrep-14.1.1-r1+static.buildinfo.json");
    assert_eq!(Buildinfo::from_json(json.as_bytes()).ok(), Some(info));
}

#[test]
fn bootstrap_builds_say_so_and_others_do_not() {
    let mut info = ripgrep();
    assert!(!String::from_utf8_lossy(&info.to_json()).contains("bootstrap"));
    info.bootstrap = true;
    assert!(String::from_utf8_lossy(&info.to_json()).contains("\"bootstrap\":true"));
}

#[test]
fn other_formats_and_unknown_fields_are_refused() {
    let json = String::from_utf8(ripgrep().to_json()).expect("UTF-8");
    let newer = json.replacen("\"format\":1", "\"format\":2", 1);
    assert!(matches!(
        Buildinfo::from_json(newer.as_bytes()),
        Err(BuildinfoError::Format(2))
    ));
    let extra = json.replacen("{", "{\"extra\":1,", 1);
    assert!(matches!(
        Buildinfo::from_json(extra.as_bytes()),
        Err(BuildinfoError::Json(_))
    ));
}

#[test]
fn rebuilds_differ_only_in_the_host() {
    let first = ripgrep();
    let mut again = ripgrep();
    again.host = host("2026-10-29T08:00:00Z");
    assert!(first.same_build(&again));
    again.outputs[0].file = "other.lpkg".into();
    assert!(!first.same_build(&again));
}
