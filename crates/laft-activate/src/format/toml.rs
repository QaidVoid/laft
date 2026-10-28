//! A TOML renderer for JSON values.

use std::fmt::Write as _;

use serde_json::{Map, Value};

use super::quote;

/// Renders `value`, which must be an object, as a TOML document.
///
/// Keys keep the order of the object. Scalars and arrays of scalars come
/// first in each table, then subtables as `[a.b]` sections and arrays of
/// objects as `[[a.b]]` sections. A `null` member is left out. An array
/// that mixes objects with other values, or that holds `null`, cannot be
/// written.
pub(crate) fn render(value: &Value) -> Result<String, String> {
    let Value::Object(map) = value else {
        return Err("TOML content must be a map or a struct".into());
    };
    let mut out = String::new();
    table(&mut out, &mut Vec::new(), map)?;
    Ok(out)
}

fn table<'a>(
    out: &mut String,
    path: &mut Vec<&'a str>,
    map: &'a Map<String, Value>,
) -> Result<(), String> {
    for (key, value) in map {
        if value.is_null() || value.is_object() || is_table_array(value)? {
            continue;
        }
        let _ = writeln!(out, "{} = {}", self::key(key), inline(value)?);
    }
    for (key, value) in map {
        path.push(key);
        match value {
            Value::Object(sub) => {
                header(out, &format!("[{}]", dotted(path)));
                table(out, path, sub)?;
            }
            Value::Array(items) if is_table_array(value)? => {
                for item in items {
                    if let Value::Object(sub) = item {
                        header(out, &format!("[[{}]]", dotted(path)));
                        table(out, path, sub)?;
                    }
                }
            }
            _ => {}
        }
        path.pop();
    }
    Ok(())
}

/// Whether `value` is a non-empty array of objects. An array that mixes
/// objects with other values is an error.
fn is_table_array(value: &Value) -> Result<bool, String> {
    let Value::Array(items) = value else {
        return Ok(false);
    };
    let tables = items.iter().filter(|item| item.is_object()).count();
    if tables > 0 && tables < items.len() {
        return Err("a TOML list cannot mix tables with other values".into());
    }
    Ok(tables > 0)
}

fn header(out: &mut String, header: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(header);
    out.push('\n');
}

fn dotted(path: &[&str]) -> String {
    path.iter().map(|k| key(k)).collect::<Vec<_>>().join(".")
}

/// A key, bare when it is made only of `A-Za-z0-9_-`, otherwise quoted.
fn key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare { key.to_owned() } else { quote(key) }
}

/// A value written in place: a scalar, an array or an inline table.
fn inline(value: &Value) -> Result<String, String> {
    Ok(match value {
        Value::Null => return Err("TOML cannot represent a null inside a list".into()),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => quote(s),
        Value::Array(items) => {
            is_table_array(value)?;
            let items: Vec<String> = items.iter().map(inline).collect::<Result<_, _>>()?;
            format!("[{}]", items.join(", "))
        }
        Value::Object(map) => {
            let mut members = Vec::new();
            for (k, v) in map {
                if !v.is_null() {
                    members.push(format!("{} = {}", key(k), inline(v)?));
                }
            }
            if members.is_empty() {
                "{}".to_owned()
            } else {
                format!("{{ {} }}", members.join(", "))
            }
        }
    })
}

#[cfg(test)]
#[path = "toml_tests.rs"]
mod tests;
