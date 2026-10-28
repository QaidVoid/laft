use serde_json::json;

use super::*;

fn canonical(value: &Value) -> String {
    String::from_utf8(to_canonical_json(value).expect("value is canonicalizable"))
        .expect("canonical JSON is UTF-8")
}

#[test]
fn rfc_8785_sample_canonicalizes_exactly() {
    let input: Value = serde_json::from_str(
        r#"{
          "numbers": [333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
          "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
          "literals": [null, true, false]
        }"#,
    )
    .expect("sample is valid JSON");
    assert_eq!(
        canonical(&input),
        r#"{"literals":[null,true,false],"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],"string":"€$\u000f\nA'B\"\\\\\"/"}"#
    );
}

#[test]
fn members_sort_by_utf16_code_units() {
    let input: Value = serde_json::from_str(
        r#"{
          "€": "Euro Sign",
          "\r": "Carriage Return",
          "דּ": "Hebrew Letter Dalet With Dagesh",
          "1": "One",
          "😀": "Emoji: Grinning Face",
          "\u0080": "Control",
          "ö": "Latin Small Letter O With Diaeresis"
        }"#,
    )
    .expect("sample is valid JSON");
    let output = canonical(&input);
    let order = [
        "Carriage Return",
        "One",
        "Control",
        "Latin Small",
        "Euro",
        "Emoji",
        "Hebrew",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|v| output.find(v).expect("every member is present"))
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{output}");
}

#[test]
fn doubles_use_ecmascript_number_formatting() {
    let vectors: &[(u64, &str)] = &[
        (0x0000000000000000, "0"),
        (0x8000000000000000, "0"),
        (0x0000000000000001, "5e-324"),
        (0x8000000000000001, "-5e-324"),
        (0x7fefffffffffffff, "1.7976931348623157e+308"),
        (0xffefffffffffffff, "-1.7976931348623157e+308"),
        (0x4340000000000000, "9007199254740992"),
        (0xc340000000000000, "-9007199254740992"),
        (0x4430000000000000, "295147905179352830000"),
        (0x44b52d02c7e14af5, "9.999999999999997e+22"),
        (0x44b52d02c7e14af6, "1e+23"),
        (0x44b52d02c7e14af7, "1.0000000000000001e+23"),
        (0x444b1ae4d6e2ef4e, "999999999999999700000"),
        (0x444b1ae4d6e2ef4f, "999999999999999900000"),
        (0x444b1ae4d6e2ef50, "1e+21"),
        (0x3eb0c6f7a0b5ed8c, "9.999999999999997e-7"),
        (0x3eb0c6f7a0b5ed8d, "0.000001"),
        (0x41b3de4355555553, "333333333.3333332"),
        (0x41b3de4355555554, "333333333.33333325"),
        (0x41b3de4355555555, "333333333.3333333"),
        (0x41b3de4355555556, "333333333.3333334"),
        (0x41b3de4355555557, "333333333.33333343"),
        (0xbecbf647612f3696, "-0.0000033333333333333333"),
        (0x43143ff3c1cb0959, "1424953923781206.2"),
    ];
    for &(bits, expected) in vectors {
        let mut out = String::new();
        write_double(f64::from_bits(bits), &mut out);
        assert_eq!(out, expected, "bits {bits:#018x}");
    }
}

#[test]
fn integers_beyond_double_precision_are_rejected() {
    assert_eq!(canonical(&json!(9007199254740991_i64)), "9007199254740991");
    assert_eq!(
        canonical(&json!(-9007199254740991_i64)),
        "-9007199254740991"
    );
    for unsafe_value in [
        json!(9007199254740992_i64),
        json!(-9007199254740992_i64),
        json!(u64::MAX),
    ] {
        assert!(matches!(
            to_canonical_json(&unsafe_value),
            Err(CanonicalJsonError::UnsafeInteger(_))
        ));
    }
}

#[test]
fn control_characters_use_short_escapes_or_lowercase_hex() {
    assert_eq!(
        canonical(&json!("\u{8}\t\n\u{c}\r\u{1}\u{1f}\u{7f}")),
        "\"\\b\\t\\n\\f\\r\\u0001\\u001f\u{7f}\""
    );
}
