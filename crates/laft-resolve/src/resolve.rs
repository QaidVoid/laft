use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use laft_core::{
    Dep, Name, OptionConstraint, OptionConstraints, OptionValue, PkgRef, StoreId, Variant, Version,
    VersionPrefix, VersionReq,
};
use laft_index::Yank;
use laft_pkg::Meta;

use crate::error::{Bound, BoundSource, Demand, Error, ErrorKind, ValueOrigin};
use crate::explain::{Chain, Learned, Package, Reason, Warning, Why};
use crate::universe::{Candidate, Key, Source, Universe, option_order, order, provides};

/// The most passes one resolution may take. A resolution without conflicts
/// takes one pass, and each conflict usually costs one or two more.
const MAX_PASSES: usize = 1000;

/// A package reference requested for a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The reference, for example `llvm@19` or `ripgrep:static`.
    pub pkg: PkgRef,
    /// Where the request was made, for example `/etc/laft/system.stav:8:22`
    /// or `command line`.
    pub origin: String,
}

/// Which installed packages may move to a newer version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Upgrade {
    /// Keep every installed version that still satisfies everything.
    #[default]
    None,
    /// Select the best candidate of every name.
    All,
    /// Select the best candidate of these names and keep the others.
    Names(BTreeSet<Name>),
}

/// A package currently in the view being resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installed<'a> {
    /// Its store id.
    pub id: StoreId,
    /// Its metadata, read from the store entry.
    pub meta: &'a Meta,
}

/// The option values the configuration gives one package name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionRequest {
    /// Values set at normal or forced priority, with where each was set.
    /// They are hard constraints: every candidate of the name must record
    /// each fixed option, with exactly this value.
    pub fixed: BTreeMap<String, FixedValue>,
    /// Values set with `default(...)`, by a part or by a profile. They
    /// steer the choice among candidates, and nothing requires them.
    pub preferred: BTreeMap<String, OptionValue>,
}

/// A fixed option value and where the configuration set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedValue {
    /// The value.
    pub value: OptionValue,
    /// Where the configuration set it, for example
    /// `/etc/laft/system.stav:14:5`.
    pub origin: String,
}

/// A package a recipe snapshot can build.
///
/// Its dependencies are unknown until the recipe is evaluated with the
/// chosen values, so a buildable candidate satisfies only dependencies on
/// its own name, and joins no alternatives group. When the view selects
/// one and building is not allowed, the resolution fails with S011.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buildable {
    /// The package name.
    pub name: Name,
    /// The version the snapshot builds.
    pub version: Version,
    /// The revision.
    pub rev: u32,
    /// The variant the recipe builds.
    pub variant: Variant,
    /// The declared default of every option. The values of a build start
    /// from these.
    pub defaults: BTreeMap<String, OptionValue>,
}

/// A package the view needs from a recipe snapshot, with the values it
/// would be built with. Building it and publishing it in the local
/// repository lets the next resolution select the built package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    /// The package the snapshot builds.
    pub buildable: Buildable,
    /// The option values it would be built with: the declared defaults,
    /// with the fixed values of the view and every option constraint the
    /// selection met applied over them. A build takes the option context
    /// of the view it is for; how the dependencies of the build itself
    /// resolve is the `dep_options` choice of the build planning, not
    /// part of the request.
    pub values: BTreeMap<String, OptionValue>,
    /// Why the view needs it: its requests first, then the dependencies it
    /// satisfies, in the order they were met.
    pub reasons: Vec<Reason>,
}

/// What to resolve: the requests of one view and its current state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveInput<'a> {
    /// The view name, used in messages. The initrd closure, which belongs to
    /// no view, is resolved under the name `initrd`.
    pub view: String,
    /// The requests, after view inheritance.
    pub requests: Vec<Request>,
    /// The configured member of alternatives groups, by group.
    pub alternatives: BTreeMap<Name, Name>,
    /// The packages currently in the view, by name. An entry whose metadata
    /// names another package is ignored.
    pub installed: BTreeMap<Name, Installed<'a>>,
    /// Which installed packages may move to a newer version.
    pub upgrade: Upgrade,
    /// The fixed and preferred option values, by package name.
    pub options: BTreeMap<Name, OptionRequest>,
    /// The packages a recipe snapshot can build, one entry per name. When
    /// several entries name one package, the newest version wins.
    pub buildable: Vec<Buildable>,
    /// Whether the view may select a package that a recipe snapshot builds
    /// after the resolution. When false, such a selection fails with S011.
    pub allow_builds: bool,
}

impl ResolveInput<'_> {
    /// An input for `view` with no requests, alternatives or installed
    /// packages.
    pub fn new(view: impl Into<String>) -> Self {
        Self {
            view: view.into(),
            ..Self::default()
        }
    }
}

/// The exact package set of a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<'a> {
    /// The view name.
    pub view: String,
    /// One package per name.
    pub packages: BTreeMap<Name, Selected<'a>>,
    /// The effective member of every alternatives group present in the view,
    /// by group.
    pub alternatives: BTreeMap<Name, Name>,
    /// The constraints the search learned from conflicts and kept, in the
    /// order they were learned.
    pub learned: Vec<Learned>,
    /// Warnings to show the user.
    pub warnings: Vec<Warning>,
    /// The packages the view needs from a recipe snapshot, in name order.
    /// While these are unresolved, the view is incomplete: the packages'
    /// own dependencies are not in it. Building them and resolving again
    /// completes the view.
    pub builds: Vec<BuildRequest>,
}

/// A package of a [`Resolution`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected<'a> {
    /// The chosen candidate.
    pub candidate: Candidate<'a>,
    /// Why it is in the view: its requests first, then the dependencies it
    /// satisfies, in the order they were met.
    pub reasons: Vec<Reason>,
    /// Whether the installed version was kept by preference.
    pub kept_installed: bool,
}

impl Resolution<'_> {
    /// Explains why `name` is in the view, with the shortest chain of
    /// dependencies from a request. Returns `None` when the view does not
    /// contain `name`.
    pub fn why(&self, name: &Name) -> Option<Why> {
        let selected = self.packages.get(name)?;
        let chain = Chain::find(name, |n| {
            self.packages
                .get(n)
                .map(|s| (Package::of(s.candidate.meta), s.reasons.as_slice()))
        })?;
        let nearest = chain.steps.first().map(|s| &s.package.name);
        let mut also: Vec<Package> = selected
            .reasons
            .iter()
            .filter_map(|r| match r {
                Reason::Dependency { by, .. } if Some(by) != nearest && by != name => {
                    self.packages.get(by).map(|s| Package::of(s.candidate.meta))
                }
                _ => None,
            })
            .collect();
        also.sort();
        also.dedup();
        Some(Why {
            view: self.view.clone(),
            package: Package::of(selected.candidate.meta),
            chain,
            also,
        })
    }
}

