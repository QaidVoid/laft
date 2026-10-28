use laft_core::Timestamp;
use proptest::prelude::*;

use super::*;
use crate::test_support::{DAY, T0, entry, index, meta};

fn yank(name: &str) -> Yank {
    let gone = entry(name, "0.9");
    Yank {
        id: gone.id,
        name: gone.meta.name,
        version: gone.meta.version,
        rev: 1,
        variant: Variant::Dynamic,
        reason: "broken build".into(),
        time: Timestamp::from_secs(T0),
    }
}

#[test]
fn encoded_index_decodes_to_the_same_index() {
    let mut original = index(T0);
    original.yanked.push(yank("hello"));
    original
        .defaults
        .providers
        .insert("so:libz.so.1".into(), "zlib".parse().expect("valid name"));
    let bytes = original.encode().expect("valid");
    assert_eq!(Index::decode(&bytes).expect("decodes"), original);
    assert_eq!(
        decompress(&bytes, MAX_INDEX_SIZE).expect("zstd"),
        original.to_json().expect("valid")
    );
}

#[test]
fn entries_hold_metadata_and_entry_fields_in_one_object() {
    let json = serde_json::to_value(entry("hello", "1.0")).expect("serializable");
    let object = json.as_object().expect("an object");
    for field in ["id", "file", "size", "blake3", "name", "version", "summary"] {
        assert!(object.contains_key(field), "{field}");
    }
}

#[test]
fn unknown_entry_fields_are_rejected() {
    let mut json = serde_json::to_value(entry("hello", "1.0")).expect("serializable");
    json.as_object_mut()
        .expect("an object")
        .insert("mirror".into(), "x".into());
    assert!(serde_json::from_value::<IndexEntry>(json).is_err());
}

#[test]
fn packages_must_follow_the_index_order() {
    let mut unsorted = index(T0);
    unsorted.packages.reverse();
    assert!(unsorted.validate().is_err());

    let mut versions = index(T0);
    versions.packages = vec![entry("hello", "2.0"), entry("hello", "1.0")];
    versions.validate().expect("newer versions come first");
    versions.packages.reverse();
    assert!(versions.validate().is_err());

    let mut variants = index(T0);
    let mut fixed = entry("hello", "1.0");
    fixed.meta.variant = Variant::Static;
    fixed.id = entry("hello-static", "1.0").id;
    fixed.file = format!("pool/{}.lpkg", fixed.id);
    variants.packages = vec![entry("hello", "1.0"), fixed];
    variants.validate().expect("dynamic comes before static");
    variants.packages.reverse();
    assert!(variants.validate().is_err());
}

#[test]
fn expiry_must_follow_the_signing_time_within_30_days() {
    let mut late = index(T0);
    late.expires = Timestamp::from_secs(T0 + 30 * DAY);
    late.validate().expect("30 days is the limit");
    late.expires = Timestamp::from_secs(T0 + 30 * DAY + 1);
    assert!(late.validate().is_err());

    let mut early = index(T0);
    early.expires = early.timestamp;
    assert!(early.validate().is_err());
}

#[test]
fn entry_file_must_be_its_pool_path() {
    let mut bad = index(T0);
    bad.packages[0].file = "pool/other.lpkg".into();
    assert!(bad.validate().is_err());
}

#[test]
fn yanked_package_cannot_also_be_available() {
    let mut both = index(T0);
    let mut gone = yank("hello");
    gone.id = both.packages[0].id;
    both.yanked.push(gone);
    assert!(both.validate().is_err());

    let mut same = index(T0);
    let mut gone = yank("hello");
    gone.version = same.packages[0].meta.version.clone();
    same.yanked.push(gone);
    let error = same.validate().expect_err("the identity is available");
    assert!(error.to_string().contains("also available"), "{error}");
}

#[test]
fn yanks_are_sorted_without_duplicates() {
    let mut sorted = index(T0);
    sorted.yanked = vec![yank("hello"), yank("zlib")];
    sorted.validate().expect("sorted by name");

    let mut unsorted = sorted.clone();
    unsorted.yanked.reverse();
    assert!(unsorted.validate().is_err());

    let mut twice = index(T0);
    twice.yanked = vec![yank("hello"), yank("hello")];
    assert!(twice.validate().is_err());

    let mut same_id = index(T0);
    let mut other = yank("zlib");
    other.id = yank("hello").id;
    same_id.yanked = vec![yank("hello"), other];
    assert!(same_id.validate().is_err());
}

#[test]
fn invalid_provider_defaults_are_rejected() {
    let mut bad = index(T0);
    bad.defaults
        .providers
        .insert("so:".into(), "zlib".parse().expect("valid name"));
    assert!(bad.validate().is_err());
}

#[test]
fn decompression_stops_at_the_limit() {
    let bytes = zstd::encode_all(&[b'a'; 1001][..], 1).expect("compressible");
    assert_eq!(decompress(&bytes, 1001).expect("at the limit").len(), 1001);
    let err = decompress(&bytes, 1000).expect_err("over the limit");
    assert!(
        matches!(err.kind(), ErrorKind::TooLarge { limit: 1000 }),
        "{err}"
    );
}

