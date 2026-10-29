//! The dependencies between the recipes of one build run, and the order
//! they are built in.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use laft_core::{Name, PkgRef, Variant};
use serde::{Deserialize, Serialize};

use crate::{Problem, Recipe};

/// One package and variant: what one build produces, and one node of the
/// build graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Item {
    /// The package name.
    pub name: Name,
    /// The variant.
    pub variant: Variant,
}

impl Item {
    /// The item for `name` and `variant`.
    pub fn new(name: Name, variant: Variant) -> Self {
        Self { name, variant }
    }

    /// The item a recipe builds.
    pub fn of(recipe: &Recipe) -> Self {
        Self::new(recipe.name.clone(), recipe.variant)
    }
}

/// Written like a package reference: `zlib` or `zlib:static`.
impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.variant {
            Variant::Dynamic => write!(f, "{}", self.name),
            variant => write!(f, "{}:{variant}", self.name),
        }
    }
}

/// The items of the same run that one item's build needs, by kind of
/// dependency. A dependency that no recipe of the run builds is satisfied
/// by a published package and is not listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deps {
    /// Items whose packages run during the build.
    pub build: BTreeSet<Item>,
    /// Items whose packages the build links against.
    pub link: BTreeSet<Item>,
    /// Items whose packages the check phase needs.
    pub check: BTreeSet<Item>,
}

impl Deps {
    /// Every dependency, of any kind.
    pub fn all(&self) -> impl Iterator<Item = &Item> {
        let all: BTreeSet<&Item> = self
            .build
            .iter()
            .chain(&self.link)
            .chain(&self.check)
            .collect();
        all.into_iter()
    }
}

/// The recipes of one build run and the dependencies between them.
///
/// A package reference resolves to a recipe of the run when it names the
/// package or one of its subpackages, explicit or automatic, and its
/// version prefix, if any, matches the recipe's version. The variant of
/// the reference selects the item. Any other reference must be satisfied
/// by a published package.
#[derive(Debug, Clone)]
pub struct BuildGraph {
    recipes: BTreeMap<Item, Recipe>,
    deps: BTreeMap<Item, Deps>,
}

/// What a package reference resolves to.
enum Resolved {
    Item(Item),
    /// A recipe of the run provides the name, but not in this variant.
    NoVariant(Name),
    External,
}

impl BuildGraph {
    /// Builds the graph of `recipes`. `published` says whether a package
    /// reference that no recipe provides is satisfied by a published
    /// package.
    ///
    /// # Errors
    ///
    /// Returns every problem found: `R020` for a reference to a package of
    /// the run in a variant it does not build, such as a `static` build
    /// linking a library without a `static` variant, and `R021` for a
    /// reference that neither a recipe nor a published package satisfies,
    /// and `R022` for two recipes of one package and variant.
    pub fn new(
        recipes: impl IntoIterator<Item = Recipe>,
        published: &dyn Fn(&PkgRef) -> bool,
    ) -> Result<Self, Vec<Problem>> {
        let mut problems = Vec::new();
        let mut unique = BTreeMap::new();
        for recipe in recipes {
            match unique.entry(Item::of(&recipe)) {
                Entry::Occupied(taken) => problems.push(Problem::new(
                    "R022",
                    format!("the run holds two recipes for {}", taken.key()),
                )),
                Entry::Vacant(free) => {
                    free.insert(recipe);
                }
            }
        }
        let recipes = unique;
        let mut mains: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
        let mut subs: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
        for (item, recipe) in &recipes {
            mains.entry(recipe.name.as_str()).or_default().push(item);
            let explicit = recipe.outputs.subpackages.keys().map(String::as_str);
            let automatic = recipe.outputs.split.iter().map(|s| s.as_str());
            for suffix in explicit.chain(automatic) {
                subs.entry(format!("{}-{suffix}", recipe.name))
                    .or_default()
                    .push(item);
            }
        }
        let resolve = |r: &PkgRef| {
            let name = r.name().as_str();
            let candidates = mains.get(name).or_else(|| subs.get(name));
            let matching: Vec<&&Item> = candidates
                .into_iter()
                .flatten()
                .filter(|item| {
                    r.prefix()
                        .is_none_or(|p| p.matches(&recipes[**item].version))
                })
                .collect();
            match matching.first() {
                None => Resolved::External,
                Some(first) => match matching.iter().find(|i| i.variant == r.variant()) {
                    Some(item) => Resolved::Item((**item).clone()),
                    None => Resolved::NoVariant(first.name.clone()),
                },
            }
        };

        let mut deps = BTreeMap::new();
        for (item, recipe) in &recipes {
            let mut resolve_all = |refs: &[PkgRef], verb: &str| {
                let mut edges = BTreeSet::new();
                for r in refs {
                    match resolve(r) {
                        Resolved::Item(dep) => {
                            edges.insert(dep);
                        }
                        Resolved::NoVariant(name) => problems.push(Problem::new(
                            "R020",
                            format!(
                                "{item} {verb} {name}, which has no {} variant",
                                r.variant()
                            ),
                        )),
                        Resolved::External if published(r) => {}
                        Resolved::External => problems.push(Problem::new(
                            "R021",
                            format!(
                                "{item} {verb} {r}, which no recipe builds and no repository publishes"
                            ),
                        )),
                    }
                }
                edges
            };
            let out = Deps {
                build: resolve_all(&recipe.build_deps, "builds with"),
                link: resolve_all(&recipe.link_deps, "links against"),
                check: resolve_all(&recipe.check_deps, "checks with"),
            };
            // Runtime dependencies are checked but do not order builds.
            resolve_all(&recipe.outputs.deps, "depends on");
            deps.insert(item.clone(), out);
        }
        if problems.is_empty() {
            Ok(Self { recipes, deps })
        } else {
            Err(problems)
        }
    }

