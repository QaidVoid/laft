use super::{MAX_NAME_CHARS, OptionValue, is_valid_name};
use crate::to_canonical_json;

#[test]
fn values_parse_from_their_text_forms() {
    let cases = [
        ("true", OptionValue::Bool(true)),
        ("false", OptionValue::Bool(false)),
        ("amd", OptionValue::Tag("amd".to_owned())),
        ("x11_sha", OptionValue::Tag("x11_sha".to_owned())),
    ];
    for (text, expected) in cases {
        assert_eq!(text.parse::<OptionValue>().unwrap(), expected, "{text}");
        assert_eq!(expected.to_string(), text);
    }
}

#[test]
fn values_reject_bad_text() {
    for text in ["", "True", "1x", "a-b", "a b", "vulkan=", "_x", "täg"] {
        assert!(text.parse::<OptionValue>().is_err(), "{text}");
    }
}

#[test]
fn names_follow_the_option_grammar() {
    for name in ["a", "llvm", "x11", "a1_b2", &"a".repeat(MAX_NAME_CHARS)] {
        assert!(is_valid_name(name), "{name}");
    }
    for name in [
        "",
        "A",
        "_a",
        "1a",
        "a-b",
        "a.b",
        &"a".repeat(MAX_NAME_CHARS + 1),
    ] {
        assert!(!is_valid_name(name), "{name}");
    }
}

#[test]
fn values_render_and_read_back_as_json() {
    let json = to_canonical_json(&OptionValue::Bool(true)).unwrap();
    assert_eq!(json, b"true");
    let json = to_canonical_json(&OptionValue::tag("intel")).unwrap();
    assert_eq!(json, b"\"intel\"");
    let value: OptionValue = serde_json::from_slice(b"\"none\"").unwrap();
    assert_eq!(value, OptionValue::tag("none"));
}