#[test]
fn non_zstd_input_is_rejected() {
    assert!(Index::decode(b"{\"format\":1}").is_err());
    assert!(Index::decode(b"").is_err());
}

#[test]
fn generations_come_from_the_signing_time_and_increase() {
    let signed = Timestamp::from_secs(T0);
    assert_eq!(next_generation(signed, None).expect("first"), T0);
    assert_eq!(next_generation(signed, Some(T0 - 1)).expect("later"), T0);
    assert!(next_generation(signed, Some(T0)).is_err());
}

/// A package identity: name, version, revision and static variant.
type Identity = (String, String, u32, bool);

fn arb_identity() -> impl Strategy<Value = Identity> {
    (
        "[a-z][a-z0-9]{0,5}",
        "[0-9]{1,2}(\\.[0-9]{1,2}){0,2}",
        1u32..4,
        any::<bool>(),
    )
}

fn package((name, version, rev, fixed): &Identity, any_arch: bool) -> IndexEntry {
    let mut entry = entry(name, version);
    entry.meta = meta(name, version);
    entry.meta.rev = *rev;
    if *fixed {
        entry.meta.variant = Variant::Static;
    }
    if any_arch {
        entry.meta.arch = Arch::Any;
    }
    entry.id = StoreId::from_manifest_hash(&Blake3Hash::of(
        format!("{name}-{version}-{rev}-{fixed}").as_bytes(),
    ));
    entry.file = format!("pool/{}.lpkg", entry.id);
    entry
}

fn arb_index() -> impl Strategy<Value = Index> {
    (
        prop::collection::vec((arb_identity(), any::<bool>()), 0..6),
        prop::collection::vec((arb_identity(), "[a-z ]{1,12}"), 0..4),
        prop::collection::btree_map("[a-z]{1,4}", "[a-z]{1,4}", 0..3),
        prop::collection::btree_map("[a-z]{1,4}", "[a-z]{1,4}", 0..3),
        0..=MAX_SAFE_INTEGER,
        0..=Timestamp::MAX.as_secs() - MAX_VALIDITY_SECS,
        1..=MAX_VALIDITY_SECS,
    )
        .prop_map(
            |(packages, yanks, alternatives, providers, generation, signed, validity)| {
                let mut packages: Vec<IndexEntry> =
                    packages.iter().map(|(id, any)| package(id, *any)).collect();
                packages.sort_by(|a, b| {
                    let key = |e: &IndexEntry| {
                        let m = &e.meta;
                        (
                            m.name.clone(),
                            Reverse((m.version.clone(), m.rev)),
                            m.variant,
                        )
                    };
                    key(a).cmp(&key(b))
                });
                packages.dedup_by(|a, b| {
                    let key = |e: &IndexEntry| {
                        let m = &e.meta;
                        (m.name.clone(), m.version.clone(), m.rev, m.variant)
                    };
                    key(a) == key(b) || a.id == b.id
                });
                let mut yanked: Vec<Yank> = yanks
                    .iter()
                    .map(|(id, reason)| {
                        let gone = package(id, false);
                        Yank {
                            id: StoreId::from_manifest_hash(&Blake3Hash::of(
                                format!("yanked {}", gone.id).as_bytes(),
                            )),
                            name: gone.meta.name,
                            version: gone.meta.version,
                            rev: gone.meta.rev,
                            variant: gone.meta.variant,
                            reason: reason.clone(),
                            time: Timestamp::from_secs(signed),
                        }
                    })
                    .filter(|y| {
                        !packages.iter().any(|p| {
                            let m = &p.meta;
                            (&m.name, &m.version, m.rev, m.variant)
                                == (&y.name, &y.version, y.rev, y.variant)
                        })
                    })
                    .collect();
                let key = |y: &Yank| {
                    (
                        y.name.clone(),
                        Reverse((y.version.clone(), y.rev)),
                        y.variant,
                    )
                };
                yanked.sort_by_key(key);
                yanked.dedup_by(|a, b| key(a) == key(b) || a.id == b.id);
                let name = |s: &String| s.parse::<Name>().expect("generated names are valid");
                Index {
                    format: Index::FORMAT,
                    repo: "core".parse().expect("valid name"),
                    channel: "current".parse().expect("valid name"),
                    arch: Arch::X86_64V3,
                    generation,
                    timestamp: Timestamp::from_secs(signed),
                    expires: Timestamp::from_secs(signed + validity),
                    defaults: Defaults {
                        alternatives: alternatives
                            .iter()
                            .map(|(g, m)| (name(g), name(m)))
                            .collect(),
                        providers: providers
                            .iter()
                            .map(|(lib, p)| (format!("so:lib{lib}.so.1"), name(p)))
                            .collect(),
                    },
                    packages,
                    yanked,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn generated_indexes_round_trip_with_stable_bytes(index in arb_index()) {
        let json = index.to_json().expect("generated indexes are valid");
        let parsed = Index::from_json(&json).expect("written JSON parses");
        prop_assert_eq!(&parsed, &index);
        prop_assert_eq!(parsed.to_json().expect("valid"), json);
    }
}