    /// Every item, in order.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.recipes.keys()
    }

    /// The recipe of `item`.
    pub fn recipe(&self, item: &Item) -> Option<&Recipe> {
        self.recipes.get(item)
    }

    /// The dependencies of `item` within the run.
    pub fn deps(&self, item: &Item) -> Option<&Deps> {
        self.deps.get(item)
    }

    /// The `static` items whose packages end up inside the outputs of the
    /// `static` item `item`: its link dependencies and theirs, transitively.
    /// They are what a change of a static library must rebuild.
    pub fn static_inputs(&self, item: &Item) -> BTreeSet<Item> {
        let mut seen = BTreeSet::new();
        let mut todo = vec![item];
        while let Some(next) = todo.pop() {
            for dep in self.deps.get(next).into_iter().flat_map(|d| &d.link) {
                if dep.variant == Variant::Static && seen.insert(dep.clone()) {
                    todo.push(dep);
                }
            }
        }
        seen.remove(item);
        seen
    }

    /// Returns every item in an order in which each comes after the items
    /// it depends on, so that a build can use what the run built before.
    ///
    /// Items in `use_published` do not hold back their dependents, which
    /// use the published packages instead. A dependency of an item on
    /// itself is always satisfied by a published package. Items that
    /// depend on each other in a cycle are fine as long as some item of
    /// every cycle is not `dirty`, because a clean item is not rebuilt.
    ///
    /// # Errors
    ///
    /// Returns `B010` with the cycle when items of `dirty` depend on each
    /// other in a cycle, for example `gcc -> glibc -> gcc`.
    pub fn order(
        &self,
        dirty: &BTreeSet<Item>,
        use_published: &BTreeSet<Item>,
    ) -> Result<Vec<Item>, Problem> {
        let edges: BTreeMap<&Item, BTreeSet<&Item>> = self
            .deps
            .iter()
            .map(|(item, deps)| {
                let deps = deps
                    .all()
                    .filter(|d| *d != item && !use_published.contains(*d))
                    .collect();
                (item, deps)
            })
            .collect();

        let dirty_edges: BTreeMap<&Item, BTreeSet<&Item>> = edges
            .iter()
            .filter(|(item, _)| dirty.contains(**item))
            .map(|(item, deps)| {
                let deps = deps
                    .iter()
                    .copied()
                    .filter(|d| dirty.contains(*d))
                    .collect();
                (*item, deps)
            })
            .collect();
        if let Some(cycle) = find_cycle(&dirty_edges) {
            let path: Vec<String> = cycle.iter().map(ToString::to_string).collect();
            return Err(Problem::new(
                "B010",
                format!("build cycle among dirty packages: {}", path.join(" -> ")),
            ));
        }

        let components = components(&edges);
        let mut component_of: BTreeMap<&Item, usize> = BTreeMap::new();
        for (i, c) in components.iter().enumerate() {
            for item in c {
                component_of.insert(*item, i);
            }
        }
        let mut waiting: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); components.len()];
        for (item, deps) in &edges {
            for dep in deps {
                let (from, to) = (component_of[item], component_of[dep]);
                if from != to {
                    waiting[from].insert(to);
                }
            }
        }
        // Components are released smallest first, so the order does not
        // depend on anything but the graph.
        let mut ready: BTreeSet<(&Item, usize)> = BTreeSet::new();
        for (i, c) in components.iter().enumerate() {
            if waiting[i].is_empty() {
                ready.insert((c[0], i));
            }
        }
        let mut order = Vec::new();
        while let Some(first) = ready.pop_first() {
            let (_, done) = first;
            order.extend(within(&components[done], &dirty_edges));
            for (i, wait) in waiting.iter_mut().enumerate() {
                if wait.remove(&done) && wait.is_empty() {
                    ready.insert((components[i][0], i));
                }
            }
        }
        Ok(order.into_iter().cloned().collect())
    }
}

