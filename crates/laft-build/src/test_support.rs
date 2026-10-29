//! Recipes and items built by hand for unit tests.

use std::collections::BTreeMap;

use laft_core::{PkgRef, Variant};

use crate::recipe::{
    BuildOptions, Ci, Metadata, Operations, Outputs, Phases, Recipe, Runner, Subpackage,
};
use crate::{Item, Split};

/// A recipe of `name` at version 1.0 for the variant of `item`, written
/// like a package reference, with nothing else set.
pub(crate) fn recipe(item: &str) -> Recipe {
    let item = self::item(item);
    Recipe {
        name: item.name,
        version: "1.0".parse().expect("a valid version"),
        rev: 1,
        variant: item.variant,
        repos: vec!["core".parse().expect("a valid name")],
        meta: Metadata {
            summary: "A package".into(),
            description: None,
            homepage: None,
            license: vec!["MIT".into()],
            license_files: None,
        },
        sources: Vec::new(),
        vendors: Vec::new(),
        patches: Vec::new(),
        patch_strip: 1,
        files: Vec::new(),
        build_deps: Vec::new(),
        link_deps: Vec::new(),
        check_deps: Vec::new(),
        phases: Phases {
            prepare: None,
            configure: None,
            build: None,
            check: None,
            install: None,
            post_install: None,
        },
        env: BTreeMap::new(),
        options: BuildOptions {
            check: true,
            parallel: true,
            strip: true,
            debug_split: true,
            static_pie: true,
            keep_archives: false,
            lto: false,
            empty: false,
            keep_dirs: Vec::new(),
        },
        outputs: Outputs {
            split: match item.variant {
                Variant::Dynamic => vec![Split::Dbg, Split::Dev, Split::Doc],
                Variant::Static => vec![Split::Dbg],
            },
            subpackages: BTreeMap::new(),
            alternatives: BTreeMap::new(),
            deps: Vec::new(),
            provides: Vec::new(),
            conflicts: Vec::new(),
            replaces: Vec::new(),
            ignore_deps: Vec::new(),
            setuid: Vec::new(),
            caps: BTreeMap::new(),
            ownership: BTreeMap::new(),
        },
        smoke: None,
        abi_epoch: 0,
        toolchain_epoch: 0,
        operations: Operations {
            maintainers: Vec::new(),
            update: None,
            cpe: Vec::new(),
            ci: Ci {
                runner: Runner::Standard,
                timeout_minutes: 360,
            },
        },
    }
}

/// Parses an item written like a package reference, `zlib:static`.
pub(crate) fn item(text: &str) -> Item {
    let r: PkgRef = text.parse().expect("a valid reference");
    Item::new(r.name().clone(), r.variant())
}

/// Parses package references.
pub(crate) fn refs(texts: &[&str]) -> Vec<PkgRef> {
    texts
        .iter()
        .map(|t| t.parse().expect("a valid reference"))
        .collect()
}

/// An explicit subpackage holding `usr/lib/*.so.*`.
pub(crate) fn subpackage() -> Subpackage {
    Subpackage {
        summary: "Libraries".into(),
        files: vec!["usr/lib/*.so.*".into()],
        exclude: Vec::new(),
        deps: Vec::new(),
        provides: Vec::new(),
        conflicts: Vec::new(),
        replaces: Vec::new(),
        depends_on_main: false,
        license: vec!["MIT".into()],
        keep_empty: false,
    }
}