impl<'a> Universe<'a> {
    /// Resolves the requests of one view into one package per name.
    ///
    /// Each pass selects greedily: requests first, in name order, then
    /// dependencies breadth first. Without an upgrade, the installed version
    /// of a name is kept whenever it satisfies everything. Conflicts,
    /// replacements and alternatives are checked on the complete set.
    ///
    /// When a pass hits a conflict, the resolver learns a constraint that
    /// rules the conflict out and starts a new pass. A dependency that an
    /// earlier choice contradicts is first learned as a constraint on the
    /// depended-on name, and if that leads nowhere, the depending package is
    /// avoided instead, so an older or other candidate of it is tried. Every
    /// other failure avoids the package whose dependency or conflict failed.
    /// The search backtracks over these choices depth first, in a fixed
    /// order, so it finds a resolution whenever one exists that does not
    /// depend on choosing between several provider names. When no
    /// resolution exists, the error is the first failure the search met.
    /// The search makes a bounded number of passes. When they run out, it
    /// reports the first failure it met, or S009 if it met none.
    ///
    /// The result depends only on the universe's contents and on `input`,
    /// never on the order of repositories, index entries or requests.
    ///
    /// # Errors
    ///
    /// Fails with an [`Error`] that names the view, the error code and the
    /// chain of dependencies back to a request, as described on
    /// [`ErrorKind`].
    ///
    /// # Panics
    ///
    /// Does not panic on any input. The only `expect`s guard internal
    /// invariants, such as every selected package tracing back to a
    /// request.
    ///
    /// ```
    /// use laft_core::Timestamp;
    /// use laft_index::{Defaults, Index};
    /// use laft_resolve::{Repository, ResolveInput, Request, Universe};
    ///
    /// # let index = Index {
    /// #     format: Index::FORMAT,
    /// #     repo: "core".parse()?,
    /// #     channel: "current".parse()?,
    /// #     arch: laft_pkg::Arch::X86_64V3,
    /// #     generation: 1,
    /// #     timestamp: Timestamp::from_secs(0),
    /// #     expires: Timestamp::from_secs(86_400),
    /// #     defaults: Defaults::default(),
    /// #     packages: vec![],
    /// #     yanked: vec![],
    /// #     options: Default::default(),
    /// #     recipes: None,
    /// #     profile: None,
    /// # };
    /// let universe = Universe::new(&[Repository { index: &index, priority: 0 }]);
    /// let mut input = ResolveInput::new("system");
    /// input.requests.push(Request {
    ///     pkg: "ripgrep".parse()?,
    ///     origin: "/etc/laft/system.stav:8:22".into(),
    /// });
    /// let error = universe.resolve(&input).unwrap_err();
    /// assert_eq!(error.code(), Some("S001"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn resolve(&self, input: &ResolveInput<'a>) -> Result<Resolution<'a>, Error> {
        let fail = |kind| Error {
            view: input.view.clone(),
            kind,
        };
        let ctx = Ctx::new(self, input);
        let (requests, renames) = ctx.normalize().map_err(fail)?;
        let mut path: Vec<Fact<'a>> = Vec::new();
        let mut choices: Vec<(usize, std::vec::IntoIter<Fact<'a>>)> = Vec::new();
        let mut first = None;
        for _ in 0..MAX_PASSES {
            let mut pass = Pass::new(&ctx, &requests, &path);
            let options = match pass.run() {
                Ok(()) => {
                    let alternatives = pass.alternatives().map_err(fail)?;
                    let mut warnings = renames;
                    warnings.extend(pass.warnings());
                    let builds = pass.builds();
                    return Ok(Resolution {
                        view: input.view.clone(),
                        packages: pass.into_packages(),
                        alternatives,
                        learned: path.iter().map(Fact::learned).collect(),
                        warnings,
                        builds,
                    });
                }
                Err(Stop::Fatal(kind)) => return Err(fail(kind)),
                Err(Stop::Conflict { error, options }) => {
                    if let Some(error) = error {
                        first.get_or_insert(error);
                    }
                    options
                }
            };
            choices.push((path.len(), options.into_iter()));
            loop {
                let Some((len, options)) = choices.last_mut() else {
                    // Every conflict without an error offers options, the
                    // search tries each of them, and no option repeats a fact
                    // already on the path, so an exhausted tree has recorded
                    // at least one failure.
                    let error = first.expect("every exhausted branch ends in a failure");
                    return Err(fail(error));
                };
                path.truncate(*len);
                if let Some(fact) = options.next() {
                    path.push(fact);
                    break;
                }
                choices.pop();
            }
        }
        Err(fail(first.unwrap_or_else(|| {
            Box::new(ErrorKind::RestartLimit {
                passes: MAX_PASSES,
                learned: path.iter().map(Fact::learned).collect(),
            })
        })))
    }
}

/// A constraint the search assumes on its current branch.
#[derive(Debug, Clone, Copy)]
enum Fact<'a> {
    /// Every candidate of `name` must satisfy `dep`, which `by` has.
    Require {
        name: &'a Name,
        dep: &'a Dep,
        by: &'a Meta,
    },
    /// The candidate is never selected.
    Avoid(Candidate<'a>),
}

impl Fact<'_> {
    fn learned(&self) -> Learned {
        match *self {
            Self::Require { name, dep, by } => Learned::Require {
                name: name.clone(),
                dep: dep.clone(),
                by: Package::of(by),
            },
            Self::Avoid(candidate) => Learned::Avoid {
                package: Package::of(candidate.meta),
            },
        }
    }
}

/// Whether every constraint holds for the recorded option values. A
/// constraint on an option the values do not record fails.
/// Whether the values a package records hold every constraint of a
/// dependency on it. An option it does not record cannot be checked, so
/// its constraint holds; `laft-repo check` names such constraints KR015.
fn constraints_hold(
    options: &BTreeMap<String, OptionValue>,
    constraints: &OptionConstraints,
) -> bool {
    constraints
        .as_slice()
        .iter()
        .all(|c| options.get(c.name()).is_none_or(|v| c.matches(v)))
}

/// Whether `options` holds every constraint, failing an option that the
/// package does not record. A request comes from the configuration, so a
/// missing value is a demand nothing satisfies.
fn constraints_hold_strict(
    options: &BTreeMap<String, OptionValue>,
    constraints: &OptionConstraints,
) -> bool {
    constraints
        .as_slice()
        .iter()
        .all(|c| options.get(c.name()).is_some_and(|v| c.matches(v)))
}

/// The value a constraint demands.
fn canonical(c: &OptionConstraint) -> OptionValue {
    match c {
        OptionConstraint::True { .. } => OptionValue::Bool(true),
        OptionConstraint::False { .. } => OptionValue::Bool(false),
        OptionConstraint::Tag { tag, .. } => OptionValue::Tag(tag.clone()),
    }
}

/// The value a constraint demands, as `option=value`.
fn canonical_text(c: &OptionConstraint) -> String {
    format!("{}={}", c.name(), canonical(c))
}

/// Whether `meta` satisfies `dep`: by its version and options for a
/// dependency on its own name, by replacing the name for a dependency on
/// another name, and by its provides otherwise.
fn satisfies(meta: &Meta, dep: &Dep) -> bool {
    match dep {
        Dep::Pkg(name, req, opts) if *name == meta.name => {
            req.matches(&meta.version, meta.rev) && constraints_hold(&meta.options, opts)
        }
        Dep::Pkg(name, _, _) => meta
            .replaces
            .iter()
            .any(|r| matches!(r, Dep::Pkg(replaced, _, _) if replaced == name)),
        _ => provides(meta, dep),
    }
}

/// The installed package of a name, as the universe sees it.
#[derive(Debug, Clone, Copy)]
enum InstalledState<'a> {
    /// The owner's index lists the same store id.
    Listed(Candidate<'a>),
    /// Visible next to the index candidates: the owner pruned it, or no
    /// repository has the name at all.
    Extra {
        candidate: Candidate<'a>,
        orphan: bool,
    },
    /// The owner yanked it, so it is not a candidate.
    Yanked {
        candidate: Candidate<'a>,
        yank: &'a Yank,
    },
}

/// The requests of one name, merged.
#[derive(Debug)]
struct Normalized {
    name: Name,
    variant: Variant,
    prefix: Option<VersionPrefix>,
    /// The option constraints of the requests, merged and deduped.
    options: OptionConstraints,
    /// The origin and text of the request that carried each constraint,
    /// index-aligned with `options`. Two requests may demand different
    /// values of one option.
    asked: Vec<(String, String)>,
    /// The request whose prefix was kept, as written.
    text: String,
    origin: String,
    reasons: Vec<Reason>,
}

/// What stays the same across the passes of one resolution.
struct Ctx<'u, 'a> {
    universe: &'u Universe<'a>,
    input: &'u ResolveInput<'a>,
    installed: BTreeMap<&'u str, InstalledState<'a>>,
    extra_provides: BTreeMap<Key<'a>, Vec<&'a Name>>,
    /// The packages a recipe snapshot can build, one per name.
    buildable: BTreeMap<&'u str, &'u Buildable>,
    /// The candidates of names whose order the view's option values and the
    /// declarations refine: best first by the preference order of the view.
    option_sorted: HashMap<&'u str, Vec<Candidate<'a>>>,
}

