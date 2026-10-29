//! The host side of the system configuration: prelude, base part, std
//! catalog, symbol types and implicit primary groups.

use std::path::Path;

use laft_core::PkgRef;
use stav::{Host, ImplicitRefs, Source, SymbolType, Value};

use crate::loader::DiskLoader;

/// The schema of the system configuration.
const PRELUDE: &str = include_str!("../stav/prelude/system.stav");

/// The part every system includes before its entry file.
const BASE: &str = include_str!("../stav/std/base.stav");

/// The name of the prelude in diagnostics.
pub(crate) const PRELUDE_NAME: &str = "<prelude>/system.stav";

/// The name of the base part in diagnostics.
pub(crate) const BASE_NAME: &str = "std/base.stav";

/// Returns the host of a system configuration in `root_dir`.
pub(crate) fn host(root_dir: &Path) -> Host {
    let mut host = Host::new(DiskLoader::new(root_dir));
    host.prelude.push(Source::new(PRELUDE_NAME, PRELUDE));
    host.implicit_parts.push(Source::new(BASE_NAME, BASE));
    host.symbols = vec![
        SymbolType {
            name: "Pkg".to_owned(),
            words: true,
            strings: false,
            constructor: "pkg".to_owned(),
            parse: parse_pkg,
        },
        SymbolType {
            name: "Spdx".to_owned(),
            words: false,
            strings: false,
            constructor: "spdx".to_owned(),
            parse: parse_spdx,
        },
    ];
    host.implicit_refs.push(ImplicitRefs {
        target: "Group".to_owned(),
        labels: implicit_groups,
    });
    host
}

/// Returns the canonical text of a package reference such as
/// `llvm@19:static`.
pub(crate) fn parse_pkg(text: &str) -> Result<String, String> {
    text.parse::<PkgRef>()
        .map(|r| r.to_string())
        .map_err(|e| format!("`{text}` is not a package reference: {e}"))
}

/// Returns a license identifier unchanged when it is written like an SPDX
/// license identifier: letters, digits, `.` and `-`, optionally followed
/// by `+`.
pub(crate) fn parse_spdx(text: &str) -> Result<String, String> {
    let id = text.strip_suffix('+').unwrap_or(text);
    let valid = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    if valid {
        Ok(text.to_owned())
    } else {
        Err(format!(
            "`{text}` is not a license identifier of letters, digits, `.` and `-`, optionally followed by `+`"
        ))
    }
}

/// Returns the names of the implicit primary groups of the root value
/// `root`: one for every user without an explicit `group` whose name no
/// declared group has.
pub(crate) fn implicit_groups(root: &Value) -> Vec<String> {
    let users = root.get("system").and_then(|s| s.get("users"));
    let entries = |field| match users.and_then(|u| u.get(field)) {
        Some(Value::Map(entries)) => entries.as_slice(),
        _ => &[],
    };
    let groups = entries("groups");
    entries("accounts")
        .iter()
        .filter(|(_, user)| matches!(user.get("group"), Some(Value::None) | None))
        .filter(|(name, _)| !groups.iter().any(|(group, _)| group == name))
        .map(|(name, _)| name.clone())
        .collect()
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