/// Orders the members of one strongly connected component: the dirty
/// members after the dirty members they depend on, which never form a
/// cycle here, and otherwise by name.
fn within<'a>(
    members: &[&'a Item],
    dirty_edges: &BTreeMap<&'a Item, BTreeSet<&'a Item>>,
) -> Vec<&'a Item> {
    let set: BTreeSet<&Item> = members.iter().copied().collect();
    let mut placed: BTreeSet<&Item> = BTreeSet::new();
    let mut order = Vec::new();
    while order.len() < members.len() {
        let next = members.iter().copied().find(|m| {
            !placed.contains(*m)
                && dirty_edges.get(*m).is_none_or(|deps| {
                    deps.iter()
                        .all(|d| !set.contains(*d) || placed.contains(*d))
                })
        });
        let Some(next) = next else {
            // Unreachable when the dirty members have no cycle, which the
            // caller checked. Placing the rest keeps the order complete.
            order.extend(members.iter().copied().filter(|m| !placed.contains(*m)));
            break;
        };
        placed.insert(next);
        order.push(next);
    }
    order
}

/// Returns a cycle of `edges`, starting and ending with the same item, or
/// `None` when there is none.
fn find_cycle<'a>(edges: &BTreeMap<&'a Item, BTreeSet<&'a Item>>) -> Option<Vec<&'a Item>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Active,
        Done,
    }
    let mut marks: BTreeMap<&Item, Mark> = BTreeMap::new();
    for start in edges.keys() {
        if marks.contains_key(start) {
            continue;
        }
        // An explicit stack of (item, its remaining dependencies) keeps
        // deep graphs off the call stack.
        let mut path: Vec<&Item> = vec![start];
        let mut stack: Vec<Vec<&Item>> = vec![deps_of(edges, start)];
        marks.insert(start, Mark::Active);
        while let Some(todo) = stack.last_mut() {
            match todo.pop() {
                Some(next) => match marks.get(next) {
                    Some(Mark::Active) => {
                        let from = path.iter().position(|i| *i == next).unwrap_or(0);
                        let mut cycle = path[from..].to_vec();
                        cycle.push(next);
                        return Some(cycle);
                    }
                    Some(Mark::Done) => {}
                    None => {
                        marks.insert(next, Mark::Active);
                        path.push(next);
                        stack.push(deps_of(edges, next));
                    }
                },
                None => {
                    stack.pop();
                    if let Some(done) = path.pop() {
                        marks.insert(done, Mark::Done);
                    }
                }
            }
        }
    }
    None
}

/// The dependencies of `item`, largest first, so that popping visits them
/// in order.
fn deps_of<'a>(edges: &BTreeMap<&'a Item, BTreeSet<&'a Item>>, item: &Item) -> Vec<&'a Item> {
    edges
        .get(item)
        .map(|d| d.iter().rev().copied().collect())
        .unwrap_or_default()
}

/// Returns the strongly connected components of `edges` with Tarjan's
/// algorithm, each sorted.
fn components<'a>(edges: &BTreeMap<&'a Item, BTreeSet<&'a Item>>) -> Vec<Vec<&'a Item>> {
    struct State<'a> {
        index: BTreeMap<&'a Item, usize>,
        low: BTreeMap<&'a Item, usize>,
        on_stack: BTreeSet<&'a Item>,
        stack: Vec<&'a Item>,
        out: Vec<Vec<&'a Item>>,
    }
    let mut s = State {
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        on_stack: BTreeSet::new(),
        stack: Vec::new(),
        out: Vec::new(),
    };
    for root in edges.keys() {
        if s.index.contains_key(root) {
            continue;
        }
        // Each frame is an item and the dependencies it has yet to visit.
        let mut frames: Vec<(&Item, Vec<&Item>)> = Vec::new();
        let enter = |s: &mut State<'a>, item: &'a Item| {
            let n = s.index.len();
            s.index.insert(item, n);
            s.low.insert(item, n);
            s.stack.push(item);
            s.on_stack.insert(item);
        };
        enter(&mut s, root);
        frames.push((root, deps_of(edges, root)));
        while let Some((item, todo)) = frames.last_mut() {
            let item = *item;
            if let Some(next) = todo.pop() {
                if !s.index.contains_key(next) {
                    enter(&mut s, next);
                    frames.push((next, deps_of(edges, next)));
                } else if s.on_stack.contains(next) {
                    let low = s.low[item].min(s.index[next]);
                    s.low.insert(item, low);
                }
                continue;
            }
            frames.pop();
            if let Some((parent, _)) = frames.last() {
                let low = s.low[parent].min(s.low[item]);
                s.low.insert(parent, low);
            }
            if s.low[item] == s.index[item] {
                let mut component = Vec::new();
                while let Some(member) = s.stack.pop() {
                    s.on_stack.remove(member);
                    component.push(member);
                    if member == item {
                        break;
                    }
                }
                component.sort();
                s.out.push(component);
            }
        }
    }
    s.out
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
