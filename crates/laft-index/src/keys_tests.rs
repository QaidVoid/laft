use laft_core::Timestamp;
use proptest::prelude::*;

use super::*;
use crate::test_support::{TestKey, keys, public_keys};

#[test]
fn keys_round_trip_as_canonical_json() {
    let (a, b) = (TestKey::generate(), TestKey::generate());
    let mut doc = keys(
        3,
        vec![a.online(), b.online()],
        vec![TestKey::generate().id()],
    );
    doc.generation_floor = Some(1_761_600_000);
    let json = doc.to_json().expect("valid");
    assert_eq!(Keys::from_json(&json).expect("parses"), doc);
    assert_eq!(laft_core::to_canonical_json(&doc).expect("canonical"), json);
    assert_eq!(doc.comment().to_string(), "laft-keys repo=core version=3");
}

#[test]
fn absent_generation_floor_is_written_as_null() {
    let json = keys(1, vec![], vec![]).to_json().expect("valid");
    let text = String::from_utf8(json).expect("UTF-8");
    assert!(text.contains(r#""generation_floor":null"#), "{text}");
}

#[test]
fn key_id_must_match_the_public_key() {
    let (a, b) = (TestKey::generate(), TestKey::generate());
    let mut key = a.online();
    key.id = b.id();
    assert!(keys(1, vec![key], vec![]).validate().is_err());
}

#[test]
fn invalid_documents_are_rejected() {
    let a = TestKey::generate();
    let twice = keys(1, vec![a.online(), a.online()], vec![]);
    assert!(twice.validate().is_err());

    let revoked_twice = keys(1, vec![], vec![a.id(), a.id()]);
    assert!(revoked_twice.validate().is_err());

    let mut inverted = a.online();
    inverted.not_after = Timestamp::from_secs(inverted.not_before.as_secs() - 1);
    assert!(keys(1, vec![inverted], vec![]).validate().is_err());

    let mut future = keys(1, vec![], vec![]);
    future.format = 2;
    assert!(future.validate().is_err());

    let json = keys(1, vec![], vec![]).to_json().expect("valid");
    let text = String::from_utf8(json).expect("UTF-8");
    let extra = text.replacen('{', r#"{"extra":1,"#, 1);
    assert!(Keys::from_json(extra.as_bytes()).is_err());
}

fn arb_keys() -> impl Strategy<Value = Keys> {
    let windows = prop::collection::vec(
        (0..=Timestamp::MAX.as_secs(), 0..=Timestamp::MAX.as_secs()),
        4,
    );
    (
        0..=MAX_SAFE_INTEGER,
        prop::sample::subsequence((0..public_keys().len()).collect::<Vec<_>>(), 0..=4),
        windows,
        prop::sample::subsequence((0..public_keys().len()).collect::<Vec<_>>(), 0..=4),
        prop::option::of(0..=MAX_SAFE_INTEGER),
    )
        .prop_map(
            |(version, listed, windows, revoked, generation_floor)| Keys {
                format: Keys::FORMAT,
                repo: "core".parse().expect("valid name"),
                version,
                keys: listed
                    .into_iter()
                    .map(|i| {
                        let public = public_keys()[i].clone();
                        let (a, b) = windows[i];
                        OnlineKey {
                            id: public.id(),
                            public,
                            not_before: Timestamp::from_secs(a.min(b)),
                            not_after: Timestamp::from_secs(a.max(b)),
                        }
                    })
                    .collect(),
                revoked: revoked.into_iter().map(|i| public_keys()[i].id()).collect(),
                generation_floor,
            },
        )
}

proptest! {
    #[test]
    fn generated_keys_round_trip_with_stable_bytes(keys in arb_keys()) {
        let json = keys.to_json().expect("generated keys are valid");
        let parsed = Keys::from_json(&json).expect("written JSON parses");
        prop_assert_eq!(&parsed, &keys);
        prop_assert_eq!(parsed.to_json().expect("valid"), json);
    }
}
