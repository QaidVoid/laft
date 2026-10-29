//! The host side of recipes: the prelude, the std catalog, the symbol types
//! and the `laft/target` module.

use std::path::PathBuf;

use laft_core::{PkgRef, Variant};
use stav::{Host, HostModule, Source, SymbolType};

use crate::Target;
use crate::loader::{RepoLoader, Scope};

/// The schema of recipes and repository descriptors.
pub(crate) const PRELUDE: &str = include_str!("../stav/prelude/recipe.stav");

/// The name of the prelude in diagnostics.
pub(crate) const PRELUDE_NAME: &str = "<prelude>/recipe.stav";

/// The import path of the target module.
pub(crate) const TARGET_MODULE: &str = "laft/target";

/// Returns the host for the repositories `roots`, reading what `scope`
/// allows. The `laft/target` module holds the values of the `dynamic`
/// target, and [`Target::values`] replaces them for one evaluation.
pub(crate) fn host(roots: Vec<PathBuf>, scope: Scope) -> Host {
    let mut host = Host::new(RepoLoader::new(roots, scope));
    host.prelude.push(Source::new(PRELUDE_NAME, PRELUDE));
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
    host.host_modules.push(HostModule {
        path: TARGET_MODULE.to_owned(),
        source: Target::new(Variant::Dynamic).module_source(),
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
/// license or exception identifier: letters, digits, `.` and `-`,
/// optionally followed by `+`.
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