impl<'u, 'a> Ctx<'u, 'a> {
    fn new(universe: &'u Universe<'a>, input: &'u ResolveInput<'a>) -> Self {
        let mut installed = BTreeMap::new();
        let mut extra_provides: BTreeMap<Key<'a>, Vec<&'a Name>> = BTreeMap::new();
        let mut buildable: BTreeMap<&'u str, &'u Buildable> = BTreeMap::new();
        fn rank(b: &Buildable) -> (&Version, u32, &Variant) {
            (&b.version, b.rev, &b.variant)
        }
        for entry in &input.buildable {
            match buildable.entry(entry.name.as_str()) {
                std::collections::btree_map::Entry::Occupied(mut at)
                    if rank(entry) > rank(at.get()) =>
                {
                    at.insert(entry);
                }
                std::collections::btree_map::Entry::Vacant(at) => {
                    at.insert(entry);
                }
                _ => {}
            }
        }
        for (name, inst) in &input.installed {
            if inst.meta.name != *name {
                continue;
            }
            let candidate = Candidate {
                id: inst.id,
                meta: inst.meta,
                source: Source::Installed,
            };
            let state = match universe.owner(name.as_str()) {
                None => InstalledState::Extra {
                    candidate,
                    orphan: true,
                },
                Some(owner) => {
                    if let Some(listed) = owner.candidates.iter().find(|c| c.id == inst.id) {
                        InstalledState::Listed(*listed)
                    } else if let Some(yank) = owner.yanked.iter().find(|y| y.id == inst.id) {
                        InstalledState::Yanked { candidate, yank }
                    } else {
                        InstalledState::Extra {
                            candidate,
                            orphan: false,
                        }
                    }
                }
            };
            if let InstalledState::Extra { candidate, .. } = state {
                for provide in &candidate.meta.provides {
                    extra_provides
                        .entry(Key::of_provide(provide))
                        .or_default()
                        .push(&candidate.meta.name);
                }
            }
            installed.insert(name.as_str(), state);
        }
        let mut option_sorted: HashMap<&'u str, Vec<Candidate<'a>>> = HashMap::new();
        let names_with_values = input
            .options
            .keys()
            .map(|n| n.as_str())
            .chain(universe.decls.keys().copied());
        for of in names_with_values {
            let Some(owner) = universe.owner(of) else {
                continue;
            };
            if owner.repos.len() != 1 || owner.candidates.len() < 2 {
                continue;
            }
            let decls = universe.decls(of).unwrap_or(&[]);
            let request = input
                .options
                .iter()
                .find(|(n, _)| n.as_str() == of)
                .map(|(_, r)| r);
            let mut list = owner.candidates.clone();
            list.sort_by(|a, b| {
                order(a, b)
                    .then_with(|| option_order(a, b, decls, request))
                    .then_with(|| a.id.cmp(&b.id))
            });
            option_sorted.insert(of, list);
        }
        Self {
            universe,
            input,
            installed,
            extra_provides,
            buildable,
            option_sorted,
        }
    }

    /// The package a recipe snapshot can build, when the snapshot has one.
    fn buildable(&self, name: &Name) -> Option<&'u Buildable> {
        self.buildable.get(name.as_str()).copied()
    }

    fn prefer(&self, name: &Name) -> bool {
        match &self.input.upgrade {
            Upgrade::None => true,
            Upgrade::All => false,
            Upgrade::Names(names) => !names.contains(name),
        }
    }

    /// The candidates of `name`: the owner's, best first by the preference
    /// order of the view, and a visible installed candidate that the owner
    /// does not list.
    fn candidates<'s>(
        &'s self,
        name: &str,
    ) -> Result<(&'s [Candidate<'a>], Option<Candidate<'a>>), Box<ErrorKind>> {
        if let Some(sorted) = self.option_sorted.get(name) {
            let extra = match self.installed.get(name) {
                Some(InstalledState::Extra { candidate, .. }) => Some(*candidate),
                _ => None,
            };
            return Ok((sorted.as_slice(), extra));
        }
        let listed = match self.universe.owner(name) {
            Some(owner) if owner.repos.len() > 1 => {
                return Err(Box::new(ErrorKind::EqualPriority {
                    name: owner.name.clone(),
                    repos: owner.repos.iter().map(|r| (*r).clone()).collect(),
                    priority: owner.priority,
                }));
            }
            Some(owner) => owner.candidates.as_slice(),
            None => &[],
        };
        let extra = match self.installed.get(name) {
            Some(InstalledState::Extra { candidate, .. }) => Some(*candidate),
            _ => None,
        };
        Ok((listed, extra))
    }

    /// Every candidate of `name`, best first.
    fn ordered<'s>(
        &'s self,
        name: &str,
    ) -> Result<impl Iterator<Item = Candidate<'a>> + 's, Box<ErrorKind>> {
        let (listed, extra) = self.candidates(name)?;
        let split = extra.map_or(listed.len(), |e| {
            listed.partition_point(|c| order(c, &e).is_lt())
        });
        Ok(listed[..split]
            .iter()
            .copied()
            .chain(extra)
            .chain(listed[split..].iter().copied()))
    }

    fn available(&self, name: &str) -> Vec<Package> {
        self.ordered(name)
            .map(|cands| cands.map(|c| Package::of(c.meta)).collect())
            .unwrap_or_default()
    }

    /// The candidate of `name` to select among those `accept` takes, and
    /// whether it is the installed one kept by preference.
    ///
    /// When the installed version is preferred and accepted, it wins.
    /// Otherwise the best accepted candidate wins, except that with
    /// `dynamic_first` the best dynamic one wins over every static one.
    fn best(
        &self,
        name: &Name,
        accept: impl Fn(&Candidate<'a>) -> bool,
        dynamic_first: bool,
    ) -> Result<Option<(Candidate<'a>, bool)>, Box<ErrorKind>> {
        let name = name.as_str();
        let installed = match self.installed.get(name) {
            Some(InstalledState::Listed(c) | InstalledState::Extra { candidate: c, .. }) => {
                Some(*c)
            }
            _ => None,
        };
        let candidates = self.ordered(name)?;
        if let Some(c) = installed
            && self.prefer(&c.meta.name)
            && accept(&c)
        {
            return Ok(Some((c, true)));
        }
        let mut found = None;
        for c in candidates.filter(|c| accept(c)) {
            if !dynamic_first || c.meta.variant == Variant::Dynamic {
                return Ok(Some((c, false)));
            }
            found.get_or_insert(c);
        }
        Ok(found.map(|c| (c, false)))
    }

    fn is_ambiguous(&self, name: &str) -> bool {
        self.universe.owner(name).is_some_and(|o| o.repos.len() > 1)
    }

    /// The names with a visible candidate that provides `key`, sorted.
    fn provider_names(&self, key: Key<'a>) -> Vec<&'a Name> {
        let listed = self.universe.providers(key);
        match self.extra_provides.get(&key) {
            None => listed.to_vec(),
            Some(extra) => {
                let mut all: Vec<&'a Name> = listed.iter().chain(extra).copied().collect();
                all.sort();
                all.dedup();
                all
            }
        }
    }

    fn yanked(&self, name: &str) -> Option<(Candidate<'a>, &'a Yank)> {
        match self.installed.get(name) {
            Some(InstalledState::Yanked { candidate, yank }) => Some((*candidate, *yank)),
            _ => None,
        }
    }

    /// Whether `name` has a visible candidate: an index candidate of its
    /// owner, an installed package the owner pruned or no repository lists,
    /// or a package a recipe snapshot can build. An ambiguous name counts,
    /// because several repositories list it. A yanked installed package
    /// does not count.
    fn has_candidates(&self, name: &str) -> bool {
        self.buildable.contains_key(name)
            || self.is_ambiguous(name)
            || self
                .ordered(name)
                .is_ok_and(|mut candidates| candidates.next().is_some())
    }

    /// Sorts the requests, follows renames and merges the requests of each
    /// name. A request follows a rename only without a version prefix,
    /// because the replacing package has versions of its own. A replacer is
    /// always a name with candidates, so one step reaches it.
    fn normalize(&self) -> Result<(Vec<Normalized>, Vec<Warning>), Box<ErrorKind>> {
        let mut requests: Vec<&Request> = self.input.requests.iter().collect();
        requests.sort_by_cached_key(|r| (r.pkg.to_string(), r.origin.clone()));
        let mut warnings = Vec::new();
        let mut groups: BTreeMap<Name, Vec<(&Request, Option<&VersionPrefix>)>> = BTreeMap::new();
        for request in requests {
            let name = request.pkg.name();
            let mut target = (name.clone(), request.pkg.prefix());
            if !self.has_candidates(name.as_str()) {
                match self.universe.replacers(name.as_str()) {
                    [] => {}
                    [one] if request.pkg.prefix().is_none() => {
                        warnings.push(Warning::Renamed {
                            from: name.clone(),
                            to: (*one).clone(),
                            origin: request.origin.clone(),
                        });
                        target = ((*one).clone(), None);
                    }
                    replacers => {
                        return Err(Box::new(ErrorKind::NoCandidate {
                            request: request.pkg.to_string(),
                            origin: request.origin.clone(),
                            available: Vec::new(),
                            replaced_by: replacers.iter().map(|n| (*n).clone()).collect(),
                        }));
                    }
                }
            }
            groups
                .entry(target.0)
                .or_default()
                .push((request, target.1));
        }

        let mut normalized = Vec::with_capacity(groups.len());
        for (name, group) in groups {
            let listing = || {
                let mut requests: Vec<(String, String)> = group
                    .iter()
                    .map(|(r, _)| (r.pkg.to_string(), r.origin.clone()))
                    .collect();
                requests.sort();
                requests
            };
            let variant = group[0].0.pkg.variant();
            if group.iter().any(|(r, _)| r.pkg.variant() != variant) {
                return Err(Box::new(ErrorKind::ConflictingRequests {
                    name,
                    requests: listing(),
                    variants: true,
                }));
            }
            let prefixes: Vec<&VersionPrefix> = group.iter().filter_map(|(_, p)| *p).collect();
            let kept = group
                .iter()
                .find(|(_, p)| p.is_some_and(|p| prefixes.iter().all(|q| p.is_within(q))));
            if kept.is_none() && !prefixes.is_empty() {
                return Err(Box::new(ErrorKind::ConflictingRequests {
                    name,
                    requests: listing(),
                    variants: false,
                }));
            }
            let (head, prefix) = kept.copied().unwrap_or(group[0]);
            let mut options: Vec<OptionConstraint> = Vec::new();
            let mut asked: Vec<(String, String)> = Vec::new();
            for (r, _) in &group {
                for c in r.pkg.options().as_slice() {
                    if !options.contains(c) {
                        options.push(c.clone());
                        asked.push((r.origin.clone(), r.pkg.to_string()));
                    }
                }
            }
            normalized.push(Normalized {
                name,
                variant,
                prefix: prefix.cloned(),
                options: options.into(),
                asked,
                text: head.pkg.to_string(),
                origin: head.origin.clone(),
                reasons: group
                    .iter()
                    .map(|(r, _)| Reason::Requested {
                        origin: r.origin.clone(),
                        text: r.pkg.to_string(),
                    })
                    .collect(),
            });
        }
        Ok((normalized, warnings))
    }
}

/// Why a pass ended early.
enum Stop<'a> {
    /// No other choice can avoid the failure, so the search ends.
    Fatal(Box<ErrorKind>),
    /// A conflict that a learned constraint may avoid.
    Conflict {
        /// What to report when no option leads to a resolution, or `None`
        /// for a dependency that contradicts an earlier choice, which is not
        /// a failure by itself.
        error: Option<Box<ErrorKind>>,
        /// Constraints that each rule the conflict out, in the order to
        /// try them. Together they cover every resolution that the
        /// constraints of the branch allow, except that a choice between
        /// several provider names is never revisited.
        options: Vec<Fact<'a>>,
    },
}

