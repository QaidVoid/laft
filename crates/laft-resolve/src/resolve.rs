use std::collections::{BTreeMap, BTreeSet, VecDeque};

use laft_core::{Dep, Name, PkgRef, StoreId, Variant, VersionPrefix, VersionReq};
use laft_index::Yank;
use laft_pkg::Meta;

use crate::error::{Bound, BoundSource, Error, ErrorKind};
use crate::explain::{Chain, Learned, Package, Reason, Warning, Why};
use crate::universe::{Candidate, Key, Source, Universe, order, provides};

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
    /// reports the first failure it met, or R009 if it met none.
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
    /// # };
    /// let universe = Universe::new(&[Repository { index: &index, priority: 0 }]);
    /// let mut input = ResolveInput::new("system");
    /// input.requests.push(Request {
    ///     pkg: "ripgrep".parse()?,
    ///     origin: "/etc/laft/system.stav:8:22".into(),
    /// });
    /// let error = universe.resolve(&input).unwrap_err();
    /// assert_eq!(error.code(), Some("R001"));
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
                    return Ok(Resolution {
                        view: input.view.clone(),
                        packages: pass.into_packages(),
                        alternatives,
                        learned: path.iter().map(Fact::learned).collect(),
                        warnings,
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

/// Whether `meta` satisfies `dep`: by its version for a dependency on its
/// own name, by replacing the name for a dependency on another name, and by
/// its provides otherwise.
fn satisfies(meta: &Meta, dep: &Dep) -> bool {
    match dep {
        Dep::Pkg(name, req) if *name == meta.name => req.matches(&meta.version, meta.rev),
        Dep::Pkg(name, _) => meta
            .replaces
            .iter()
            .any(|r| matches!(r, Dep::Pkg(replaced, _) if replaced == name)),
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
}

impl<'u, 'a> Ctx<'u, 'a> {
    fn new(universe: &'u Universe<'a>, input: &'u ResolveInput<'a>) -> Self {
        let mut installed = BTreeMap::new();
        let mut extra_provides: BTreeMap<Key<'a>, Vec<&'a Name>> = BTreeMap::new();
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
        Self {
            universe,
            input,
            installed,
            extra_provides,
        }
    }

    fn prefer(&self, name: &Name) -> bool {
        match &self.input.upgrade {
            Upgrade::None => true,
            Upgrade::All => false,
            Upgrade::Names(names) => !names.contains(name),
        }
    }

    /// The candidates of `name`: the owner's, best first, and a visible
    /// installed candidate that the owner does not list.
    fn candidates(
        &self,
        name: &str,
    ) -> Result<(&'u [Candidate<'a>], Option<Candidate<'a>>), Box<ErrorKind>> {
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
    fn ordered(
        &self,
        name: &str,
    ) -> Result<impl Iterator<Item = Candidate<'a>> + 'u, Box<ErrorKind>> {
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
    /// owner, or an installed package the owner pruned or no repository
    /// lists. An ambiguous name counts, because several repositories list
    /// it. A yanked installed package does not count.
    fn has_candidates(&self, name: &str) -> bool {
        self.is_ambiguous(name)
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
            normalized.push(Normalized {
                name,
                variant,
                prefix: prefix.cloned(),
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

/// One greedy pass over the requests and their closure.
struct Pass<'p, 'u, 'a> {
    ctx: &'p Ctx<'u, 'a>,
    requests: &'p [Normalized],
    /// The constraints of the branch, in the order they were learned.
    facts: &'p [Fact<'a>],
    required: BTreeMap<&'a Name, Vec<&'a Dep>>,
    avoided: BTreeSet<StoreId>,
    selected: BTreeMap<Name, Sel<'a>>,
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
            providers: BTreeMap::new(),
            queue: VecDeque::new(),
        }
    }

    /// Whether the constraints of the branch allow `candidate` of `name`.
    fn allowed(&self, name: &Name, candidate: &Candidate<'_>) -> bool {
        !self.avoided.contains(&candidate.id)
            && self
                .required
                .get(name)
                .is_none_or(|deps| deps.iter().all(|dep| satisfies(candidate.meta, dep)))
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

    fn chain(&self, name: &Name) -> Chain {
        Chain::find(name, |n| {
            self.selected
                .get(n)
                .map(|s| (Package::of(s.candidate.meta), s.reasons.as_slice()))
        })
        .expect("every selected package traces back to a request")
    }

    fn run(&mut self) -> Result<(), Stop<'a>> {
        for request in self.requests {
            let accept = |c: &Candidate<'a>| {
                c.meta.variant == request.variant
                    && request
                        .prefix
                        .as_ref()
                        .is_none_or(|p| p.matches(&c.meta.version))
                    && self.allowed(&request.name, c)
            };
            match self.ctx.best(&request.name, accept, false)? {
                Some((candidate, kept)) => self.select(candidate, request.reasons.clone(), kept),
                None => return Err(self.no_candidate(request)?),
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

    /// R005 for `name`: every constraint the branch requires of it, `extra`,
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
            && let Some(prefix) = &request.prefix
        {
            constraints.push(Bound {
                dep: Dep::Pkg(name.clone(), VersionReq::Prefix(prefix.clone())),
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
            (Dep::Pkg(name, req), _) => self.satisfy_name(dep, name, req, by, reason),
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
        by: Candidate<'a>,
        reason: Reason,
    ) -> Result<(), Stop<'a>> {
        if let Some(sel) = self.selected.get_mut(name) {
            let meta = sel.candidate.meta;
            if req.matches(&meta.version, meta.rev) {
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
        let accept =
            |c: &Candidate<'a>| req.matches(&c.meta.version, c.meta.rev) && self.allowed(name, c);
        if let Some((candidate, kept)) = self.ctx.best(name, accept, true)? {
            self.select(candidate, vec![reason], kept);
            return Ok(());
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
                self.chain(&by.meta.name),
            )
        } else {
            ErrorKind::Unsatisfied {
                dep: dep.clone(),
                by: Package::of(by.meta),
                chain: self.chain(&by.meta.name),
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
            chain: self.chain(&by.meta.name),
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
                    pass.chain(&by.meta.name),
                ),
                None => ErrorKind::Unsatisfied {
                    dep: dep.clone(),
                    by: Package::of(by.meta),
                    chain: pass.chain(&by.meta.name),
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
                        chain: self.chain(name),
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
                chain: self.chain(&by.meta.name),
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

    /// R006 between selected `a` and `b`, avoided by dropping either one.
    fn conflict(&self, a: &Name, b: &Name, replaces: bool) -> Stop<'a> {
        let sel = |n: &Name| {
            self.selected
                .get(n)
                .expect("conflicts are checked between selected packages")
        };
        let error = ErrorKind::Conflict {
            package: Package::of(sel(a).candidate.meta),
            package_chain: self.chain(a),
            other: Package::of(sel(b).candidate.meta),
            other_chain: self.chain(b),
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
                    Dep::Pkg(other, req) if other != name && matches(other, req) => {
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
                if let Dep::Pkg(other, req) = dep
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
