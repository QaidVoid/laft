//! Post-processing: turning the tree a build installed into packages.
//!
//! [`process`] runs the steps below on the output directory, in this
//! order. Each step is a function of this module. A failing step stops the
//! build with its code.
//!
//! | Step | Function | What it does |
//! |---|---|---|
//! | P1 | [`check_output`] | the payload holds a file (`B101`) |
//! | P2 | [`normalize_paths`] | moves everything into `usr/`, deletes libtool and info index files (`B102`) |
//! | P3 | [`check_paths`] | only `usr/` and `etc/` (`B103`), static archives removed (`W110`) |
//! | P4 | [`static_layout`] | `static` builds: the sysroot layout, no shared objects (`B104`, `B105`) |
//! | P4b | [`strip_trigger_outputs`] | removes files that views generate (`W113`) |
//! | P5 | [`privileges`] | modes, owners, requested privileges (`B106`, `B111`, `B114`) |
//! | P6 | [`scan_elf`], [`soname_links`], [`check_private_libraries`], [`check_needs`] | search paths and library placement (`B107`, `B108`, `B112`, `B113`, `W111`, `W114`, `W115`) |
//! | P7 | [`strip`] | strips ELF files, splits debug info |
//! | P8 | [`abi_digests`] | ABI digests of libraries |
//! | P9 | [`detect`], [`imports`], [`exports`] | dependencies and provides (`W112`), imports and exports |
//! | P10 | [`static_deps`] | static libraries linked into a `static` build |
//! | P11 | [`split()`] | the main package and its subpackages (`B115`) |
//! | P12 | [`triggers`] | the triggers each package activates |
//! | P13 | [`alternatives`] | alternative groups (`R040`) |
//! | P14 | [`package()`] | metadata, manifest and store id (`B116`) |
//! | P15 | [`lint`] | suspicious contents (`B109`, `W101`, `W120`, `W121`, `W122`) |
//! | P16 | [`write_packages`] | the package files |
//! | P17 | [`smoke()`] | the recipe's smoke test (`B110`) |
//!
//! The builder runs P16 and P17 itself, because they need the output
//! directory and the build-side store.

mod deps;
mod elf;
mod layout;
mod lint;
mod package;
mod privileges;
mod smoke;
mod split;
mod strip;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use laft_core::{Blake3Hash, Soname, Variant};
use laft_pkg::StaticDep;

pub use deps::{Detected, alternatives, detect, exports, imports, static_deps, triggers};
pub(crate) use elf::read_elf;
pub use elf::{
    ElfFiles, abi_digests, check_needs, check_private_libraries, scan_elf, soname_links,
};
pub use layout::{
    check_output, check_paths, normalize_paths, static_layout, strip_trigger_outputs,
};
pub use lint::lint;
pub use package::{Package, Payload, package, write_packages};
pub use privileges::{Privileges, privileges};
pub use smoke::smoke;
pub use split::{Part, split};
pub use strip::{DEBUG_DIR, debug_path, strip};

use crate::Recipe;
use crate::error::{BuildError, Warning};
use crate::exec::Executor;

/// What post-processing works with besides the payload.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    /// The recipe that was built.
    pub recipe: &'a Recipe,
    /// The output directory, which the install phases filled.
    pub out: &'a Path,
    /// Runs `objcopy` and `strip` with the build view's tools.
    pub exec: &'a Executor,
    /// The sonames the packages of the build view provide, or `None` when
    /// they are unknown because the view does not come from the store.
    pub view_sonames: Option<&'a BTreeSet<Soname>>,
    /// The `static` packages that came into the build view through its
    /// link dependencies.
    pub view_statics: &'a [StaticDep],
}

/// What post-processing produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The packages, the main package first.
    pub packages: Vec<Package>,
    /// The ABI digest of every library, by soname.
    pub abi: BTreeMap<Soname, Blake3Hash>,
    /// What the payload imports from each library it needs.
    pub imports: BTreeMap<String, BTreeSet<String>>,
    /// What the payload's libraries export, by soname.
    pub exports: BTreeMap<String, BTreeSet<String>>,
    /// Everything the steps reported.
    pub warnings: Vec<Warning>,
}

/// Runs the post-processing steps P1 to P15 on the payload of a build.
///
/// # Errors
///
/// Fails with the code of the first step that fails, and when the payload
/// cannot be read or changed.
pub fn process(input: &Input<'_>) -> Result<Output, BuildError> {
    let (recipe, out) = (input.recipe, input.out);
    let mut warnings = Vec::new();
    check_output(out, recipe.options.empty)?;
    normalize_paths(out)?;
    check_paths(
        out,
        recipe.variant,
        recipe.options.keep_archives,
        &mut warnings,
    )?;
    if recipe.variant == Variant::Static {
        static_layout(out)?;
    }
    strip_trigger_outputs(out, &mut warnings)?;
    let privileges = privileges(out, &recipe.outputs)?;
    let elves = scan_elf(out, &mut warnings)?;
    soname_links(out, &elves)?;
    check_private_libraries(&elves)?;
    check_needs(&elves, input.view_sonames, &mut warnings);
    strip(out, &elves, &recipe.options, input.exec)?;
    let abi = abi_digests(&elves);
    let detected = detect(
        out,
        &elves,
        recipe.variant,
        &recipe.outputs.ignore_deps,
        &mut warnings,
    )?;
    let imports = imports(out, &elves, &input.exec.usr());
    let exports = exports(&elves);
    let static_deps = static_deps(recipe.variant, input.view_statics);
    let parts = split(out, recipe, &elves)?;
    let mut groups = alternatives(recipe, &parts)?;
    let entries = laft_pkg::scan(out)?;
    let payload = Payload {
        entries: &entries,
        elves: &elves,
        detected: &detected,
        privileges: &privileges,
        abi: &abi,
        static_deps: &static_deps,
    };
    let packages = parts
        .iter()
        .map(|part| {
            let groups = groups.remove(&part.name).unwrap_or_default();
            package(payload, recipe, part, groups)
        })
        .collect::<Result<Vec<_>, _>>()?;
    lint(out, &privileges.world_writable, &mut warnings)?;
    Ok(Output {
        packages,
        abi: abi.into_values().map(|a| (a.soname, a.digest)).collect(),
        imports,
        exports,
        warnings,
    })
}
