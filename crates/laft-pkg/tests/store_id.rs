//! A fixed store id test vector.
//!
//! The fixture is the ripgrep example manifest. Its file and metadata hashes
//! are the BLAKE3 hashes of the bytes `rg` and `meta`. The expected id was
//! computed outside Rust with `b3sum` and `base32`, and it must never change.

use laft_core::{Blake3Hash, StoreId, Variant};
use laft_pkg::{Arch, Entry, Identity, Manifest};

const MANIFEST: &[u8] = include_bytes!("fixtures/manifest-ripgrep.json");
const ID: &str = include_str!("fixtures/manifest-ripgrep.id");

fn example() -> Manifest {
    let identity = Identity {
        name: "ripgrep".parse().expect("valid name"),
        version: "14.1.1".parse().expect("valid version"),
        rev: 1,
        variant: Variant::Dynamic,
        arch: Arch::X86_64V3,
    };
    let dir = |path: &str| Entry::Dir {
        path: path.into(),
        mode: 0o755,
        owner: None,
        group: None,
    };
    let entries = vec![
        dir("usr"),
        dir("usr/bin"),
        Entry::File {
            path: "usr/bin/rg".into(),
            mode: 0o555,
            size: 5321872,
            hash: Blake3Hash::of(b"rg"),
            owner: None,
            group: None,
        },
    ];
    Manifest::new(identity, Blake3Hash::of(b"meta"), entries).expect("example manifest is valid")
}

#[test]
fn example_manifest_serializes_to_the_fixture() {
    assert_eq!(
        String::from_utf8(example().to_json()).expect("UTF-8"),
        String::from_utf8(MANIFEST.to_vec()).expect("UTF-8")
    );
}

#[test]
fn example_manifest_has_the_fixed_store_id() {
    let expected: StoreId = ID.trim_end().parse().expect("fixture id is valid");
    assert_eq!(example().store_id(), expected);
    let parsed = Manifest::from_json(MANIFEST).expect("fixture parses");
    assert_eq!(parsed.store_id(), expected);
    assert_eq!(
        parsed.store_id().dir_name(
            &parsed.package().name,
            &parsed.package().version,
            Variant::Dynamic
        ),
        format!("{expected}-ripgrep-14.1.1")
    );
}
