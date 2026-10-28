use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use laft_core::{Dep, Name, Provide, StoreId};
use laft_index::{Index, IndexEntry, Yank};
use laft_pkg::Meta;

/// An accepted index of an enabled repository and the repository's
/// priority.
#[derive(Debug, Clone, Copy)]
pub struct Repository<'a> {
    /// The index. Its `repo` field names the repository.
    pub index: &'a Index,
    /// The repository priority. For each package name, only the repository
    /// with the highest priority among those that list the name is used.
    pub priority: i64,
}

/// A package the resolver can select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate<'a> {
    /// The store id.
    pub id: StoreId,
    /// The package metadata.
    pub meta: &'a Meta,
    /// Where the candidate comes from.
    pub source: Source<'a>,
}

/// Where a [`Candidate`] comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source<'a> {
    /// An entry of a repository index.
    Index {
        /// The repository name.
        repo: &'a Name,
        /// The index entry, which says where to download the package.
        entry: &'a IndexEntry,
    },
    /// An installed store entry that no index of the owning repository
    /// lists any more, or whose name no repository lists at all.
    Installed,
}

/// The packages of every enabled repository, prepared for resolving any
/// number of views.
///
/// Each package name belongs to one repository, its owner: the repository
/// with the highest priority among those whose index lists the name, as a
/// package or as a yanked package. Only the owner's packages of that name
/// are candidates, so a high-priority overlay hides every version of a name
/// from the repositories below it. When several repositories share the
/// highest priority for a name, the name is ambiguous, which is an error
/// only when a resolution needs it.
///
/// Repository defaults for providers and alternatives are merged so that the
/// repository with the higher priority wins. Between repositories of equal
/// priority, the one whose name sorts first wins.
///
/// Building a universe sorts every candidate list once. Lookups use hash
/// maps that are never iterated, so the result of a resolution does not
/// depend on the order of repositories or index entries.
///
/// ```
/// use laft_core::Timestamp;
/// use laft_index::{Defaults, Index};
/// use laft_resolve::{Repository, ResolveInput, Universe};
///
/// let index = Index {
///     format: Index::FORMAT,
///     repo: "core".parse()?,
///     channel: "current".parse()?,
///     arch: laft_pkg::Arch::X86_64V3,
///     generation: 1,
///     timestamp: Timestamp::from_secs(0),
///     expires: Timestamp::from_secs(86_400),
///     defaults: Defaults::default(),
///     packages: vec![],
///     yanked: vec![],
/// };
/// let universe = Universe::new(&[Repository { index: &index, priority: 0 }]);
/// let resolution = universe.resolve(&ResolveInput::new("system"))?;
/// assert!(resolution.packages.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Universe<'a> {
    pub(crate) names: HashMap<&'a str, Owner<'a>>,
    pub(crate) provides: HashMap<Key<'a>, Vec<&'a Name>>,
    pub(crate) replaces: HashMap<&'a str, Vec<&'a Name>>,
    pub(crate) provider_defaults: HashMap<&'a str, &'a Name>,
    pub(crate) alternative_defaults: HashMap<&'a str, &'a Name>,
}

/// The candidates of one package name.
#[derive(Debug)]
pub(crate) struct Owner<'a> {
    pub(crate) name: &'a Name,
    /// The owning repositories, sorted. More than one means the name is
    /// ambiguous.
    pub(crate) repos: Vec<&'a Name>,
    pub(crate) priority: i64,
    /// Candidates, best first.
    pub(crate) candidates: Vec<Candidate<'a>>,
    /// Packages of this name that the owner withdrew.
    pub(crate) yanked: Vec<&'a Yank>,
}

/// The text a provide is looked up by: its kind and name, without the
/// version of a `pc:` provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Key<'a> {
    So(&'a str),
    Pc(&'a str),
    Virtual(&'a str),
}

impl<'a> Key<'a> {
    pub(crate) fn of_provide(provide: &'a Provide) -> Self {
        match provide {
            Provide::So(soname) => Self::So(soname.as_str()),
            Provide::Pc(name, _) => Self::Pc(name.as_str()),
            Provide::Virtual(name) => Self::Virtual(name.as_str()),
        }
    }

    /// The key a dependency is satisfied through, or `None` for a
    /// dependency on a package name.
    pub(crate) fn of_dep(dep: &'a Dep) -> Option<Self> {
        match dep {
            Dep::So(soname) => Some(Self::So(soname.as_str())),
            Dep::Pc(name, _) => Some(Self::Pc(name.as_str())),
            Dep::Virtual(name) => Some(Self::Virtual(name.as_str())),
            Dep::Pkg(..) => None,
        }
    }
}

impl fmt::Display for Key<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::So(s) => write!(f, "so:{s}"),
            Self::Pc(s) => write!(f, "pc:{s}"),
            Self::Virtual(s) => write!(f, "virtual:{s}"),
        }
    }
}