impl From<Box<ErrorKind>> for Stop<'_> {
    fn from(kind: Box<ErrorKind>) -> Self {
        Self::Fatal(kind)
    }
}

impl<'a> Stop<'a> {
    fn failed(error: ErrorKind, options: Vec<Fact<'a>>) -> Self {
        Self::Conflict {
            error: Some(Box::new(error)),
            options,
        }
    }
}

struct Sel<'a> {
    candidate: Candidate<'a>,
    reasons: Vec<Reason>,
    kept: bool,
}

/// A package the snapshot builds, selected for this pass.
struct BuildSel<'u> {
    buildable: &'u Buildable,
    values: BTreeMap<String, OptionValue>,
    reasons: Vec<Reason>,
}

/// What a selection asks of a candidate.
struct Want<'a> {
    /// The version requirement.
    req: VersionReq,
    /// The variant a request names. A dependency names none and takes the
    /// dynamic variant when one exists.
    variant: Option<Variant>,
    /// Whether the best dynamic candidate wins over every static one.
    dynamic_first: bool,
    /// The option constraints of the request or the dependency.
    options: &'a OptionConstraints,
    /// The origin and text of the request that carried each constraint,
    /// index-aligned with `options`. Empty for a dependency, whose
    /// constraints share one source.
    asked: &'a [(String, String)],
    /// Whether a candidate that does not record the constrained option
    /// fails. A request comes from the configuration, which can demand
    /// values no published package has; a dependency on a package that
    /// records no options is unverifiable and holds.
    strict: bool,
}

impl Want<'_> {
    /// Whether a package with this version and revision is wanted.
    fn matches(&self, version: &Version, rev: u32) -> bool {
        self.req.matches(version, rev)
    }
}

/// What a selection found.
enum Pick<'a> {
    /// A package from an index or the store, and whether it is the
    /// installed one kept by preference.
    Package(Candidate<'a>, bool),
    /// A package the snapshot builds with these values.
    Build(BTreeMap<String, OptionValue>),
}

/// Where the option values of a demanded build come from.
#[derive(Clone, Copy)]
enum Asked<'p> {
    /// A request made at this location, as written.
    Request { origin: &'p str, text: &'p str },
    /// A dependency of this package.
    Dependency(&'p Meta, &'p Dep),
}

/// One greedy pass over the requests and their closure.
struct Pass<'p, 'u, 'a> {
    ctx: &'p Ctx<'u, 'a>,
    requests: &'p [Normalized],
    /// The constraints of the branch, in the order they were learned.
    facts: &'p [Fact<'a>],
    required: BTreeMap<&'a Name, Vec<&'a Dep>>,
    avoided: BTreeSet<StoreId>,
    selected: BTreeMap<Name, Sel<'a>>,
    builds: BTreeMap<Name, BuildSel<'u>>,
    providers: BTreeMap<Key<'a>, BTreeSet<&'a Name>>,
    queue: VecDeque<&'a Name>,
}

