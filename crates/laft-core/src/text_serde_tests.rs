use super::*;

#[test]
fn identifiers_serialize_as_their_text_form() {
    let dep: Dep = "zlib>=1.3,<2".parse().expect("valid dependency");
    let json = serde_json::to_string(&dep).expect("serializable");
    assert_eq!(json, r#""zlib>=1.3,<2""#);
    assert_eq!(
        serde_json::from_str::<Dep>(&json).expect("deserializable"),
        dep
    );

    let hash = Blake3Hash::of(b"payload");
    let json = serde_json::to_string(&hash).expect("serializable");
    assert_eq!(json, format!("\"{hash}\""));
    assert_eq!(
        serde_json::from_str::<Blake3Hash>(&json).expect("deserializable"),
        hash
    );
}

#[test]
fn invalid_text_fails_to_deserialize_with_the_parse_error() {
    let err = serde_json::from_str::<Name>(r#""Foo""#).expect_err("uppercase names are invalid");
    assert!(
        err.to_string().contains("invalid package name \"Foo\""),
        "{err}"
    );
    assert!(serde_json::from_str::<Variant>(r#""shared""#).is_err());
    assert!(serde_json::from_str::<Version>("1").is_err());
}
