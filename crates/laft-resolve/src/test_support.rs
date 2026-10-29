//! Packages, indexes and requests for tests.

use std::collections::BTreeMap;

use laft_core::{Blake3Hash, Name, StoreId, Timestamp};
use laft_index::{Defaults, Index, IndexEntry, Yank};
use laft_pkg::{Alternative, Arch, Meta};

use crate::{Error, Installed, Repository, Request, Resolution, ResolveInput, Universe, Upgrade};

pub(crate) fn name(s: &str) -> Name {
    s.parse().expect("test name is valid")
}

/// Package metadata from `"<name> <version>[-r<rev>] [static]"`.
pub(crate) fn pkg(text: &str) -> Meta {
    let mut parts = text.split_whitespace();
    let name = parts.next().expect("the text starts with the name");
    let version = parts.next().expect("the text has a version");
    let variant = parts.next().map_or(Default::default(), |v| {
        v.parse().expect("test variant is valid")
    });
    let (version, rev) = match version.rsplit_once("-r") {
        Some((v, r)) => (v, r.parse().expect("test revision is a number")),
        None => (version, 1),
    };
    Meta {
        format: laft_pkg::FORMAT,
        name: self::name(name),
        version: version.parse().expect("test version is valid"),
        rev,
        variant,
        arch: Arch::X86_64V3,
        origin: self::name(name),
        summary: String::new(),
        homepage: None,
        license: vec![],
        depends: vec![],
        provides: vec![],
        conflicts: vec![],
        replaces: vec![],
        alternatives: vec![],
        triggers: vec![],
        static_deps: vec![],
        abi: BTreeMap::new(),
        abi_epoch: 0,
        requested_privileges: vec![],
        installed_size: 0,
    }
}

/// Builder methods that keep every list sorted, as metadata requires.
pub(crate) trait MetaExt: Sized {
    fn depends(self, deps: &[&str]) -> Self;
    fn provides(self, provides: &[&str]) -> Self;
    fn conflicts(self, deps: &[&str]) -> Self;
    fn replaces(self, deps: &[&str]) -> Self;
    fn alternative(self, group: &str, paths: &[&str]) -> Self;
}

fn parsed<T: std::str::FromStr + ToString>(items: &[&str]) -> Vec<T>
where
    T::Err: std::fmt::Debug,
{
    let mut list: Vec<T> = items
        .iter()
        .map(|s| s.parse().expect("test dependency is valid"))
        .collect();
    list.sort_by_key(ToString::to_string);
    list.dedup_by_key(|d| d.to_string());
    list
}

impl MetaExt for Meta {
    fn depends(mut self, deps: &[&str]) -> Self {
        self.depends = parsed(deps);
        self
    }

    fn provides(mut self, provides: &[&str]) -> Self {
        self.provides = parsed(provides);
        self
    }

    fn conflicts(mut self, deps: &[&str]) -> Self {
        self.conflicts = parsed(deps);
        self
    }

    fn replaces(mut self, deps: &[&str]) -> Self {
        self.replaces = parsed(deps);
        self
    }

    fn alternative(mut self, group: &str, paths: &[&str]) -> Self {
        self.alternatives.push(Alternative {
            group: name(group),
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
        });
        self
    }
}

/// A content address derived from the whole metadata, so equal metadata
/// gets the same id in every index and in the installed set.
pub(crate) fn id(meta: &Meta) -> StoreId {
    StoreId::from_manifest_hash(&Blake3Hash::of(format!("{meta:?}").as_bytes()))
}

pub(crate) fn entry(meta: Meta) -> IndexEntry {
    let id = id(&meta);
    IndexEntry {
        meta,
        id,
        file: format!("pool/{id}.lpkg"),
        size: 0,
        blake3: Blake3Hash::of(b""),
    }
}

pub(crate) fn index(repo: &str, packages: Vec<Meta>) -> Index {
    Index {
        format: Index::FORMAT,
        repo: name(repo),
        channel: name("current"),
        arch: Arch::X86_64V3,
        generation: 1,
        timestamp: Timestamp::from_secs(0),
        expires: Timestamp::from_secs(86_400),
        defaults: Defaults::default(),
        packages: packages.into_iter().map(entry).collect(),
        yanked: vec![],
    }
}

/// A yank record for `meta`, made on 2026-10-27.
pub(crate) fn yank(meta: &Meta, reason: &str) -> Yank {
    Yank {
        id: id(meta),
        name: meta.name.clone(),
        version: meta.version.clone(),
        rev: meta.rev,
        variant: meta.variant,
        reason: reason.to_owned(),
        time: "2026-10-27T09:00:00Z".parse().expect("valid timestamp"),
    }
}

/// A request whose origin is `test:<text>`.
pub(crate) fn req(text: &str) -> Request {
    req_at(text, &format!("test:{text}"))
}

pub(crate) fn req_at(text: &str, origin: &str) -> Request {
    Request {
        pkg: text.parse().expect("test request is valid"),
        origin: origin.to_owned(),
    }
}

pub(crate) fn universe<'a>(repos: &[(&'a Index, i64)]) -> Universe<'a> {
    let repos: Vec<Repository<'a>> = repos
        .iter()
        .map(|(index, priority)| Repository {
            index,
            priority: *priority,
        })
        .collect();
    Universe::new(&repos)
}

pub(crate) fn input<'a>(
    view: &str,
    requests: Vec<Request>,
    installed: &[&'a Meta],
) -> ResolveInput<'a> {
    ResolveInput {
        view: view.to_owned(),
        requests,
        alternatives: BTreeMap::new(),
        installed: installed
            .iter()
            .map(|meta| (meta.name.clone(), Installed { id: id(meta), meta }))
            .collect(),
        upgrade: Upgrade::None,
    }
}

/// Resolves `requests` for view `system` against one repository `core`.
pub(crate) fn resolve_core<'a>(
    core: &'a Index,
    requests: &[&str],
    installed: &[&'a Meta],
) -> Result<Resolution<'a>, Error> {
    universe(&[(core, 0)]).resolve(&input(
        "system",
        requests.iter().map(|r| req(r)).collect(),
        installed,
    ))
}

/// The selected packages as labels, in name order.
pub(crate) fn labels(resolution: &Resolution<'_>) -> Vec<String> {
    resolution
        .packages
        .values()
        .map(|s| crate::Package::of(s.candidate.meta).to_string())
        .collect()
}

/// A small deterministic generator for shuffling and synthetic data.
pub(crate) struct Seeded(u64);

impl Seeded {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

/// A resolution exists, but each pass learns only one of the 1001 bounds
/// that keep every `libN` below version 2, so the search runs out of passes.
pub(crate) fn one_bound_per_pass_universe() -> Index {
    let mut packages = Vec::new();
    let mut deps = Vec::new();
    for i in 1..=1001 {
        let (lib, user) = (format!("lib{i:04}"), format!("user{i:04}"));
        packages.push(pkg(&format!("{lib} 1")));
        packages.push(pkg(&format!("{lib} 2")));
        packages.push(pkg(&format!("{user} 1")).depends(&[&format!("{lib}<2")]));
        deps.extend([lib, user]);
    }
    let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
    packages.push(pkg("app 1").depends(&deps));
    index("core", packages)
}
