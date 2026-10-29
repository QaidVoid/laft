//! Deciding what to rebuild: build keys, rebuild triggers, build records
//! and the schedule that stops propagating unchanged outputs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use laft_core::{Blake3Hash, Name, Sha256Hash, StoreId, Variant};
use serde::{Deserialize, Serialize};

use crate::{BuildGraph, Item, Problem, Recipe};

/// Why an item needs a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    /// `T1`: the plan hash differs from the latest build. Recipe edits,
    /// schema changes and style template changes do this.
    Plan,
    /// `T2`: the hash of a source, a vendor tarball, a patch or another
    /// file the recipe names changed.
    Source,
    /// `T3`: a link dependency no longer provides a soname or a symbol
    /// that the latest build imports from it.
    Import,
    /// `T4`: a `static` package inside a `static` build has a new store id.
    Static,
    /// `T5`: the `abi_epoch` of a link dependency changed.
    Epoch,
    /// `T6`: the toolchain epoch of the recipe repositories changed.
    Toolchain,
    /// `T7`: there is no previous successful build.
    New,
    /// `T8`: a rebuild was requested.
    Forced,
}

impl Trigger {
    /// The code, `T1` to `T8`.
    pub fn code(self) -> &'static str {
        match self {
            Self::Plan => "T1",
            Self::Source => "T2",
            Self::Import => "T3",
            Self::Static => "T4",
            Self::Epoch => "T5",
            Self::Toolchain => "T6",
            Self::New => "T7",
            Self::Forced => "T8",
        }
    }

    /// The name, for example `plan`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Source => "source",
            Self::Import => "import",
            Self::Static => "static",
            Self::Epoch => "epoch",
            Self::Toolchain => "toolchain",
            Self::New => "new",
            Self::Forced => "forced",
        }
    }
}

/// Written as the code and the name, for example `T1 plan`.
impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code(), self.name())
    }
}

/// A link dependency as a build sees it in its build view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkInput {
    /// The package name.
    pub name: Name,
    /// Its `abi_epoch`, from its buildinfo or, when this run builds it,
    /// from its recipe.
    pub abi_epoch: u32,
    /// The sonames it provides.
    pub sonames: BTreeSet<String>,
}

/// A `static` package in the build view of a `static` build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticInput {
    /// The package name.
    pub name: Name,
    /// Its store id.
    pub id: StoreId,
}

/// Everything the build key of one build covers.
///
/// It deliberately leaves out the versions of the build tools and the
/// store ids of dynamic link dependencies, so that a new compiler release
/// or a library update does not rebuild everything. Those changes rebuild
/// dependents only through [`Trigger::Import`] and [`Trigger::Epoch`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyInputs {
    /// The plan hash of the recipe.
    pub plan: Blake3Hash,
    /// The hash of every source that declares one and of every vendor
    /// tarball, in plan order.
    pub sources: Vec<Sha256Hash>,
    /// The hash of every patch, in order, then of every other file the
    /// recipe names.
    pub patches: Vec<Blake3Hash>,
    /// The link dependencies in the build view, sorted by name.
    pub links: Vec<LinkInput>,
    /// For a `static` build, every `static` package in the build view,
    /// sorted by name. Empty for a `dynamic` build.
    #[serde(rename = "static")]
    pub statics: Vec<StaticInput>,
    /// The toolchain epoch of the recipe repositories.
    pub toolchain_epoch: u32,
}

impl KeyInputs {
    /// The inputs of a build of `recipe` whose build view holds the link
    /// dependencies `links` and the `static` packages `statics`. `statics`
    /// is ignored for a `dynamic` build.
    pub fn new(recipe: &Recipe, mut links: Vec<LinkInput>, mut statics: Vec<StaticInput>) -> Self {
        links.sort_by(|a, b| a.name.cmp(&b.name));
        statics.sort_by(|a, b| a.name.cmp(&b.name));
        if recipe.variant == Variant::Dynamic {
            statics.clear();
        }
        Self {
            plan: recipe.plan_hash(),
            sources: recipe.source_hashes(),
            patches: recipe.file_hashes(),
            links,
            statics,
            toolchain_epoch: recipe.toolchain_epoch,
        }
    }

    /// The build key: BLAKE3 of the canonical JSON of the inputs, exactly
    /// as they serialize, so a stored [`BuildRecord`] can re-derive it. Two
    /// builds with the same key are expected to produce the same outputs.
    ///
    /// # Panics
    ///
    /// Never in practice: the inputs always serialize to canonical JSON.
    pub fn build_key(&self) -> Blake3Hash {
        let json = laft_core::to_canonical_json(self)
            .expect("build key inputs hold no number that canonical JSON refuses");
        Blake3Hash::of(&json)
    }

