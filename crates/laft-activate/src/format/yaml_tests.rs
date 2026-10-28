use serde_json::json;

use super::*;

#[test]
fn nested_values_render_in_block_style() {
    let value = json!({
        "name": "laft",
        "path": "/usr/bin/x",
        "count": 3,
        "enabled": false,
        "nothing": null,
        "empty_list": [],
        "empty_map": {},
        "servers": [
            { "host": "a.example", "ports": [80, 443] },
            { "host": "b.example" },
        ],
        "matrix": [[1, 2], [3]],
        "nested": { "deeper": { "key": "value" } },
    });
    insta::assert_snapshot!(render(&value));
}

#[test]
fn strings_that_read_as_other_types_are_quoted() {
    for (input, output) in [
        ("yes", "\"yes\"\n"),
        ("No", "\"No\"\n"),
        ("on", "\"on\"\n"),
        ("null", "\"null\"\n"),
        ("1.0", "\"1.0\"\n"),
        ("0x10", "\"0x10\"\n"),
        ("", "\"\"\n"),
        (".inf", "\".inf\"\n"),
        ("a: b", "\"a: b\"\n"),
        ("- x", "\"- x\"\n"),
        ("#x", "\"#x\"\n"),
        ("plain-word_1.2", "plain-word_1.2\n"),
    ] {
        assert_eq!(render(&json!(input)), output, "{input:?}");
    }
}

#[test]
fn scalars_and_empty_collections_render_on_one_line() {
    assert_eq!(render(&json!(null)), "null\n");
    assert_eq!(render(&json!(true)), "true\n");
    assert_eq!(render(&json!(-7)), "-7\n");
    assert_eq!(render(&json!([])), "[]\n");
    assert_eq!(render(&json!({})), "{}\n");
}
