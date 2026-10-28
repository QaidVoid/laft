use std::collections::{BTreeMap, BTreeSet, VecDeque};

use laft_core::{Dep, Name, PkgRef, StoreId, Variant, VersionPrefix, VersionReq};
use laft_index::Yank;
use laft_pkg::Meta;

use crate::error::{Bound, BoundSource, Error, ErrorKind};
use crate::explain::{Chain, Learned, Package, Reason, Warning, Why};
use crate::universe::{Candidate, Key, Source, Universe, order, provides};

/// The most passes one resolution may take. Every restart adds a constraint
/// that was not there before, so a valid repository never comes close.
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
    /// The constraints learned on restarts, in the order they were learned.
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
    /// Requests are selected first, in name order, then dependencies
    /// breadth first. Without an upgrade, the installed version of a name is
    /// kept whenever it satisfies everything. When a dependency contradicts
    /// an earlier choice that no request fixed, the resolver learns the
    /// dependency's constraint and starts over. Conflicts, replacements and
    /// alternatives are checked on the complete set.
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
        let mut learned: BTreeMap<Name, Vec<Learned>> = BTreeMap::new();
        let mut history = Vec::new();
        for _ in 0..MAX_PASSES {
            let mut pass = Pass::new(&ctx, &requests, &learned);
            match pass.run() {
                Ok(()) => {
                    let alternatives = pass.alternatives().map_err(fail)?;
                    let mut warnings = renames;
                    warnings.extend(pass.warnings());
                    return Ok(Resolution {
                        view: input.view.clone(),
                        packages: pass.into_packages(),
                        alternatives,
                        learned: history,
                        warnings,
                    });
                }
                Err(Stop::Fail(kind)) => return Err(fail(kind)),
                Err(Stop::Restart(l)) => {
                    if learned
                        .get(&l.name)
                        .is_some_and(|list| list.iter().any(|x| x.req == l.req))
                    {
                        return Err(fail(Box::new(pass.unsatisfiable(&l.name, None))));
                    }
                    learned.entry(l.name.clone()).or_default().push(l.clone());
                    history.push(l);
                }
            }
        }
        Err(fail(Box::new(ErrorKind::RestartLimit { learned: history })))
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

    fn prefer(&self, name: &str) -> bool {
        match &self.input.upgrade {
            Upgrade::None => true,
            Upgrade::All => false,
            Upgrade::Names(names) => !names.iter().any(|n| n.as_str() == name),
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

    /// The best candidate of `name` that `accept` takes, and whether it is
    /// the installed one kept by preference. With `dynamic_first`, a
    /// dynamic candidate wins over any static one.
    fn best(
        &self,
        name: &str,
        accept: impl Fn(&Candidate<'a>) -> bool,
        dynamic_first: bool,
    ) -> Result<Option<(Candidate<'a>, bool)>, Box<ErrorKind>> {
        let installed = match self.installed.get(name) {
            Some(InstalledState::Listed(c) | InstalledState::Extra { candidate: c, .. }) => {
                Some(*c)
            }
            _ => None,
        };
        let candidates = self.ordered(name)?;
        if let Some(c) = installed
            && self.prefer(name)
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

    /// Whether `name` has any candidate at all.
    fn has_candidates(&self, name: &str) -> bool {
        self.is_ambiguous(name)
            || self
                .ordered(name)
                .is_ok_and(|mut candidates| candidates.next().is_some())
    }

    /// Sorts the requests, follows renames and merges the requests of each
    /// name.
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
                    [one] => {
                        warnings.push(Warning::Renamed {
                            from: name.clone(),
                            to: (*one).clone(),
                            origin: request.origin.clone(),
                        });
                        target = ((*one).clone(), None);
                    }
                    several => {
                        return Err(Box::new(ErrorKind::NoCandidate {
                            request: request.pkg.to_string(),
                            origin: request.origin.clone(),
                            available: Vec::new(),
                            replaced_by: several.iter().map(|n| (*n).clone()).collect(),
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
enum Stop {
    Restart(Learned),
    Fail(Box<ErrorKind>),
}

impl From<ErrorKind> for Stop {
    fn from(kind: ErrorKind) -> Self {
        Self::Fail(Box::new(kind))
    }
}

impl From<Box<ErrorKind>> for Stop {
    fn from(kind: Box<ErrorKind>) -> Self {
        Self::Fail(kind)
    }
}

struct Sel<'a> {
    candidate: Candidate<'a>,
    reasons: Vec<Reason>,
    /// Whether a request with a version prefix fixed the version.
    forced: bool,
    kept: bool,
}

/// One greedy pass over the requests and their closure.
struct Pass<'p, 'u, 'a> {
    ctx: &'p Ctx<'u, 'a>,
    requests: &'p [Normalized],
    learned: &'p BTreeMap<Name, Vec<Learned>>,
    selected: BTreeMap<Name, Sel<'a>>,
    providers: BTreeMap<Key<'a>, BTreeSet<&'a Name>>,
    queue: VecDeque<&'a Name>,
}

impl<'p, 'u, 'a> Pass<'p, 'u, 'a> {
    fn new(
        ctx: &'p Ctx<'u, 'a>,
        requests: &'p [Normalized],
        learned: &'p BTreeMap<Name, Vec<Learned>>,
    ) -> Self {
        Self {
            ctx,
            requests,
            learned,
            selected: BTreeMap::new(),
            providers: BTreeMap::new(),
            queue: VecDeque::new(),
        }
    }

    fn learned_ok(&self, name: &Name, candidate: &Candidate<'_>) -> bool {
        self.learned.get(name).is_none_or(|list| {
            list.iter()
                .all(|l| l.req.matches(&candidate.meta.version, candidate.meta.rev))
        })
    }

    fn select(&mut self, candidate: Candidate<'a>, reasons: Vec<Reason>, forced: bool, kept: bool) {
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
                forced,
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

    fn run(&mut self) -> Result<(), Stop> {
        for request in self.requests {
            let accept = |c: &Candidate<'a>| {
                c.meta.variant == request.variant
                    && request
                        .prefix
                        .as_ref()
                        .is_none_or(|p| p.matches(&c.meta.version))
                    && self.learned_ok(&request.name, c)
            };
            match self.ctx.best(request.name.as_str(), accept, false)? {
                Some((candidate, kept)) => self.select(
                    candidate,
                    request.reasons.clone(),
                    request.prefix.is_some(),
                    kept,
                ),
                None => return Err(self.no_candidate(request)?.into()),
            }
        }
        while let Some(name) = self.queue.pop_front() {
            let Some(meta) = self.selected.get(name).map(|s| s.candidate.meta) else {
                continue;
            };
            for dep in &meta.depends {
                self.satisfy(dep, meta)?;
            }
        }
        self.check_conflicts()?;
        Ok(())
    }

    fn no_candidate(&self, request: &Normalized) -> Result<ErrorKind, Box<ErrorKind>> {
        let name = &request.name;
        let matching = self.ctx.ordered(name.as_str())?.any(|c| {
            c.meta.variant == request.variant
                && request
                    .prefix
                    .as_ref()
                    .is_none_or(|p| p.matches(&c.meta.version))
        });
        if matching {
            return Ok(self.unsatisfiable(name, None));
        }
        if let Some((candidate, yank)) = self.ctx.yanked(name.as_str()) {
            return Ok(yanked(
                candidate,
                yank,
                request.text.clone(),
                None,
                Chain {
                    steps: Vec::new(),
                    origin: request.origin.clone(),
                    request: request.text.clone(),
                },
            ));
        }
        Ok(ErrorKind::NoCandidate {
            request: request.text.clone(),
            origin: request.origin.clone(),
            available: self.ctx.available(name.as_str()),
            replaced_by: Vec::new(),
        })
    }

    /// R005 for `name`: every learned constraint, `extra`, and the request's
    /// prefix.
    fn unsatisfiable(&self, name: &Name, extra: Option<Bound>) -> ErrorKind {
        let mut constraints: Vec<Bound> = self
            .learned
            .get(name)
            .into_iter()
            .flatten()
            .map(|l| Bound {
                dep: l.dep(),
                from: BoundSource::Package(l.by.clone()),
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

    fn satisfy(&mut self, dep: &'a Dep, by: &'a Meta) -> Result<(), Stop> {
        let reason = Reason::Dependency {
            by: by.name.clone(),
            dep: dep.clone(),
        };
        match (dep, Key::of_dep(dep)) {
            (Dep::Pkg(name, req), _) => self.satisfy_name(dep, name, req, by, reason),
            (_, Some(key)) => self.satisfy_provide(dep, key, by, reason),
            (_, None) => Ok(()),
        }
    }

    fn satisfy_name(
        &mut self,
        dep: &'a Dep,
        name: &'a Name,
        req: &'a VersionReq,
        by: &'a Meta,
        reason: Reason,
    ) -> Result<(), Stop> {
        if let Some(sel) = self.selected.get_mut(name) {
            let meta = sel.candidate.meta;
            if req.matches(&meta.version, meta.rev) {
                sel.reasons.push(reason);
                return Ok(());
            }
            if sel.forced {
                let bound = Bound {
                    dep: dep.clone(),
                    from: BoundSource::Package(Package::of(by)),
                };
                return Err(self.unsatisfiable(name, Some(bound)).into());
            }
            return Err(Stop::Restart(Learned {
                name: name.clone(),
                req: req.clone(),
                by: Package::of(by),
            }));
        }
        let accept = |c: &Candidate<'a>| {
            req.matches(&c.meta.version, c.meta.rev) && self.learned_ok(name, c)
        };
        if let Some((candidate, kept)) = self.ctx.best(name.as_str(), accept, true)? {
            self.select(candidate, vec![reason], false, kept);
            return Ok(());
        }
        if self
            .ctx
            .ordered(name.as_str())?
            .any(|c| req.matches(&c.meta.version, c.meta.rev))
        {
            let bound = Bound {
                dep: dep.clone(),
                from: BoundSource::Package(Package::of(by)),
            };
            return Err(self.unsatisfiable(name, Some(bound)).into());
        }
        if let Some((candidate, yank)) = self.ctx.yanked(name.as_str()) {
            return Err(yanked(
                candidate,
                yank,
                dep.to_string(),
                Some(Package::of(by)),
                self.chain(&by.name),
            )
            .into());
        }
        Err(ErrorKind::Unsatisfied {
            dep: dep.clone(),
            by: Package::of(by),
            chain: self.chain(&by.name),
            available: self.ctx.available(name.as_str()),
        }
        .into())
    }

    fn satisfy_provide(
        &mut self,
        dep: &'a Dep,
        key: Key<'a>,
        by: &'a Meta,
        reason: Reason,
    ) -> Result<(), Stop> {
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
                    Some(Package::of(by)),
                    pass.chain(&by.name),
                ),
                None => ErrorKind::Unsatisfied {
                    dep: dep.clone(),
                    by: Package::of(by),
                    chain: pass.chain(&by.name),
                    available: Vec::new(),
                },
            }
        };

        let names = self.ctx.provider_names(key);
        let mut usable = Vec::new();
        let mut wrong = None;
        for name in &names {
            if self.selected.contains_key(*name) {
                wrong.get_or_insert(*name);
            } else if self.ctx.is_ambiguous(name.as_str())
                || self
                    .ctx
                    .best(
                        name.as_str(),
                        |c| provides(c.meta, dep) && self.learned_ok(name, c),
                        true,
                    )?
                    .is_some()
            {
                usable.push(*name);
            }
        }

        if usable.is_empty() {
            if let Some(name) = wrong {
                let provider = self
                    .ctx
                    .ordered(name.as_str())?
                    .find(|c| provides(c.meta, dep));
                if let (Some(provider), Some(sel)) = (provider, self.selected.get(name)) {
                    return Err(ErrorKind::WrongProvider {
                        dep: dep.clone(),
                        by: Package::of(by),
                        provider: Package::of(provider.meta),
                        selected: Package::of(sel.candidate.meta),
                        chain: self.chain(name),
                    }
                    .into());
                }
            }
            for name in &names {
                if self
                    .ctx
                    .ordered(name.as_str())?
                    .any(|c| provides(c.meta, dep))
                {
                    return Err(self.unsatisfiable(name, None).into());
                }
            }
            return Err(unsatisfied(self).into());
        }

        let installed: Vec<&&'a Name> = usable
            .iter()
            .filter(|n| self.ctx.input.installed.contains_key(**n) && self.ctx.prefer(n.as_str()))
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
            return Err(ErrorKind::AmbiguousProvider {
                dep: dep.clone(),
                providers: usable.iter().map(|n| (*n).clone()).collect(),
                by: Package::of(by),
                chain: self.chain(&by.name),
            }
            .into());
        };
        let best = self.ctx.best(
            pick.as_str(),
            |c| provides(c.meta, dep) && self.learned_ok(pick, c),
            true,
        )?;
        match best {
            Some((candidate, kept)) => {
                self.select(candidate, vec![reason], false, kept);
                Ok(())
            }
            None => Err(unsatisfied(self).into()),
        }
    }

    fn conflict(&self, a: &Name, b: &Name, replaces: bool) -> Stop {
        let label = |n: &Name| {
            self.selected
                .get(n)
                .map(|s| Package::of(s.candidate.meta))
                .expect("conflicts are checked between selected packages")
        };
        Stop::from(ErrorKind::Conflict {
            package: label(a),
            package_chain: self.chain(a),
            other: label(b),
            other_chain: self.chain(b),
            replaces,
        })
    }

    fn check_conflicts(&self) -> Result<(), Stop> {
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
    /// the configured one, the only one, or the repository default.
    fn alternatives(&self) -> Result<BTreeMap<Name, Name>, Box<ErrorKind>> {
        let mut groups: BTreeMap<&Name, Vec<&Name>> = BTreeMap::new();
        for (name, sel) in &self.selected {
            for alternative in &sel.candidate.meta.alternatives {
                groups.entry(&alternative.group).or_default().push(name);
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
                        return Err(Box::new(ErrorKind::AlternativeAmbiguous {
                            group: group.clone(),
                            members: present(),
                        }));
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
