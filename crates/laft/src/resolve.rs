//! Turning the configuration into resolver inputs and resolving every view.

use std::collections::BTreeMap;

use laft_config::SystemConfig;
use laft_core::{Name, PkgRef, StoreId, Variant};
use laft_pkg::Meta;
use laft_resolve::{
    Buildable, Installed, OptionRequest, Repository, Request, Resolution, ResolveInput, Universe,
    Upgrade,
};
use laft_store::{Generation, Root};
use laft_svc::ServiceSet;

use crate::env::RepoIndex;
use crate::failure::Failure;

/// The view name of the system view.
pub(crate) const SYSTEM: &str = "system";

/// The name the initrd closure is resolved under.
pub(crate) const INITRD: &str = "initrd";

/// The metadata of installed packages, read from the store, by view and
/// package name.
#[derive(Debug, Default)]
pub(crate) struct InstalledMetas {
    views: BTreeMap<String, BTreeMap<Name, (StoreId, Meta)>>,
}

impl InstalledMetas {
    /// The installed packages of the system view and the declared views of
    /// `generation`.
    pub(crate) fn of(root: &Root, generation: Option<&Generation>) -> Self {
        let mut views = BTreeMap::new();
        let Some(generation) = generation else {
            return Self { views };
        };
        let store = root.store();
        let read = |records: &[laft_store::PackageRecord]| {
            records
                .iter()
                .filter_map(|p| Some((p.name.clone(), (p.id, store.meta(&p.dir).ok()?))))
                .collect::<BTreeMap<_, _>>()
        };
        views.insert(SYSTEM.to_owned(), read(&generation.packages));
        views.insert(INITRD.to_owned(), read(&generation.initrd_packages));
        for (name, version) in &generation.views {
            if let Ok(view) = root.view(name, *version) {
                views.insert(name.to_string(), read(&view.packages));
            }
        }
        Self { views }
    }

    fn view(&self, view: &str) -> BTreeMap<Name, Installed<'_>> {
        self.views
            .get(view)
            .map(|packages| {
                packages
                    .iter()
                    .map(|(name, (id, meta))| (name.clone(), Installed { id: *id, meta }))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The resolution of the system view and of every declared view.
#[derive(Debug)]
pub(crate) struct Resolved<'a> {
    /// The system view.
    pub(crate) system: Resolution<'a>,
    /// The initrd closure, which belongs to no view.
    pub(crate) initrd: Resolution<'a>,
    /// The declared views, by name.
    pub(crate) views: BTreeMap<Name, Resolution<'a>>,
}

/// The universe of the accepted indexes.
pub(crate) fn universe(indexes: &[RepoIndex]) -> Universe<'_> {
    let repos: Vec<Repository<'_>> = indexes
        .iter()
        .map(|r| Repository {
            index: &r.accepted.index,
            priority: r.repo.priority,
        })
        .collect();
    Universe::new(&repos)
}

/// The packages the accepted indexes can build from their recipe
/// snapshots, one per name.
///
/// Only a repository that publishes a snapshot contributes. The newest
/// entry wins for a name; a tie goes to the repository with the higher
/// priority, then to the greater name, as the option declarations do. The
/// declared defaults of the winning repository are the values a build
/// starts from.
pub(crate) fn buildables(indexes: &[RepoIndex]) -> Vec<Buildable> {
    let mut by_priority: Vec<&RepoIndex> = indexes
        .iter()
        .filter(|repo| repo.accepted.index.recipes.is_some())
        .collect();
    by_priority.sort_by_key(|repo| (repo.repo.priority, repo.name.clone()));
    let mut newest: BTreeMap<Name, (&laft_index::IndexEntry, &RepoIndex)> = BTreeMap::new();
    for repo in by_priority {
        for entry in &repo.accepted.index.packages {
            let rank =
                |e: &laft_index::IndexEntry| (e.meta.version.clone(), e.meta.rev, e.meta.variant);
            match newest.entry(entry.meta.name.clone()) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert((entry, repo));
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    if rank(entry) >= rank(slot.get().0) {
                        slot.insert((entry, repo));
                    }
                }
            }
        }
    }
    newest
        .into_iter()
        .map(|(name, (entry, repo))| {
            let defaults = repo
                .accepted
                .index
                .options
                .get(&name)
                .map(|decls| {
                    decls
                        .iter()
                        .map(|decl| (decl.name.clone(), decl.default.clone()))
                        .collect()
                })
                .unwrap_or_default();
            Buildable {
                name,
                version: entry.meta.version.clone(),
                rev: entry.meta.rev,
                variant: entry.meta.variant,
                defaults,
            }
        })
        .collect()
}

/// The fixed and preferred values of one view, as the resolver asks for
/// them: by package name.
pub(crate) fn option_request(values: &laft_config::OptionValues) -> BTreeMap<Name, OptionRequest> {
    values
        .fixed
        .iter()
        .map(|(pkg, opts)| {
            (
                pkg.clone(),
                OptionRequest {
                    fixed: opts
                        .iter()
                        .map(|(opt, setting)| {
                            (
                                opt.clone(),
                                laft_resolve::FixedValue {
                                    value: setting.value.clone(),
                                    origin: setting.origin.clone(),
                                },
                            )
                        })
                        .collect(),
                    preferred: values.preferred.get(pkg).cloned().unwrap_or_default(),
                },
            )
        })
        .collect()
}

