use serde_json::json;

use super::*;

#[test]
fn nested_values_render_as_sections() {
    let value = json!({
        "unqualified-search-registries": ["docker.io"],
        "title": "a \"quoted\" title",
        "skipped": null,
        "count": 3,
        "ratio": 0.5,
        "server": {
            "host": "example.org",
            "tls": { "enabled": true },
            "ports": [80, 443],
        },
        "mirror": [
            { "url": "https://a.example", "weight": 1 },
            { "url": "https://b.example", "tags": [{ "name": "x" }] },
        ],
        "odd key": { "nested list": [[1, 2], []] },
    });
    insta::assert_snapshot!(render(&value).expect("renders"));
}

#[test]
fn an_empty_table_renders_nothing() {
    assert_eq!(render(&json!({})).expect("renders"), "");
}

#[test]
fn the_top_level_must_be_a_table() {
    for value in [json!(["a"]), json!("a"), json!(1), json!(null)] {
        assert!(render(&value).is_err(), "{value}");
    }
}

#[test]
fn lists_cannot_mix_tables_with_other_values() {
    assert!(render(&json!({ "a": [{ "b": 1 }, 2] })).is_err());
    assert!(render(&json!({ "a": [[{ "b": 1 }, 2]] })).is_err());
    assert!(render(&json!({ "a": [1, null] })).is_err());
}