impl<'p, 'u, 'a> Pass<'p, 'u, 'a> {
    fn new(ctx: &'p Ctx<'u, 'a>, requests: &'p [Normalized], facts: &'p [Fact<'a>]) -> Self {
        let mut required: BTreeMap<&'a Name, Vec<&'a Dep>> = BTreeMap::new();
        let mut avoided = BTreeSet::new();
        for fact in facts {
            match *fact {
                Fact::Require { name, dep, .. } => required.entry(name).or_default().push(dep),
                Fact::Avoid(candidate) => {
                    avoided.insert(candidate.id);
                }
            }
        }
        Self {
            ctx,
            requests,
            facts,
            required,
            avoided,
            selected: BTreeMap::new(),
            builds: BTreeMap::new(),
            providers: BTreeMap::new(),
            queue: VecDeque::new(),
        }
    }

    /// Whether the branch allows `candidate` of `name`: not avoided, every
    /// learned constraint satisfied, and every fixed option value of the
    /// configuration recorded with exactly that value.
    fn allowed(&self, name: &Name, candidate: &Candidate<'_>) -> bool {
        !self.avoided.contains(&candidate.id)
            && self.fixed_ok(name, &candidate.meta.options)
            && self
                .required
                .get(name)
                .is_none_or(|deps| deps.iter().all(|dep| satisfies(candidate.meta, dep)))
    }

    /// Whether the configuration's fixed values all appear in `options`.
    /// An option the package does not record fails, so a candidate built
    /// before the recipe had options is never selected for a name whose
    /// options the configuration sets.
    fn fixed_ok(&self, name: &Name, options: &BTreeMap<String, OptionValue>) -> bool {
        match self.ctx.input.options.get(name) {
            None => true,
            Some(req) => req
                .fixed
                .iter()
                .all(|(opt, fixed)| options.get(opt) == Some(&fixed.value)),
        }
    }

    /// The selection for `name`: the best acceptable candidate, the
    /// installed one first, and a build the snapshot offers only when it
    /// beats every published candidate.
    fn choose(&self, name: &Name, want: &Want<'_>) -> Result<Option<Pick<'a>>, Box<ErrorKind>> {
        let accept = |c: &Candidate<'a>| {
            want.matches(&c.meta.version, c.meta.rev)
                && want.variant.is_none_or(|v| c.meta.variant == v)
                && (if want.strict {
                    constraints_hold_strict(&c.meta.options, want.options)
                } else {
                    constraints_hold(&c.meta.options, want.options)
                })
                && self.allowed(name, c)
        };
        let best = self.ctx.best(name, accept, want.dynamic_first)?;
        if let Some((c, true)) = best {
            return Ok(Some(Pick::Package(c, true)));
        }
        if let Some(values) = self.buildable_values(name, want) {
            let b = self
                .ctx
                .buildable(name)
                .expect("buildable values imply a buildable");
            let wins = match best {
                Some((c, _)) => (&b.version, b.rev) > (&c.meta.version, c.meta.rev),
                None => true,
            };
            if wins {
                return Ok(Some(Pick::Build(values)));
            }
        }
        Ok(best.map(|(c, kept)| Pick::Package(c, kept)))
    }

    /// The values a build of `name` would use, when the snapshot has the
    /// package and the values satisfy `want` and every constraint of the
    /// branch: the declared defaults, with the fixed values of the view and
    /// the option constraints of the learned dependencies and of `want`
    /// applied over them, in that order. The build takes the option
    /// context of the view; `dep_options` of the build planning decides
    /// how its build dependencies resolve.
    fn buildable_values(
        &self,
        name: &Name,
        want: &Want<'_>,
    ) -> Option<BTreeMap<String, OptionValue>> {
        let b = self.ctx.buildable(name)?;
        if !want.matches(&b.version, b.rev) || want.variant.is_some_and(|v| v != b.variant) {
            return None;
        }
        if want.variant.is_none()
            && want.dynamic_first
            && b.variant != Variant::Dynamic
            && self.ctx.ordered(name.as_str()).is_ok_and(|mut candidates| {
                candidates.any(|c| {
                    c.meta.variant == Variant::Dynamic
                        && want.matches(&c.meta.version, c.meta.rev)
                        && constraints_hold(&c.meta.options, want.options)
                        && self.allowed(name, &c)
                })
            })
        {
            return None;
        }
        let mut values = b.defaults.clone();
        if let Some(req) = self.ctx.input.options.get(name) {
            for (opt, fixed) in &req.fixed {
                values.insert(opt.clone(), fixed.value.clone());
            }
        }
        let apply = |constraints: &OptionConstraints, values: &mut BTreeMap<_, _>| {
            for c in constraints.as_slice() {
                let value = match c {
                    OptionConstraint::True { .. } => OptionValue::Bool(true),
                    OptionConstraint::False { .. } => OptionValue::Bool(false),
                    OptionConstraint::Tag { tag, .. } => OptionValue::Tag(tag.clone()),
                };
                values.insert(c.name().to_owned(), value);
            }
        };
        let demanded = self.required.get(name).into_iter().flatten();
        let mut applied: Vec<&OptionConstraints> = Vec::with_capacity(2);
        for dep in demanded {
            if let Dep::Pkg(_, _, opts) = dep {
                apply(opts, &mut values);
                applied.push(opts);
            }
        }
        apply(want.options, &mut values);
        applied.push(want.options);
        let holds = applied.iter().all(|opts| constraints_hold(&values, opts))
            && self.fixed_ok(name, &values);
        holds.then_some(values)
    }

    fn select(&mut self, candidate: Candidate<'a>, reasons: Vec<Reason>, kept: bool) {
        let name = &candidate.meta.name;
        for provide in &candidate.meta.provides {
            self.providers
                .entry(Key::of_provide(provide))
                .or_default()
                .insert(name);
        }
        self.selected.insert(
            name.clone(),
            Sel {
                candidate,
                reasons,
                kept,
            },
        );
        self.queue.push_back(name);
    }

    /// Records that the view selects a package the snapshot builds, or
    /// fails with S011 when building is not allowed.
    fn build(
        &mut self,
        name: &Name,
        values: BTreeMap<String, OptionValue>,
        want: &Want<'_>,
        asked: Option<Asked<'_>>,
        reasons: Vec<Reason>,
    ) -> Result<(), Stop<'a>> {
        if !self.ctx.input.allow_builds {
            return Err(Stop::failed(
                self.nothing_published(name, &values, want, asked),
                Vec::new(),
            ));
        }
        let buildable = self
            .ctx
            .buildable(name)
            .expect("buildable values imply a buildable");
        self.builds.insert(
            name.clone(),
            BuildSel {
                buildable,
                values,
                reasons,
            },
        );
        Ok(())
    }

    /// S011 for `name`: the values the build would use, where each value
    /// that deviates from the declared defaults comes from, and the
    /// published candidates that match on version and variant.
    fn nothing_published(
        &self,
        name: &Name,
        values: &BTreeMap<String, OptionValue>,
        want: &Want<'_>,
        asked: Option<Asked<'_>>,
    ) -> ErrorKind {
        let b = self
            .ctx
            .buildable(name)
            .expect("buildable values imply a buildable");
        let empty = BTreeMap::new();
        let fixed = self
            .ctx
            .input
            .options
            .get(name)
            .map(|req| &req.fixed)
            .unwrap_or(&empty);
        let sources = values
            .iter()
            .filter(|(opt, v)| b.defaults.get(*opt) != Some(*v))
            .filter_map(|(opt, v)| {
                let text = format!("{opt}={v}");
                if let Some(f) = fixed.get(opt) {
                    return Some((text, ValueOrigin::Config(f.origin.clone())));
                }
                self.facts
                    .iter()
                    .filter_map(|fact| match *fact {
                        Fact::Require {
                            name: constrained,
                            dep: Dep::Pkg(_, _, opts),
                            by,
                        } if *constrained == *name
                            && opts.as_slice().iter().any(|c| c.name() == opt) =>
                        {
                            Some((
                                text.clone(),
                                ValueOrigin::Dependency(Package::of(by), format!("{name}{opts}")),
                            ))
                        }
                        _ => None,
                    })
                    .next()
                    .or_else(|| {
                        want.options
                            .as_slice()
                            .iter()
                            .any(|c| c.name() == opt)
                            .then(|| match asked {
                                Some(Asked::Dependency(by, dep)) => (
                                    text.clone(),
                                    ValueOrigin::Dependency(Package::of(by), dep.to_string()),
                                ),
                                Some(Asked::Request { origin, .. }) => {
                                    (text.clone(), ValueOrigin::Request(origin.to_owned()))
                                }
                                None => (text.clone(), ValueOrigin::Request(String::new())),
                            })
                    })
            })
            .collect();
        let published = self
            .ctx
            .ordered(name.as_str())
            .map(|candidates| {
                candidates
                    .filter(|c| {
                        want.matches(&c.meta.version, c.meta.rev)
                            && want.variant.is_none_or(|v| c.meta.variant == v)
                    })
                    .map(|c| (Package::of(c.meta), self.published_set(name, c.meta)))
                    .collect()
            })
            .unwrap_or_default();
        let vals = values.iter().map(|(opt, v)| format!("{opt}={v}")).collect();
        ErrorKind::NothingPublished {
            name: name.clone(),
            values: vals,
            sources,
            published,
        }
    }

    /// The text after a published candidate's label in S011: the option
    /// values it was built with, as they differ from the declared defaults.
    fn published_set(&self, name: &Name, meta: &Meta) -> String {
        let Some(b) = self.ctx.buildable(name) else {
            return "defaults".to_owned();
        };
        if meta.options.is_empty() {
            return "no options".to_owned();
        }
        let diffs: Vec<String> = meta
            .options
            .iter()
            .filter(|(opt, v)| b.defaults.get(*opt) != Some(*v))
            .map(|(opt, v)| format!("{opt}={v}"))
            .collect();
        if diffs.is_empty() {
            "defaults".to_owned()
        } else {
            diffs.join(", ")
        }
    }

    /// The option failure that explains a selection of `name` finding no
    /// candidate while some candidate matches on version and variant: S013
    /// when a demanded value contradicts a fixed one, S012 when two demands
    /// contradict each other, and `None` when the options are not the
    /// cause.
    fn option_failure(&self, name: &Name, want: &Want<'_>, asked: Asked<'_>) -> Option<ErrorKind> {
        let matches = |c: &Candidate<'_>| {
            want.matches(&c.meta.version, c.meta.rev)
                && want.variant.is_none_or(|v| c.meta.variant == v)
        };
        if !self.ctx.ordered(name.as_str()).ok()?.any(|c| matches(&c)) {
            return None;
        }
        let empty = BTreeMap::new();
        let fixed = self
            .ctx
            .input
            .options
            .get(name)
            .map(|req| &req.fixed)
            .unwrap_or(&empty);
        let mut demands: Vec<(&OptionConstraint, Asked<'_>)> = want
            .options
            .as_slice()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                (
                    c,
                    want.asked
                        .get(i)
                        .map_or(asked, |(o, t)| Asked::Request { origin: o, text: t }),
                )
            })
            .collect();
        for fact in self.facts.iter() {
            if let Fact::Require {
                name: constrained,
                dep: dep @ Dep::Pkg(_, _, opts),
                by,
            } = *fact
                && *constrained == *name
            {
                for c in opts.as_slice() {
                    demands.push((c, Asked::Dependency(by, dep)));
                }
            }
        }
        for (i, (c, from)) in demands.iter().enumerate() {
            if let Some(f) = fixed.get(c.name()) {
                if !c.matches(&f.value) {
                    return Some(self.fixed_conflict(c, *from, f));
                }
                continue;
            }
            for (fc, other) in demands.iter().skip(i + 1) {
                if fc.name() == c.name() && canonical(fc) != canonical(c) {
                    return Some(ErrorKind::OptionConflict {
                        name: name.clone(),
                        demands: vec![self.demand(c, *from), self.demand(fc, *other)],
                    });
                }
            }
            for (dep, opts, by) in self.facts.iter().filter_map(|fact| match *fact {
                Fact::Require {
                    name: constrained,
                    dep: dep @ Dep::Pkg(_, _, opts),
                    by,
                } if *constrained == *name => Some((dep, opts, by)),
                _ => None,
            }) {
                for fc in opts.as_slice() {
                    if fc.name() == c.name() && canonical(fc) != canonical(c) {
                        return Some(ErrorKind::OptionConflict {
                            name: name.clone(),
                            demands: vec![
                                self.demand(c, *from),
                                self.demand(fc, Asked::Dependency(by, dep)),
                            ],
                        });
                    }
                }
            }
        }
        None
    }

    /// S013 for a constraint that a fixed value contradicts. The chain of
    /// a source that is not selected in this pass is unknown and left out.
    fn fixed_conflict(&self, c: &OptionConstraint, from: Asked<'_>, f: &FixedValue) -> ErrorKind {
        let (text, by, chain) = match from {
            Asked::Dependency(by, dep) => {
                let base = self.chain(&by.name);
                (dep.to_string(), Some(Package::of(by)), base)
            }
            Asked::Request { origin, text } => (
                text.to_owned(),
                None,
                Some(Chain {
                    steps: Vec::new(),
                    origin: origin.to_owned(),
                    request: text.to_owned(),
                }),
            ),
        };
        ErrorKind::FixedConflict {
            name: c.name().parse().expect("a constraint names an option"),
            text,
            by,
            chain,
            value: canonical_text(c),
            fixed: (format!("{}={}", c.name(), f.value), f.origin.clone()),
        }
    }

    /// The demand of one option constraint, with its source and chain. The
    /// chain of a source that is not selected in this pass is unknown and
    /// left out, and a request has none: its origin line says where it
    /// comes from.
    fn demand(&self, c: &OptionConstraint, from: Asked<'_>) -> Demand {
        let (origin, chain) = match from {
            Asked::Dependency(by, dep) => (
                ValueOrigin::Dependency(Package::of(by), dep.to_string()),
                self.chain(&by.name),
            ),
            Asked::Request { origin, .. } => (ValueOrigin::Request(origin.to_owned()), None),
        };
        Demand {
            value: canonical_text(c),
            origin,
            chain,
        }
    }

    /// How `name` got into the view, when it is selected in this pass. A
    /// fact may name a package that this pass never selected, so callers
    /// that cannot require a selection degrade to `None` instead.
    fn chain(&self, name: &Name) -> Option<Chain> {
        Chain::find(name, |n| {
            self.selected
                .get(n)
                .map(|s| (Package::of(s.candidate.meta), s.reasons.as_slice()))
        })
    }

    /// How the selected `name` got into the view. Only callers that hold
    /// the invariant call this.
    fn chain_selected(&self, name: &Name) -> Chain {
        self.chain(name)
            .expect("every selected package traces back to a request")
    }

    fn run(&mut self) -> Result<(), Stop<'a>> {
        for request in self.requests {
            let want = Want {
                req: request
                    .prefix
                    .as_ref()
                    .map_or(VersionReq::Any, |p| VersionReq::Prefix(p.clone())),
                variant: Some(request.variant),
                dynamic_first: false,
                options: &request.options,
                asked: &request.asked,
                strict: true,
            };
            match self.choose(&request.name, &want)? {
                Some(Pick::Package(candidate, kept)) => {
                    self.select(candidate, request.reasons.clone(), kept);
                }
                Some(Pick::Build(values)) => {
                    self.build(
                        &request.name,
                        values,
                        &want,
                        Some(Asked::Request {
                            origin: &request.origin,
                            text: &request.text,
                        }),
                        request.reasons.clone(),
                    )?;
                }
                None => {
                    let asked = Asked::Request {
                        origin: &request.origin,
                        text: &request.text,
                    };
                    if let Some(kind) = self.option_failure(&request.name, &want, asked) {
                        return Err(Stop::failed(kind, Vec::new()));
                    }
                    return Err(self.no_candidate(request)?);
                }
            }
        }
        while let Some(name) = self.queue.pop_front() {
            let candidate = self
                .selected
                .get(name)
                .map(|s| s.candidate)
                .expect("only selected names are queued");
            for dep in &candidate.meta.depends {
                self.satisfy(dep, candidate)?;
            }
        }
        self.check_conflicts()
    }

    fn no_candidate(&self, request: &Normalized) -> Result<Stop<'a>, Box<ErrorKind>> {
        let name = &request.name;
        let wanted = |c: &Candidate<'_>| {
            c.meta.variant == request.variant
                && request
                    .prefix
                    .as_ref()
                    .is_none_or(|p| p.matches(&c.meta.version))
        };
        if self.ctx.ordered(name.as_str())?.any(|c| wanted(&c)) {
            return Ok(Stop::failed(self.unsatisfiable(name, None), Vec::new()));
        }
        if let Some((candidate, yank)) = self.ctx.yanked(name.as_str())
            && wanted(&candidate)
        {
            return Ok(Stop::Fatal(Box::new(yanked(
                candidate,
                yank,
                request.text.clone(),
                None,
                Chain {
                    steps: Vec::new(),
                    origin: request.origin.clone(),
                    request: request.text.clone(),
                },
            ))));
        }
        Ok(Stop::Fatal(Box::new(ErrorKind::NoCandidate {
            request: request.text.clone(),
            origin: request.origin.clone(),
            available: self.ctx.available(name.as_str()),
            replaced_by: Vec::new(),
        })))
    }

    /// S005 for `name`: every constraint the branch requires of it, `extra`,
    /// and the request's prefix.
    fn unsatisfiable(&self, name: &Name, extra: Option<Bound>) -> ErrorKind {
        let mut constraints: Vec<Bound> = self
            .facts
            .iter()
            .filter_map(|fact| match *fact {
                Fact::Require { name: n, dep, by } if n == name => Some(Bound {
                    dep: dep.clone(),
                    from: BoundSource::Package(Package::of(by)),
                }),
                _ => None,
            })
            .collect();
        constraints.extend(extra);
        if let Some(request) = self.requests.iter().find(|r| r.name == *name)
            && (request.prefix.is_some() || !request.options.is_empty())
        {
            constraints.push(Bound {
                dep: Dep::Pkg(
                    name.clone(),
                    request
                        .prefix
                        .as_ref()
                        .map_or(VersionReq::Any, |p| VersionReq::Prefix(p.clone())),
                    request.options.clone(),
                ),
                from: BoundSource::Request(request.origin.clone()),
            });
        }
        ErrorKind::Unsatisfiable {
            name: name.clone(),
            constraints,
            available: self.ctx.available(name.as_str()),
        }
    }

    fn satisfy(&mut self, dep: &'a Dep, by: Candidate<'a>) -> Result<(), Stop<'a>> {
        let reason = Reason::Dependency {
            by: by.meta.name.clone(),
            dep: dep.clone(),
        };
        match (dep, Key::of_dep(dep)) {
            (Dep::Pkg(name, req, opts), _) => self.satisfy_name(dep, name, req, opts, by, reason),
            (_, Some(key)) => self.satisfy_provide(dep, key, by, reason),
            (_, None) => Ok(()),
        }
    }

    /// Satisfies a dependency on `name`. A dependency without a version
    /// requirement on a name without candidates follows the rename when
    /// exactly one package name replaces it, and is met by a candidate of
    /// that name that replaces it.
    fn satisfy_name(
        &mut self,
        dep: &'a Dep,
        name: &'a Name,
        req: &'a VersionReq,
        opts: &'a OptionConstraints,
        by: Candidate<'a>,
        reason: Reason,
    ) -> Result<(), Stop<'a>> {
        if let Some(sel) = self.selected.get_mut(name) {
            let meta = sel.candidate.meta;
            if req.matches(&meta.version, meta.rev) && constraints_hold(&meta.options, opts) {
                sel.reasons.push(reason);
                return Ok(());
            }
            return Err(Stop::Conflict {
                error: None,
                options: vec![
                    Fact::Require {
                        name,
                        dep,
                        by: by.meta,
                    },
                    Fact::Avoid(by),
                ],
            });
        }
        if let Some(build) = self.builds.get_mut(name) {
            let b = build.buildable;
            if req.matches(&b.version, b.rev) && constraints_hold(&build.values, opts) {
                build.reasons.push(reason);
                return Ok(());
            }
            return Err(Stop::Conflict {
                error: None,
                options: vec![
                    Fact::Require {
                        name,
                        dep,
                        by: by.meta,
                    },
                    Fact::Avoid(by),
                ],
            });
        }
        let want = Want {
            req: req.clone(),
            variant: None,
            dynamic_first: true,
            options: opts,
            asked: &[],
            strict: false,
        };
        match self.choose(name, &want)? {
            Some(Pick::Package(candidate, kept)) => {
                self.select(candidate, vec![reason], kept);
                return Ok(());
            }
            Some(Pick::Build(values)) => {
                return self.build(
                    name,
                    values,
                    &want,
                    Some(Asked::Dependency(by.meta, dep)),
                    vec![reason],
                );
            }
            None => {
                let asked = Asked::Dependency(by.meta, dep);
                if let Some(kind) = self.option_failure(name, &want, asked) {
                    return Err(Stop::failed(kind, vec![Fact::Avoid(by)]));
                }
            }
        }
        if *req == VersionReq::Any
            && let [one] = self.ctx.universe.replacers(name.as_str())
            && !self.ctx.has_candidates(name.as_str())
        {
            return self.satisfy_replacement(dep, one, by, reason);
        }
        let error = if self
            .ctx
            .ordered(name.as_str())?
            .any(|c| req.matches(&c.meta.version, c.meta.rev))
        {
            let bound = Bound {
                dep: dep.clone(),
                from: BoundSource::Package(Package::of(by.meta)),
            };
            self.unsatisfiable(name, Some(bound))
        } else if let Some((candidate, yank)) = self.ctx.yanked(name.as_str())
            && req.matches(&candidate.meta.version, candidate.meta.rev)
        {
            yanked(
                candidate,
                yank,
                dep.to_string(),
                Some(Package::of(by.meta)),
                self.chain_selected(&by.meta.name),
            )
        } else {
            ErrorKind::Unsatisfied {
                dep: dep.clone(),
                by: Package::of(by.meta),
                chain: self.chain_selected(&by.meta.name),
                available: self.ctx.available(name.as_str()),
            }
        };
        Err(Stop::failed(error, vec![Fact::Avoid(by)]))
    }

    /// Satisfies `dep`, on a name without candidates, with a candidate of
    /// `to` that replaces the name.
    fn satisfy_replacement(
        &mut self,
        dep: &'a Dep,
        to: &'a Name,
        by: Candidate<'a>,
        reason: Reason,
    ) -> Result<(), Stop<'a>> {
        if let Some(sel) = self.selected.get_mut(to) {
            if satisfies(sel.candidate.meta, dep) {
                sel.reasons.push(reason);
                return Ok(());
            }
            return Err(Stop::Conflict {
                error: None,
                options: vec![
                    Fact::Require {
                        name: to,
                        dep,
                        by: by.meta,
                    },
                    Fact::Avoid(by),
                ],
            });
        }
        let accept = |c: &Candidate<'a>| satisfies(c.meta, dep) && self.allowed(to, c);
        if let Some((candidate, kept)) = self.ctx.best(to, accept, true)? {
            self.select(candidate, vec![reason], kept);
            return Ok(());
        }
        let error = ErrorKind::Unsatisfied {
            dep: dep.clone(),
            by: Package::of(by.meta),
            chain: self.chain_selected(&by.meta.name),
            available: self.ctx.available(to.as_str()),
        };
        Err(Stop::failed(error, vec![Fact::Avoid(by)]))
    }

    fn satisfy_provide(
        &mut self,
        dep: &'a Dep,
        key: Key<'a>,
        by: Candidate<'a>,
        reason: Reason,
    ) -> Result<(), Stop<'a>> {
        let satisfied = self.providers.get(&key).and_then(|names| {
            names
                .iter()
                .find(|n| {
                    self.selected
                        .get(**n)
                        .is_some_and(|s| provides(s.candidate.meta, dep))
                })
                .copied()
        });
        if let Some(name) = satisfied {
            if let Some(sel) = self.selected.get_mut(name) {
                sel.reasons.push(reason);
            }
            return Ok(());
        }

        let unsatisfied = |pass: &Self| -> ErrorKind {
            let yanked_provider = pass.ctx.installed.values().find_map(|state| match state {
                InstalledState::Yanked { candidate, yank } if provides(candidate.meta, dep) => {
                    Some((*candidate, *yank))
                }
                _ => None,
            });
            match yanked_provider {
                Some((candidate, yank)) => yanked(
                    candidate,
                    yank,
                    dep.to_string(),
                    Some(Package::of(by.meta)),
                    pass.chain_selected(&by.meta.name),
                ),
                None => ErrorKind::Unsatisfied {
                    dep: dep.clone(),
                    by: Package::of(by.meta),
                    chain: pass.chain_selected(&by.meta.name),
                    available: Vec::new(),
                },
            }
        };

        let names = self.ctx.provider_names(key);
        let mut usable = Vec::new();
        let mut wrong = Vec::new();
        for name in &names {
            if self.selected.contains_key(*name) {
                wrong.push(*name);
            } else if self.ctx.is_ambiguous(name.as_str())
                || self
                    .ctx
                    .best(
                        name,
                        |c| provides(c.meta, dep) && self.allowed(name, c),
                        true,
                    )?
                    .is_some()
            {
                usable.push(*name);
            }
        }

        if usable.is_empty() {
            let mut options = Vec::new();
            let mut error = None;
            for name in wrong {
                let provider = self
                    .ctx
                    .ordered(name.as_str())?
                    .find(|c| provides(c.meta, dep));
                if let (Some(provider), Some(sel)) = (provider, self.selected.get(name)) {
                    options.push(Fact::Require {
                        name,
                        dep,
                        by: by.meta,
                    });
                    error.get_or_insert_with(|| ErrorKind::WrongProvider {
                        dep: dep.clone(),
                        by: Package::of(by.meta),
                        provider: Package::of(provider.meta),
                        selected: Package::of(sel.candidate.meta),
                        chain: self.chain_selected(name),
                    });
                }
            }
            options.push(Fact::Avoid(by));
            if let Some(error) = error {
                return Err(Stop::failed(error, options));
            }
            for name in &names {
                if self
                    .ctx
                    .ordered(name.as_str())?
                    .any(|c| provides(c.meta, dep))
                {
                    let bound = Bound {
                        dep: dep.clone(),
                        from: BoundSource::Package(Package::of(by.meta)),
                    };
                    return Err(Stop::failed(self.unsatisfiable(name, Some(bound)), options));
                }
            }
            return Err(Stop::failed(unsatisfied(self), options));
        }

        let installed: Vec<&&'a Name> = usable
            .iter()
            .filter(|n| self.ctx.input.installed.contains_key(**n) && self.ctx.prefer(n))
            .collect();
        let pick = if let [one] = installed.as_slice() {
            **one
        } else if let [one] = usable.as_slice() {
            *one
        } else if let Some(default) = self
            .ctx
            .universe
            .provider_defaults
            .get(key.to_string().as_str())
            .filter(|d| usable.contains(d))
        {
            *default
        } else {
            let error = ErrorKind::AmbiguousProvider {
                dep: dep.clone(),
                providers: usable.iter().map(|n| (*n).clone()).collect(),
                by: Package::of(by.meta),
                chain: self.chain_selected(&by.meta.name),
            };
            return Err(Stop::failed(error, vec![Fact::Avoid(by)]));
        };
        let best = self.ctx.best(
            pick,
            |c| provides(c.meta, dep) && self.allowed(pick, c),
            true,
        )?;
        match best {
            Some((candidate, kept)) => {
                self.select(candidate, vec![reason], kept);
                Ok(())
            }
            None => Err(Stop::failed(unsatisfied(self), vec![Fact::Avoid(by)])),
        }
    }

    /// S006 between selected `a` and `b`, avoided by dropping either one.
    fn conflict(&self, a: &Name, b: &Name, replaces: bool) -> Stop<'a> {
        let sel = |n: &Name| {
            self.selected
                .get(n)
                .expect("conflicts are checked between selected packages")
        };
        let error = ErrorKind::Conflict {
            package: Package::of(sel(a).candidate.meta),
            package_chain: self.chain_selected(a),
            other: Package::of(sel(b).candidate.meta),
            other_chain: self.chain_selected(b),
            replaces,
        };
        Stop::failed(
            error,
            vec![Fact::Avoid(sel(a).candidate), Fact::Avoid(sel(b).candidate)],
        )
    }

    fn check_conflicts(&self) -> Result<(), Stop<'a>> {
        let matches = |name: &Name, req: &VersionReq| {
            self.selected
                .get(name)
                .is_some_and(|s| req.matches(&s.candidate.meta.version, s.candidate.meta.rev))
        };
        for (name, sel) in &self.selected {
            let meta = sel.candidate.meta;
            for dep in &meta.conflicts {
                match dep {
                    Dep::Pkg(other, req, _) if other != name && matches(other, req) => {
                        return Err(self.conflict(name, other, false));
                    }
                    Dep::Virtual(virtual_name) => {
                        let key = Key::Virtual(virtual_name.as_str());
                        let others = self.providers.get(&key).into_iter().flatten();
                        if let Some(other) = others.copied().find(|o| *o != name) {
                            return Err(self.conflict(name, other, false));
                        }
                    }
                    _ => {}
                }
            }
            for dep in &meta.replaces {
                if let Dep::Pkg(other, req, _) = dep
                    && other != name
                    && matches(other, req)
                {
                    return Err(self.conflict(name, other, true));
                }
            }
        }
        Ok(())
    }

    /// Chooses the member of every alternatives group present in the view:
    /// the configured one, the only one, the repository default, or the
    /// single member with the highest priority.
    fn alternatives(&self) -> Result<BTreeMap<Name, Name>, Box<ErrorKind>> {
        let mut groups: BTreeMap<&Name, Vec<&Name>> = BTreeMap::new();
        let mut priorities: BTreeMap<(&Name, &Name), i64> = BTreeMap::new();
        for (name, sel) in &self.selected {
            for alternative in &sel.candidate.meta.alternatives {
                groups.entry(&alternative.group).or_default().push(name);
                priorities.insert((&alternative.group, name), alternative.priority);
            }
        }
        let mut chosen = BTreeMap::new();
        for (group, members) in groups {
            let present = || members.iter().map(|n| (*n).clone()).collect();
            let member = match self.ctx.input.alternatives.get(group) {
                Some(choice) if members.contains(&choice) => choice,
                Some(choice) => {
                    return Err(Box::new(ErrorKind::AlternativeAbsent {
                        group: group.clone(),
                        chosen: choice.clone(),
                        present: present(),
                    }));
                }
                None => match (members.as_slice(), self.default_member(group)) {
                    ([one], _) => *one,
                    (_, Some(default)) if members.contains(&default) => default,
                    _ => {
                        let rank = |n: &&Name| priorities[&(group, *n)];
                        let top = members.iter().map(rank).max();
                        let mut highest = members.iter().filter(|n| Some(rank(n)) == top);
                        match (highest.next(), highest.next()) {
                            (Some(one), None) => *one,
                            _ => {
                                return Err(Box::new(ErrorKind::AlternativeAmbiguous {
                                    group: group.clone(),
                                    members: present(),
                                }));
                            }
                        }
                    }
                },
            };
            chosen.insert(group.clone(), member.clone());
        }
        Ok(chosen)
    }

    fn default_member(&self, group: &Name) -> Option<&'a Name> {
        self.ctx
            .universe
            .alternative_defaults
            .get(group.as_str())
            .copied()
    }

    fn warnings(&self) -> Vec<Warning> {
        let mut warnings = Vec::new();
        for state in self.ctx.installed.values() {
            let (InstalledState::Listed(candidate)
            | InstalledState::Extra { candidate, .. }
            | InstalledState::Yanked { candidate, .. }) = state;
            let Some(sel) = self.selected.get(&candidate.meta.name) else {
                continue;
            };
            match state {
                InstalledState::Extra {
                    candidate,
                    orphan: true,
                } if sel.candidate.id == candidate.id => warnings.push(Warning::Orphaned {
                    package: Package::of(candidate.meta),
                }),
                InstalledState::Yanked { candidate, yank } => warnings.push(Warning::Yanked {
                    package: Package::of(candidate.meta),
                    reason: yank.reason.clone(),
                    replacement: Package::of(sel.candidate.meta),
                }),
                _ => {}
            }
        }
        warnings
    }

    fn into_packages(self) -> BTreeMap<Name, Selected<'a>> {
        self.selected
            .into_iter()
            .map(|(name, sel)| {
                (
                    name,
                    Selected {
                        candidate: sel.candidate,
                        reasons: sel.reasons,
                        kept_installed: sel.kept,
                    },
                )
            })
            .collect()
    }

    fn builds(&self) -> Vec<BuildRequest> {
        self.builds
            .iter()
            .map(|(name, build)| {
                debug_assert_eq!(name.as_str(), build.buildable.name.as_str());
                BuildRequest {
                    buildable: build.buildable.clone(),
                    values: build.values.clone(),
                    reasons: build.reasons.clone(),
                }
            })
            .collect()
    }
}

fn yanked(
    candidate: Candidate<'_>,
    yank: &Yank,
    wanted: String,
    by: Option<Package>,
    chain: Chain,
) -> ErrorKind {
    ErrorKind::Yanked {
        package: Package::of(candidate.meta),
        reason: yank.reason.clone(),
        time: yank.time,
        wanted,
        by,
        chain,
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