    /// The triggers that a build with these inputs has, given the latest
    /// successful build `previous` and whether a rebuild was `forced`.
    /// Without `forced`, the set is empty exactly when the build key is
    /// unchanged.
    ///
    /// A link dependency whose set of sonames changed is
    /// [`Trigger::Import`], the same as one that appeared or went away.
    /// The finer check of what a build imports needs the new outputs and
    /// happens in [`Schedule::finish`].
    pub fn triggers(&self, previous: Option<&BuildRecord>, forced: bool) -> BTreeSet<Trigger> {
        let mut out = BTreeSet::new();
        if forced {
            out.insert(Trigger::Forced);
        }
        let Some(previous) = previous else {
            out.insert(Trigger::New);
            return out;
        };
        let old = &previous.inputs;
        if self.plan != old.plan {
            out.insert(Trigger::Plan);
        }
        if self.sources != old.sources || self.patches != old.patches {
            out.insert(Trigger::Source);
        }
        if self.statics != old.statics {
            out.insert(Trigger::Static);
        }
        if self.toolchain_epoch != old.toolchain_epoch {
            out.insert(Trigger::Toolchain);
        }
        let old_links: BTreeMap<&Name, &LinkInput> =
            old.links.iter().map(|l| (&l.name, l)).collect();
        let new_links: BTreeMap<&Name, &LinkInput> =
            self.links.iter().map(|l| (&l.name, l)).collect();
        if old_links.keys().ne(new_links.keys()) {
            out.insert(Trigger::Import);
        }
        for (name, link) in &new_links {
            let Some(before) = old_links.get(name) else {
                continue;
            };
            if link.abi_epoch != before.abi_epoch {
                out.insert(Trigger::Epoch);
            }
            if link.sonames != before.sonames {
                out.insert(Trigger::Import);
            }
        }
        out
    }
}

/// The record of a successful build, kept to plan the next ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRecord {
    /// What was built.
    pub item: Item,
    /// The build key.
    pub key: Blake3Hash,
    /// What the key covered.
    pub inputs: KeyInputs,
    /// The store id of every output package, by package name.
    pub outputs: BTreeMap<Name, StoreId>,
    /// What the outputs import: for every needed soname, the symbols taken
    /// from it, written `name@VERSION` or `name`.
    pub imports: BTreeMap<String, BTreeSet<String>>,
}

/// What a finished build produced, as far as its dependents care.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Built {
    /// The store id of every output package, by package name.
    pub outputs: BTreeMap<Name, StoreId>,
    /// What the outputs export: for every soname they provide, the defined
    /// dynamic symbols, written like [`BuildRecord::imports`].
    pub exports: BTreeMap<String, BTreeSet<String>>,
}

/// What finishing a build means for the rest of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cutoff {
    /// The outputs have the same store ids as the latest build, so nothing
    /// that depends on them is touched.
    Unchanged,
    /// The outputs changed. These dependents now need a build.
    Changed(Vec<Dirtied>),
}

/// A dependent that a changed build made dirty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirtied {
    /// The dependent.
    pub item: Item,
    /// Why, [`Trigger::Import`] or [`Trigger::Static`].
    pub trigger: Trigger,
    /// What changed, for example `libfoo.so.2 is no longer provided (now
    /// libfoo.so.3)`.
    pub reason: String,
    /// Whether the dependent came earlier in the order, which only happens
    /// in a dependency cycle. It needs another run.
    pub late: bool,
}

/// The items of one run in build order, which of them need a build and
/// why, and how finished builds change that.
///
/// The schedule starts with the items whose build key changed. Each
/// finished build is compared with its latest record: unchanged outputs
/// stop there, and changed ones make dependents dirty when their recorded
/// imports are no longer satisfied or a static library inside them
/// changed. Those dependents always come later in the order.
#[derive(Debug, Clone)]
pub struct Schedule {
    order: Vec<Item>,
    position: BTreeMap<Item, usize>,
    next: usize,
    dirty: BTreeMap<Item, BTreeSet<Trigger>>,
    previous: BTreeMap<Item, BuildRecord>,
    link_dependents: BTreeMap<Item, BTreeSet<Item>>,
    static_dependents: BTreeMap<Item, BTreeSet<Item>>,
}

