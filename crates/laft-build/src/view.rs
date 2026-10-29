//! Build views: the `usr` tree a build sees, assembled from the build-side
//! store.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use laft_core::{Dep, Name, PkgRef, Provide, Soname, StoreId, Timestamp, Variant};
use laft_index::{Defaults, Index, IndexEntry};
use laft_pkg::{Arch, Meta, StaticDep};
use laft_resolve::{Installed, Repository, Request, ResolveInput, Universe};
use laft_store::{PackageInput, Store};

use crate::error::BuildError;
use crate::{Item, KeyInputs, LinkInput, Recipe, StaticInput, ViewPackage};

/// The packages every build view holds: the POSIX shell that runs the
/// phases as `/bin/sh`, the core utilities, the
/// compression tools, `make`, the compiler, the binary tools, the C
/// library with its headers, the kernel headers and `pkgconf`.
pub const BUILD_BASE: &[&str] = &[
    "binutils",
    "bzip2",
    "coreutils",
    "dash",
    "diffutils",
    "file",
    "findutils",
    "gawk",
    "gcc",
    "glibc",
    "glibc-dev",
    "grep",
    "gzip",
    "linux-headers",
    "make",
    "patch",
    "pkgconf",
    "sed",
    "tar",
    "xz",
    "zstd",
];

/// The package that brings the musl cross toolchain into the view of
/// every `static` build.
pub const STATIC_TOOLCHAIN: &str = "toolchain-musl";

/// The packages that a smoke test's view holds besides the package and its
/// runtime dependencies.
pub const SMOKE_BASE: &[&str] = &["coreutils", "dash"];

/// The package references the build view of `recipe` is resolved from:
///
/// 1. every package of [`BUILD_BASE`],
/// 2. the `build_deps`, always `dynamic`,
/// 3. the `link_deps`: in a `dynamic` build each package and its `-dev`
///    subpackage, when `available` has one, and in a `static` build their
///    `static` variant, which the recipe already selected,
/// 4. [`STATIC_TOOLCHAIN`] for a `static` build,
/// 5. the `check_deps` when the check phase runs.
///
/// The resolver adds the runtime dependencies of all of them.
pub fn requests(recipe: &Recipe, check: bool, available: &BTreeSet<Name>) -> Vec<PkgRef> {
    let dynamic = |r: &PkgRef| PkgRef::new(r.name().clone(), r.prefix().cloned(), Variant::Dynamic);
    let named = |n: &str| {
        n.parse::<Name>()
            .ok()
            .map(|n| PkgRef::new(n, None, Variant::Dynamic))
    };
    let mut refs: Vec<PkgRef> = BUILD_BASE.iter().filter_map(|n| named(n)).collect();
    refs.extend(recipe.build_deps.iter().map(dynamic));
    for link in &recipe.link_deps {
        refs.push(link.clone());
        if recipe.variant == Variant::Dynamic
            && let Ok(dev) = format!("{}-dev", link.name()).parse::<Name>()
            && available.contains(&dev)
        {
            refs.push(PkgRef::new(dev, link.prefix().cloned(), Variant::Dynamic));
        }
    }
    if recipe.variant == Variant::Static {
        refs.extend(named(STATIC_TOOLCHAIN));
    }
    if check {
        refs.extend(recipe.check_deps.iter().map(dynamic));
    }
    let mut seen = BTreeSet::new();
    refs.retain(|r| seen.insert(r.to_string()));
    refs
}

/// A package of the build-side store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreEntry {
    /// The entry's directory name.
    pub dir: String,
    /// The store id.
    pub id: StoreId,
    /// The metadata.
    pub meta: Meta,
}

/// Every package of `store`.
///
/// # Errors
///
/// Fails when the store or an entry cannot be read.
pub fn store_entries(store: &Store) -> Result<Vec<StoreEntry>, BuildError> {
    let mut entries = Vec::new();
    for dir in store.list()? {
        let manifest = store.manifest(&dir)?;
        let meta = store.meta(&dir)?;
        entries.push(StoreEntry {
            dir,
            id: manifest.store_id(),
            meta,
        });
    }
    Ok(entries)
}