/// The services of `config`, validated.
pub(crate) fn services(config: &SystemConfig) -> Result<ServiceSet, Failure> {
    ServiceSet::new(&config.services).map_err(|errors| {
        let mut failure = Failure::general("the services are invalid");
        for error in errors {
            failure = failure.context(error.to_string());
        }
        failure
    })
}

/// The requests of the system view: the configured packages, the kernel
/// and the packages of enabled services.
///
/// # Panics
///
/// Panics when `boot.kernel` is not a package reference, which a loaded
/// configuration never holds.
pub(crate) fn system_requests(config: &SystemConfig, services: &ServiceSet) -> Vec<Request> {
    let mut requests = config.packages.clone();
    let mut add = |pkg: PkgRef, origin: &str| {
        if !requests.iter().any(|r| r.pkg.name() == pkg.name()) {
            requests.push(Request {
                pkg,
                origin: origin.to_owned(),
            });
        }
    };
    let kernel = config
        .boot
        .boot
        .kernel
        .parse::<PkgRef>()
        .expect("boot.kernel has the type Pkg, which loading the configuration checks");
    add(kernel, "boot.kernel");
    for name in services.packages() {
        add(
            PkgRef::new(name.clone(), None, Variant::default()),
            "services",
        );
    }
    requests
}

/// The requests and alternatives of view `name`, after inheritance from
/// the system's `system` requests.
pub(crate) fn view_requests(
    config: &SystemConfig,
    name: &Name,
    system: &[Request],
) -> (Vec<Request>, BTreeMap<Name, Name>) {
    let Some(view) = config.views.get(name) else {
        return (Vec::new(), BTreeMap::new());
    };
    if !view.inherit {
        return (view.packages.clone(), view.alternatives.clone());
    }
    let mut requests: Vec<Request> = system
        .iter()
        .filter(|r| !view.packages.iter().any(|v| v.pkg.name() == r.pkg.name()))
        .cloned()
        .collect();
    requests.extend(view.packages.iter().cloned());
    let mut alternatives = config.alternatives.clone();
    alternatives.extend(view.alternatives.clone());
    (requests, alternatives)
}

/// Resolves the system view and every declared view of `config`.
/// What the views resolve with besides their requests.
pub(crate) struct ViewContext<'a> {
    /// The fixed and preferred option values, by view name, the system
    /// view included.
    pub(crate) options: &'a BTreeMap<Name, laft_config::OptionValues>,
    /// The packages a recipe snapshot can build.
    pub(crate) buildable: &'a [Buildable],
    /// Whether the views may select a package that a recipe snapshot
    /// builds after the resolution.
    pub(crate) allow_builds: bool,
}

/// Resolves the system view and every declared view of `config`.
///
/// Every view resolves with its fixed and preferred option values from
/// `context`. When `allow_builds` is false, selecting a package that
/// `buildable` can build fails with S011, and when it is true, the
/// resolution carries the build requests to run.
pub(crate) fn resolve<'a>(
    universe: &Universe<'a>,
    config: &SystemConfig,
    context: &ViewContext<'_>,
    services: &ServiceSet,
    installed: &'a InstalledMetas,
    upgrade: &Upgrade,
    host: &laft_activate::Host,
) -> Result<Resolved<'a>, Failure> {
    let ViewContext {
        options,
        buildable,
        allow_builds,
    } = *context;
    let requests = system_requests(config, services);
    let mut input = ResolveInput::new(SYSTEM);
    input.requests = requests.clone();
    input.alternatives = config.alternatives.clone();
    input.installed = installed.view(SYSTEM);
    input.upgrade = upgrade.clone();
    input.options = options
        .get(&SYSTEM.parse().expect("a valid name"))
        .map(option_request)
        .unwrap_or_default();
    input.buildable = buildable.to_vec();
    input.allow_builds = allow_builds;
    let system = universe.resolve(&input)?;
    let mut input = ResolveInput::new(INITRD);
    input.requests = laft_activate::initrd_packages(&config.boot, host)
        .into_iter()
        .map(|p| Request {
            pkg: p.pkg,
            origin: p.origin,
        })
        .collect();
    input.installed = installed.view(INITRD);
    input.upgrade = upgrade.clone();
    let initrd = universe.resolve(&input)?;
    let mut views = BTreeMap::new();
    for name in config.views.keys() {
        let (requests, alternatives) = view_requests(config, name, &requests);
        let mut input = ResolveInput::new(name.as_str());
        input.requests = requests;
        input.alternatives = alternatives;
        input.installed = installed.view(name.as_str());
        input.upgrade = upgrade.clone();
        input.options = options.get(name).map(option_request).unwrap_or_default();
        input.buildable = buildable.to_vec();
        input.allow_builds = allow_builds;
        views.insert(name.clone(), universe.resolve(&input)?);
    }
    Ok(Resolved {
        system,
        initrd,
        views,
    })
}

/// The store reasons of a resolver's reasons.
pub(crate) fn reasons(reasons: &[laft_resolve::Reason]) -> Vec<laft_store::Reason> {
    reasons
        .iter()
        .map(|reason| match reason {
            laft_resolve::Reason::Requested { origin, text } => laft_store::Reason::Requested {
                origin: origin.clone(),
                text: text.clone(),
            },
            laft_resolve::Reason::Dependency { by, dep } => laft_store::Reason::Dependency {
                by: by.clone(),
                dep: dep.to_string(),
            },
        })
        .collect()
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