impl Schedule {
    /// The schedule of the items of `graph`. `dirty` holds the items that
    /// need a build with their triggers, and `previous` the latest
    /// successful build of each item that has one. Dependents of items in
    /// `use_published` do not wait for them.
    ///
    /// # Errors
    ///
    /// Returns `B010` when dirty items depend on each other in a cycle.
    pub fn new(
        graph: &BuildGraph,
        dirty: BTreeMap<Item, BTreeSet<Trigger>>,
        previous: BTreeMap<Item, BuildRecord>,
        use_published: &BTreeSet<Item>,
    ) -> Result<Self, Problem> {
        let dirty: BTreeMap<Item, BTreeSet<Trigger>> =
            dirty.into_iter().filter(|(_, t)| !t.is_empty()).collect();
        let order = graph.order(&dirty.keys().cloned().collect(), use_published)?;
        let mut link_dependents: BTreeMap<Item, BTreeSet<Item>> = BTreeMap::new();
        let mut static_dependents: BTreeMap<Item, BTreeSet<Item>> = BTreeMap::new();
        for item in graph.items() {
            for dep in graph.deps(item).into_iter().flat_map(|d| &d.link) {
                if item.variant == Variant::Dynamic && dep != item {
                    link_dependents
                        .entry(dep.clone())
                        .or_default()
                        .insert(item.clone());
                }
            }
            if item.variant == Variant::Static {
                for input in graph.static_inputs(item) {
                    static_dependents
                        .entry(input)
                        .or_default()
                        .insert(item.clone());
                }
            }
        }
        Ok(Self {
            position: order
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, item)| (item, i))
                .collect(),
            order,
            next: 0,
            dirty,
            previous,
            link_dependents,
            static_dependents,
        })
    }

    /// Every item of the run, in build order.
    pub fn order(&self) -> &[Item] {
        &self.order
    }

    /// The items that currently need a build, in build order, with their
    /// triggers.
    pub fn planned(&self) -> Vec<(&Item, &BTreeSet<Trigger>)> {
        self.order
            .iter()
            .filter_map(|item| self.dirty.get(item).map(|t| (item, t)))
            .collect()
    }

    /// The triggers of `item`, or `None` when it is up to date.
    pub fn triggers(&self, item: &Item) -> Option<&BTreeSet<Trigger>> {
        self.dirty.get(item)
    }

    /// The next item to build, or `None` when the run is complete. Items
    /// that are up to date are skipped.
    pub fn next_build(&mut self) -> Option<Item> {
        while let Some(item) = self.order.get(self.next) {
            self.next += 1;
            if self.dirty.contains_key(item) {
                return Some(item.clone());
            }
        }
        None
    }

    /// Records that `item` was built with the result `built`, and marks
    /// the dependents that the change affects.
    pub fn finish(&mut self, item: &Item, built: &Built) -> Cutoff {
        let unchanged = self
            .previous
            .get(item)
            .is_some_and(|p| p.outputs == built.outputs);
        if unchanged {
            return Cutoff::Unchanged;
        }
        let mut dirtied = Vec::new();
        for dependent in self.static_dependents.get(item).into_iter().flatten() {
            dirtied.push((
                dependent.clone(),
                Trigger::Static,
                format!("{item} has a new store id"),
            ));
        }
        for dependent in self.link_dependents.get(item).into_iter().flatten() {
            if let Some(reason) = self
                .previous
                .get(dependent)
                .and_then(|p| unsatisfied(p, &item.name, built))
            {
                dirtied.push((dependent.clone(), Trigger::Import, reason));
            }
        }
        let current = self.next;
        let dirtied = dirtied
            .into_iter()
            .map(|(dependent, trigger, reason)| {
                self.dirty
                    .entry(dependent.clone())
                    .or_default()
                    .insert(trigger);
                let late = self.position.get(&dependent).is_none_or(|p| *p < current);
                Dirtied {
                    item: dependent,
                    trigger,
                    reason,
                    late,
                }
            })
            .collect();
        Cutoff::Changed(dirtied)
    }
}

/// Says why the build `previous` of a dependent of `library` no longer
/// works with the new outputs `built`, or returns `None` when every
/// soname it needs from `library` is still provided with every symbol it
/// imports.
fn unsatisfied(previous: &BuildRecord, library: &Name, built: &Built) -> Option<String> {
    let link = previous.inputs.links.iter().find(|l| l.name == *library)?;
    for soname in &link.sonames {
        let Some(imported) = previous.imports.get(soname) else {
            continue;
        };
        let Some(exported) = built.exports.get(soname) else {
            let now: Vec<&str> = built.exports.keys().map(String::as_str).collect();
            return Some(format!(
                "{soname} is no longer provided (now {})",
                if now.is_empty() {
                    "none".to_owned()
                } else {
                    now.join(", ")
                }
            ));
        };
        let missing: Vec<&str> = imported.difference(exported).map(String::as_str).collect();
        if !missing.is_empty() {
            return Some(format!("{soname} no longer exports {}", missing.join(", ")));
        }
    }
    None
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
