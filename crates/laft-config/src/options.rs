//! The package options of the enabled repositories.
//!
//! The indexes of the enabled repositories record the options every
//! recipe declares. This module renders their declarations as the Stav
//! source of the `PackageOptions` prelude module, so that
//! `package.<name>.<option>` is an ordinary, type checked attribute with
//! the usual suggestions for typos.

use std::collections::BTreeSet;

use laft_core::{OptionValue, is_valid_name};
use laft_index::OptionType;

use crate::IndexOptions;

/// The name of the generated module in diagnostics.
pub(crate) const MODULE_NAME: &str = "<prelude>/package-options.stav";

/// The head of the generated module.
const HEADER: &str = "\
# The package options of the enabled repositories, rendered from the
# `options` members of their indexes. This module is generated on every
# run and is not part of the configuration.
";

/// One rendered option of one package.
struct Rendered {
    field: String,
    ty: String,
    default: String,
    doc: String,
    sum: Option<String>,
}

/// Renders the Stav source of the `PackageOptions` module.
///
/// Every package whose declarations the indexes publish gets one struct
/// of options, one sum type per tag option, and a field in
/// `PackageOptions` that defaults to the declared defaults. Packages and
/// options whose name cannot be a field of a struct, because it is a
/// keyword or does not lex as one name, are left out.
pub(crate) fn module(options: &IndexOptions) -> String {
    let mut out = String::from(HEADER);
    let mut used = BTreeSet::new();
    used.insert("PackageOptions".to_owned());
    let mut fields = Vec::new();
    for (name, decls) in &options.options {
        let Some(field) = field_name(name.as_str()) else {
            continue;
        };
        let prefix = fresh(&pascal(name.as_str()), &mut used);
        let struct_name = format!("{prefix}_Options");
        let mut own = Vec::new();
        for decl in decls {
            let Some(option) = field_name(&decl.name) else {
                continue;
            };
            let (ty, sum) = match &decl.kind {
                OptionType::Bool(_) => ("Bool".to_owned(), None),
                OptionType::Tags(tags) => {
                    if !tags.iter().all(|t| valid_tag(t)) {
                        continue;
                    }
                    let ty = fresh(&format!("{prefix}_{}", pascal(&decl.name)), &mut used);
                    let sum = format!(
                        "\n## The values of the option {option}.\ntype {ty} = {}\n",
                        tags.join(" | ")
                    );
                    (ty, Some(sum))
                }
            };
            own.push(Rendered {
                field: option.to_owned(),
                ty,
                default: value(&decl.default),
                doc: doc(&decl.doc, &decl.name),
                sum,
            });
        }
        if own.is_empty() {
            continue;
        }
        used.insert(struct_name.clone());
        let width = own.iter().map(|o| o.field.len()).max().unwrap_or_default();
        let mut body = String::new();
        for option in &own {
            if let Some(sum) = &option.sum {
                out.push_str(sum);
            }
            let pad = " ".repeat(width - option.field.len());
            body.push_str(&format!(
                "  ## {}\n  {}: {}{} = {}\n",
                option.doc, option.field, pad, option.ty, option.default
            ));
        }
        out.push_str(&format!(
            "\n## The options of {name}.\nstruct {struct_name} {{\n{body}}}\n"
        ));
        fields.push(format!(
            "  ## The options of {name}.\n  {field}: {struct_name} = {struct_name} {{}}\n"
        ));
    }
    if fields.is_empty() {
        out.push_str("\n## No package declares options.\nstruct PackageOptions {}\n");
        return out;
    }
    out.push_str("\n## The package options, by package name.\nstruct PackageOptions {\n");
    for field in &fields {
        out.push_str(field);
    }
    out.push_str("}\n");
    out
}

/// Whether `tag` can be a variant of a generated sum type.
fn valid_tag(tag: &str) -> bool {
    is_valid_name(tag) && field_name(tag).is_some()
}

/// The doc comment of a declaration: its first line, without trailing
/// whitespace, or the option name when the declaration has none.
fn doc(doc: &str, option: &str) -> String {
    let line = doc.lines().next().unwrap_or_default().trim_end();
    if line.is_empty() {
        option.to_owned()
    } else {
        line.to_owned()
    }
}

/// The Stav text of a declared default.
fn value(default: &OptionValue) -> String {
    match default {
        OptionValue::Bool(true) => "true".to_owned(),
        OptionValue::Bool(false) => "false".to_owned(),
        OptionValue::Tag(tag) => tag.clone(),
    }
}

/// The name as a field of a struct, when it lexes as one plain lowercase
/// stav identifier that is no keyword.
fn field_name(name: &str) -> Option<&str> {
    const KEYWORDS: [&str; 19] = [
        "let", "fn", "struct", "type", "match", "if", "else", "for", "in", "import", "include",
        "as", "part", "self", "true", "false", "none", "where", "block",
    ];
    let bytes = name.as_bytes();
    let ident = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_';
    if bytes.is_empty() || !bytes[0].is_ascii_lowercase() || KEYWORDS.contains(&name) {
        return None;
    }
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'-' if bytes.get(i + 1).is_some_and(|n| ident(*n)) => {}
            b'-' => return None,
            b'.' if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) => {}
            b'.' => return None,
            _ if ident(b) => {}
            _ => return None,
        }
    }
    Some(name)
}

/// The name as part of a generated type name: every boundary starts a
/// new word, and a `.` and the digit after it are dropped.
fn pascal(name: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if upper {
                out.extend(c.to_uppercase());
                upper = false;
            } else {
                out.push(c);
            }
        } else {
            upper = true;
        }
    }
    out
}

/// Returns `base`, or `base_2`, `base_3` and so on, so that no two
/// generated types share a name.
fn fresh(base: &str, used: &mut BTreeSet<String>) -> String {
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            base.to_owned()
        } else {
            format!("{base}_{n}")
        };
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
