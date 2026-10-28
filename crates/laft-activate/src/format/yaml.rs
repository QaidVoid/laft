//! A YAML renderer for JSON values.

use serde_json::{Map, Value};

use super::quote;

/// Renders `value` as a YAML 1.2 document in block style.
///
/// Mappings keep the order of the object. A string is written plain only
/// when it cannot be read as anything else: it starts with a letter or `/`,
/// holds only `A-Za-z0-9_./-` and is not a word that some YAML reader takes
/// for a boolean or null, such as `yes`, `on` or `null`. Every other string
/// is double quoted.
pub(crate) fn render(value: &Value) -> String {
    let mut out = String::new();
    block(&mut out, value, 0);
    out
}

/// Writes `value` as a block at `indent` when it is a non-empty collection,
/// otherwise as a scalar line.
fn block(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Object(map) if !map.is_empty() => mapping(out, map, indent),
        Value::Array(items) if !items.is_empty() => sequence(out, items, indent),
        _ => {
            out.push_str(&scalar(value));
            out.push('\n');
        }
    }
}

fn is_block(value: &Value) -> bool {
    match value {
        Value::Object(map) => !map.is_empty(),
        Value::Array(items) => !items.is_empty(),
        _ => false,
    }
}

fn mapping(out: &mut String, map: &Map<String, Value>, indent: usize) {
    for (key, value) in map {
        pad(out, indent);
        out.push_str(&string(key));
        out.push(':');
        if is_block(value) {
            out.push('\n');
            block(out, value, indent + 2);
        } else {
            out.push(' ');
            out.push_str(&scalar(value));
            out.push('\n');
        }
    }
}

fn sequence(out: &mut String, items: &[Value], indent: usize) {
    for item in items {
        pad(out, indent);
        out.push('-');
        if is_block(item) {
            // The nested block starts on the dash's line: its first line's
            // indentation is replaced by the dash and one space.
            let mut nested = String::new();
            block(&mut nested, item, indent + 2);
            out.push(' ');
            out.push_str(nested.get(indent + 2..).unwrap_or(&nested));
        } else {
            out.push(' ');
            out.push_str(&scalar(item));
            out.push('\n');
        }
    }
}

fn pad(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', indent));
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => string(s),
        Value::Array(_) => "[]".into(),
        Value::Object(_) => "{}".into(),
    }
}

/// Words that YAML 1.1 or 1.2 readers resolve to booleans or null.
const KEYWORDS: [&str; 9] = ["true", "false", "yes", "no", "on", "off", "null", "y", "n"];

fn string(s: &str) -> String {
    let plain = s
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '/')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))
        && !KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(s));
    if plain { s.to_owned() } else { quote(s) }
}

#[cfg(test)]
#[path = "yaml_tests.rs"]
mod tests;