/// Whether `meta` provides what `dep` asks for. A `pc:` dependency with
/// constraints needs a provide that states a version within them.
pub(crate) fn provides(meta: &Meta, dep: &Dep) -> bool {
    meta.provides.iter().any(|p| match (dep, p) {
        (Dep::So(a), Provide::So(b)) => a == b,
        (Dep::Virtual(a), Provide::Virtual(b)) => a == b,
        (Dep::Pc(a, constraints), Provide::Pc(b, version)) => {
            a == b
                && (constraints.is_empty()
                    || version
                        .as_ref()
                        .is_some_and(|v| constraints.iter().all(|c| c.matches(v, None))))
        }
        _ => false,
    })
}

/// Best first: newest version and revision, then dynamic before static,
/// then by store id so that the order is total.
pub(crate) fn order(a: &Candidate<'_>, b: &Candidate<'_>) -> Ordering {
    let (x, y) = (a.meta, b.meta);
    y.version
        .cmp(&x.version)
        .then(y.rev.cmp(&x.rev))
        .then(x.variant.cmp(&y.variant))
        .then(a.id.cmp(&b.id))
}

impl<'a> Universe<'a> {
    /// Prepares the packages of `repos` for resolution.
    pub fn new(repos: &[Repository<'a>]) -> Self {
        let mut names: HashMap<&'a str, Owner<'a>> = HashMap::new();
        for repo in repos {
            let listed = repo.index.packages.iter().map(|e| &e.meta.name);
            for name in listed.chain(repo.index.yanked.iter().map(|y| &y.name)) {
                let owner = names.entry(name.as_str()).or_insert_with(|| Owner {
                    name,
                    repos: Vec::new(),
                    priority: repo.priority,
                    candidates: Vec::new(),
                    yanked: Vec::new(),
                });
                if repo.priority > owner.priority {
                    owner.priority = repo.priority;
                    owner.repos.clear();
                }
                if repo.priority == owner.priority && !owner.repos.contains(&&repo.index.repo) {
                    owner.repos.push(&repo.index.repo);
                }
            }
        }
        for repo in repos {
            let owns = |name: &str| {
                names
                    .get(name)
                    .is_some_and(|o| o.repos.contains(&&repo.index.repo))
            };
            let mut found = Vec::new();
            for entry in &repo.index.packages {
                if owns(entry.meta.name.as_str()) {
                    found.push(Candidate {
                        id: entry.id,
                        meta: &entry.meta,
                        source: Source::Index {
                            repo: &repo.index.repo,
                            entry,
                        },
                    });
                }
            }
            let yanked: Vec<_> = repo
                .index
                .yanked
                .iter()
                .filter(|y| owns(y.name.as_str()))
                .collect();
            for candidate in found {
                if let Some(owner) = names.get_mut(candidate.meta.name.as_str()) {
                    owner.candidates.push(candidate);
                }
            }
            for yank in yanked {
                if let Some(owner) = names.get_mut(yank.name.as_str()) {
                    owner.yanked.push(yank);
                }
            }
        }

        let mut provides: HashMap<Key<'a>, Vec<&'a Name>> = HashMap::new();
        let mut replaces: HashMap<&'a str, Vec<&'a Name>> = HashMap::new();
        for owner in names.values_mut() {
            owner.repos.sort();
            owner.candidates.sort_by(order);
            owner.candidates.dedup_by_key(|c| c.id);
            owner.yanked.sort_by_key(|y| y.id);
            for candidate in &owner.candidates {
                let name = &candidate.meta.name;
                for provide in &candidate.meta.provides {
                    provides
                        .entry(Key::of_provide(provide))
                        .or_default()
                        .push(name);
                }
                for dep in &candidate.meta.replaces {
                    if let Dep::Pkg(replaced, _) = dep {
                        replaces.entry(replaced.as_str()).or_default().push(name);
                    }
                }
            }
        }
        for list in provides.values_mut().chain(replaces.values_mut()) {
            list.sort();
            list.dedup();
        }

        let mut ranked: Vec<&Repository<'a>> = repos.iter().collect();
        ranked.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.index.repo.cmp(&b.index.repo))
        });
        let mut provider_defaults = HashMap::new();
        let mut alternative_defaults = HashMap::new();
        for repo in ranked {
            for (provide, name) in &repo.index.defaults.providers {
                provider_defaults.entry(provide.as_str()).or_insert(name);
            }
            for (group, name) in &repo.index.defaults.alternatives {
                alternative_defaults.entry(group.as_str()).or_insert(name);
            }
        }

        Self {
            names,
            provides,
            replaces,
            provider_defaults,
            alternative_defaults,
        }
    }

    pub(crate) fn owner(&self, name: &str) -> Option<&Owner<'a>> {
        self.names.get(name)
    }

    pub(crate) fn providers(&self, key: Key<'a>) -> &[&'a Name] {
        self.provides.get(&key).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn replacers(&self, name: &str) -> &[&'a Name] {
        self.replaces.get(name).map_or(&[], Vec::as_slice)
    }
}
