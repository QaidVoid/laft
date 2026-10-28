use std::collections::BTreeMap;

use serde_json::json;

use super::*;

fn sections(value: Value) -> BTreeMap<String, BTreeMap<String, Value>> {
    serde_json::from_value(value).expect("a map of maps")
}

#[test]
fn ini_writes_sections_in_order_with_blank_lines_between() {
    let ini = ini(&sections(json!({
        "Network": { "NameResolvingService": "resolvconf" },
        "General": { "EnableNetworkConfiguration": true },
    })))
    .expect("renders");
    assert_eq!(
        ini,
        "[General]\nEnableNetworkConfiguration=true\n\n[Network]\nNameResolvingService=resolvconf\n"
    );
}

#[test]
fn ini_writes_the_unnamed_section_first_without_a_header() {
    let ini = ini(&sections(json!({
        "a": { "x": 1 },
        "": { "top": -3, "b": false },
    })))
    .expect("renders");
    assert_eq!(ini, "b=false\ntop=-3\n\n[a]\nx=1\n");
}

#[test]
fn ini_rejects_values_it_cannot_write() {
    for value in [
        json!([1]),
        json!(1.5),
        json!(null),
        json!({"a": 1}),
        json!("a\nb"),
    ] {
        let input = sections(json!({ "s": { "k": value } }));
        assert!(ini(&input).is_err(), "{value}");
    }
    assert!(ini(&sections(json!({ "s": { "a=b": 1 } }))).is_err());
    assert!(ini(&sections(json!({ "s]": { "a": 1 } }))).is_err());
}

#[test]
fn env_quotes_only_values_that_need_it() {
    let vars = BTreeMap::from([
        ("A".to_owned(), "x y".to_owned()),
        ("B".to_owned(), "plain".to_owned()),
        ("C".to_owned(), "/usr/bin:/bin".to_owned()),
        ("D".to_owned(), String::new()),
        ("E".to_owned(), "a\"b$c`d\\e".to_owned()),
    ]);
    assert_eq!(
        env(&vars).expect("renders"),
        "A=\"x y\"\nB=plain\nC=/usr/bin:/bin\nD=\"\"\nE=\"a\\\"b\\$c\\`d\\\\e\"\n"
    );
}

#[test]
fn env_rejects_keys_that_are_not_variable_names() {
    for key in ["1A", "A-B", "", "A B"] {
        let vars = BTreeMap::from([(key.to_owned(), "x".to_owned())]);
        assert_eq!(env(&vars), Err(key.to_owned()));
    }
}

#[test]
fn json_is_pretty_printed_with_two_spaces_and_a_final_line_feed() {
    let text = json(&json!({ "b": [1, 2], "a": { "c": "d" } })).expect("renders");
    assert_eq!(
        text,
        "{\n  \"a\": {\n    \"c\": \"d\"\n  },\n  \"b\": [\n    1,\n    2\n  ]\n}\n"
    );
}

#[test]
fn quoting_escapes_control_characters() {
    let input = format!("a\"b\\c\nd\te{}", char::from(1));
    assert_eq!(quote(&input), "\"a\\\"b\\\\c\\nd\\te\\u0001\"");
}
