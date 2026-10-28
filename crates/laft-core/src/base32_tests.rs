use proptest::prelude::*;

use super::*;

#[test]
fn encodes_rfc_4648_test_vectors_in_lowercase() {
    let vectors = [
        ("f", "my"),
        ("fo", "mzxq"),
        ("foo", "mzxw6"),
        ("foob", "mzxw6yq"),
        ("fooba", "mzxw6ytb"),
        ("foobar", "mzxw6ytboi"),
    ];
    for (input, expected) in vectors {
        assert_eq!(encode(input.as_bytes()), expected);
    }
}

#[test]
fn rejects_non_canonical_and_foreign_text() {
    let canonical = encode(&[0xff; 32]);
    assert_eq!(decode::<32>(&canonical), Some([0xff; 32]));
    assert_eq!(decode::<32>(&canonical.to_uppercase()), None);
    assert_eq!(decode::<32>(&canonical[1..]), None);
    let mut padded = canonical.clone().into_bytes();
    *padded.last_mut().expect("encoding is not empty") = b'7';
    let padded = String::from_utf8(padded).expect("alphabet is ASCII");
    assert_eq!(
        decode::<32>(&padded),
        None,
        "nonzero padding bits must be rejected"
    );
}

proptest! {
    #[test]
    fn decode_inverts_encode(bytes in any::<[u8; 32]>()) {
        prop_assert_eq!(decode::<32>(&encode(&bytes)), Some(bytes));
    }
}