/// The packages of `entries`, which [`store_entries`] read, that builds
/// may use: every package that a build made, and every seed, a package
/// whose store id `seeds` holds, whose origin and variant no such package
/// has. A seed stands in for the packages of its origin only until a build
/// made some.
pub fn usable(mut entries: Vec<StoreEntry>, seeds: &BTreeSet<StoreId>) -> Vec<StoreEntry> {
    let built: BTreeSet<(Name, Variant)> = entries
        .iter()
        .filter(|e| !seeds.contains(&e.id))
        .map(|e| (e.meta.origin.clone(), e.meta.variant))
        .collect();
    entries.retain(|e| {
        !seeds.contains(&e.id) || !built.contains(&(e.meta.origin.clone(), e.meta.variant))
    });
    entries
}

/// A build view, assembled by [`assemble`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildView {
    /// The `usr` tree.
    pub usr: PathBuf,
    /// Every package of the view, sorted by name.
    pub packages: Vec<ViewPackage>,
    /// Every soname the packages provide.
    pub sonames: BTreeSet<Soname>,
    /// The `static` packages that came in through the link dependencies of
    /// a `static` build, directly or as their dependencies.
    pub statics: Vec<StaticDep>,
}

/// Assembles the build view of `recipe` as `tree/usr` from the [`usable`]
/// packages of `store` with the seeds `seeds`: [`requests`] and their
/// runtime dependency closure, resolved like any view, with the
/// alternatives the resolution chooses. With `pinned`, only the packages
/// with those store ids are considered, seeds or not, so that a rebuild
/// sees the view of an earlier build even after the store gained newer
/// packages.
///
/// # Errors
///
/// Fails with `B050` when the requests cannot be resolved against the
/// store or a pinned package is no longer in it, and when the tree cannot
/// be assembled.
pub fn assemble(
    store: &Store,
    seeds: &BTreeSet<StoreId>,
    recipe: &Recipe,
    check: bool,
    tree: &Path,
    pinned: Option<&BTreeSet<StoreId>>,
) -> Result<BuildView, BuildError> {
    let mut entries = store_entries(store)?;
    if let Some(pinned) = pinned {
        entries.retain(|e| pinned.contains(&e.id));
        if let Some(gone) = pinned
            .iter()
            .find(|id| !entries.iter().any(|e| e.id == **id))
        {
            return Err(BuildError::problem(
                "B050",
                format!("package {gone} of the earlier build view is no longer in the store"),
            ));
        }
    } else {
        entries = usable(entries, seeds);
    }
    let names = entries.iter().map(|e| e.meta.name.clone()).collect();
    let refs = requests(recipe, check, &names);
    let origin = format!("build view of {}", Item::of(recipe));
    let chosen = resolve(&entries, &refs, &origin, None, store, Some(tree))?;
    let mut statics = Vec::new();
    if recipe.variant == Variant::Static {
        let by_name: BTreeMap<&Name, &StoreEntry> = chosen
            .iter()
            .filter(|e| e.meta.variant == Variant::Static)
            .map(|e| (&e.meta.name, *e))
            .collect();
        let mut todo: Vec<&Name> = recipe.link_deps.iter().map(PkgRef::name).collect();
        let mut seen = BTreeSet::new();
        while let Some(name) = todo.pop() {
            let Some(entry) = by_name.get(name) else {
                continue;
            };
            if !seen.insert(name) {
                continue;
            }
            statics.push(StaticDep {
                name: name.clone(),
                version: entry.meta.version.clone(),
                rev: entry.meta.rev,
                id: entry.id,
            });
            todo.extend(entry.meta.depends.iter().filter_map(|d| match d {
                Dep::Pkg(name, _) => Some(name),
                _ => None,
            }));
        }
        statics.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(BuildView {
        usr: tree.join("usr"),
        packages: chosen.iter().map(|e| view_package(e)).collect(),
        sonames: chosen
            .iter()
            .flat_map(|e| &e.meta.provides)
            .filter_map(|p| match p {
                Provide::So(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        statics,
    })
}

/// Assembles the view of a smoke test as `tree/usr` from the [`usable`]
/// packages of `store` with the seeds `seeds`: the package `main`, which
/// must be in `store`, its runtime dependency closure and [`SMOKE_BASE`].
/// Returns the `usr` tree.
///
/// # Errors
///
/// Fails with `B050` when the closure cannot be resolved, and when the tree
/// cannot be assembled.
pub fn assemble_smoke(
    store: &Store,
    seeds: &BTreeSet<StoreId>,
    main: &StoreEntry,
    tree: &Path,
) -> Result<PathBuf, BuildError> {
    let entries = usable(store_entries(store)?, seeds);
    let mut refs = vec![PkgRef::new(main.meta.name.clone(), None, main.meta.variant)];
    refs.extend(
        SMOKE_BASE
            .iter()
            .filter_map(|n| n.parse().ok())
            .map(|n| PkgRef::new(n, None, Variant::Dynamic)),
    );
    let origin = format!("smoke test of {}", main.meta.name);
    resolve(&entries, &refs, &origin, Some(main), store, Some(tree))?;
    Ok(tree.join("usr"))
}

/// Assembles a view of `refs` and their runtime dependency closure as
/// `tree/usr` from the [`usable`] packages of `store` with the seeds
/// `seeds`, such as the view a checkpoint of a stage runs in, and returns
/// its packages, sorted by name. `origin` names the view in errors.
///
/// # Errors
///
/// Fails with `B050` when the store cannot satisfy `refs`, and when the
/// tree cannot be assembled.
pub fn assemble_view(
    store: &Store,
    seeds: &BTreeSet<StoreId>,
    refs: &[PkgRef],
    origin: &str,
    tree: &Path,
) -> Result<Vec<ViewPackage>, BuildError> {
    let entries = usable(store_entries(store)?, seeds);
    let chosen = resolve(&entries, refs, origin, None, store, Some(tree))?;
    Ok(chosen.iter().map(|e| view_package(e)).collect())
}

/// The packages that [`assemble_view`] would assemble for `refs`, sorted
/// by name, without assembling them.
///
/// # Errors
///
/// Fails with `B050` when the store cannot satisfy `refs`, and when the
/// store cannot be read.
pub fn view_packages(
    store: &Store,
    seeds: &BTreeSet<StoreId>,
    refs: &[PkgRef],
    origin: &str,
) -> Result<Vec<ViewPackage>, BuildError> {
    let entries = usable(store_entries(store)?, seeds);
    let chosen = resolve(&entries, refs, origin, None, store, None)?;
    Ok(chosen.iter().map(|e| view_package(e)).collect())
}

/// The [`ViewPackage`] that `entry` is in a view.
fn view_package(entry: &StoreEntry) -> ViewPackage {
    ViewPackage {
        name: entry.meta.name.clone(),
        version: entry.meta.version.clone(),
        rev: entry.meta.rev,
        variant: entry.meta.variant,
        id: entry.id,
    }
}

/// Resolves `refs` against `entries`, preferring the installed `pinned`
/// package, assembles the result into `tree` when one is given and returns
/// the chosen entries, sorted by name.
fn resolve<'e>(
    entries: &'e [StoreEntry],
    refs: &[PkgRef],
    origin: &str,
    pinned: Option<&StoreEntry>,
    store: &Store,
    tree: Option<&Path>,
) -> Result<Vec<&'e StoreEntry>, BuildError> {
    let mut packages: Vec<IndexEntry> = entries
        .iter()
        .map(|e| IndexEntry {
            meta: e.meta.clone(),
            id: e.id,
            file: format!("pool/{}.lpkg", e.id),
            size: 0,
            blake3: laft_core::Blake3Hash::of(b""),
        })
        .collect();
    packages.sort_by(|a, b| {
        (&a.meta.name, &b.meta.version, b.meta.rev, a.meta.variant).cmp(&(
            &b.meta.name,
            &a.meta.version,
            a.meta.rev,
            b.meta.variant,
        ))
    });
    let index = Index {
        format: Index::FORMAT,
        repo: name("build"),
        channel: name("local"),
        arch: Arch::X86_64V3,
        generation: 1,
        timestamp: Timestamp::from_secs(0),
        expires: Timestamp::from_secs(1),
        defaults: Defaults::default(),
        packages,
        yanked: Vec::new(),
    };
    let universe = Universe::new(&[Repository {
        index: &index,
        priority: 0,
    }]);
    let mut input = ResolveInput::new("build");
    input.requests = refs
        .iter()
        .map(|r| Request {
            pkg: r.clone(),
            origin: origin.to_owned(),
        })
        .collect();
    if let Some(pinned) = pinned {
        input.installed.insert(
            pinned.meta.name.clone(),
            Installed {
                id: pinned.id,
                meta: &pinned.meta,
            },
        );
    }
    let resolution = universe
        .resolve(&input)
        .map_err(|e| BuildError::problem("B050", format!("{origin} cannot be resolved: {e}")))?;
    let ids: BTreeSet<StoreId> = resolution
        .packages
        .values()
        .map(|s| s.candidate.id)
        .collect();
    let mut chosen: Vec<&StoreEntry> = entries.iter().filter(|e| ids.contains(&e.id)).collect();
    if let Some(tree) = tree {
        let inputs: Vec<PackageInput> = chosen
            .iter()
            .map(|e| PackageInput {
                dir: e.dir.clone(),
                reasons: Vec::new(),
            })
            .collect();
        std::fs::create_dir_all(tree).map_err(crate::error::io(tree))?;
        store.assemble(&inputs, &resolution.alternatives, tree)?;
    }
    chosen.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    Ok(chosen)
}

fn name(text: &str) -> Name {
    text.parse().expect("the literal is a valid name")
}

/// The build key inputs of `recipe` against the [`usable`] packages of
/// `store` with the seeds `seeds`: each link dependency's newest package of
/// the right variant gives its `abi_epoch` and sonames, and in a `static`
/// build its store id. A link dependency the store does not have yet
/// contributes nothing, so its first build changes the key of its
/// dependents.
///
/// # Errors
///
/// Fails when the store cannot be read.
pub fn key_inputs(
    recipe: &Recipe,
    store: &Store,
    seeds: &BTreeSet<StoreId>,
) -> Result<KeyInputs, BuildError> {
    Ok(key_inputs_from(
        recipe,
        &usable(store_entries(store)?, seeds),
    ))
}

/// [`key_inputs`] against `entries`, the [`usable`] packages of the store,
/// so that planning many recipes reads the store once.
pub fn key_inputs_from(recipe: &Recipe, entries: &[StoreEntry]) -> KeyInputs {
    let mut links = Vec::new();
    let mut statics = Vec::new();
    for link in &recipe.link_deps {
        let newest = entries
            .iter()
            .filter(|e| {
                e.meta.name == *link.name()
                    && e.meta.variant == link.variant()
                    && link.prefix().is_none_or(|p| p.matches(&e.meta.version))
            })
            .max_by(|a, b| (&a.meta.version, a.meta.rev).cmp(&(&b.meta.version, b.meta.rev)));
        let Some(entry) = newest else {
            continue;
        };
        links.push(LinkInput {
            name: entry.meta.name.clone(),
            abi_epoch: entry.meta.abi_epoch,
            sonames: entry
                .meta
                .provides
                .iter()
                .filter_map(|p| match p {
                    Provide::So(s) => Some(s.to_string()),
                    _ => None,
                })
                .collect(),
        });
        if entry.meta.variant == Variant::Static {
            statics.push(StaticInput {
                name: entry.meta.name.clone(),
                id: entry.id,
            });
        }
    }
    KeyInputs::new(recipe, links, statics)
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
