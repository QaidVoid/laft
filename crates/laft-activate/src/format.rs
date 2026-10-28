//! The structured content renderers of declared files: JSON, TOML, YAML,
//! INI and env.
//!
//! TOML and YAML are written by small renderers here instead of a library.
//! The input is always a JSON value, so only a fraction of either format is
//! needed, nothing is ever parsed, and the output stays byte for byte
//! predictable for the generation tree.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

pub(crate) mod toml;
pub(crate) mod yaml;

/// Renders `value` as JSON, pretty printed with two spaces, followed by a
/// line feed.
pub(crate) fn json(value: &Value) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

/// Renders INI sections in key order. The section named `""` comes first
/// and has no header. Keys are written as `key=value` in key order, and a
/// blank line separates sections. Values must be strings, integers or
/// booleans.
pub(crate) fn ini(sections: &BTreeMap<String, BTreeMap<String, Value>>) -> Result<String, String> {
    let mut blocks = Vec::new();
    for (name, keys) in sections {
        let mut block = String::new();
        if !name.is_empty() {
            if name.contains([']', '\n', '\r']) {
                return Err(format!("ini section name {name:?} cannot be written"));
            }
            let _ = writeln!(block, "[{name}]");
        }
        for (key, value) in keys {
            if key.is_empty() || key.contains(['=', '\n', '\r']) {
                return Err(format!("ini key {key:?} cannot be written"));
            }
            let value = match value {
                Value::String(s) if !s.contains(['\n', '\r']) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
                _ => {
                    return Err(format!(
                        "ini value of {key:?} must be a single line string, an integer or a boolean"
                    ));
                }
            };
            let _ = writeln!(block, "{key}={value}");
        }
        if !block.is_empty() {
            blocks.push(block);
        }
    }
    Ok(blocks.join("\n"))
}

/// Renders `KEY=value` lines in key order, or returns the first key that is
/// not a variable name.
pub(crate) fn env(vars: &BTreeMap<String, String>) -> Result<String, String> {
    let mut text = String::new();
    for (key, value) in vars {
        if !is_variable_name(key) {
            return Err(key.clone());
        }
        let _ = writeln!(text, "{key}={}", env_value(value));
    }
    Ok(text)
}

/// Whether `key` matches `[A-Za-z_][A-Za-z0-9_]*`.
fn is_variable_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A value for a `KEY=value` line: bare when it is made only of
/// `A-Za-z0-9_./:@%+,-`, otherwise in double quotes with `\`, `"`, `$` and
/// the backtick escaped by a backslash.
pub(crate) fn env_value(value: &str) -> String {
    let bare = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:@%+,-".contains(c));
    if bare {
        return value.to_owned();
    }
    let mut quoted = String::from("\"");
    for c in value.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// `s` as a double quoted string with the escapes that TOML basic strings
/// and YAML double quoted scalars share.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
