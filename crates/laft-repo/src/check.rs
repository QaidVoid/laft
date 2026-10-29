//! Repository-wide checks of a candidate index.
//!
//! The checks run over an index together with the package files it lists,
//! before the index is signed. Each finding has a code, a level and a
//! message that names the packages and paths involved. An error blocks
//! publishing. A warning does not.
//!
//! | Code | Level | Check |
//! |---|---|---|
//! | KR001 | error | Two packages that can share a view own the same non-directory path outside a common alternatives group, or disagree on whether a path is a directory. |
//! | KR002 | error | A dependency that no package of the index or of a base index satisfies. |
//! | KR003 | error | A package that the resolver cannot install on its own. |
//! | KR004 | error | An alternatives group without a valid default, whose members ship different paths, or that lists a path its member does not ship. |
//! | KR005 | error | A package whose name, version, revision and variant are already published with another store id. |
//! | KR006 | warning | The newest version of a name is older than the newest one previously published. |
//! | KR007 | error | Packages of several names that can share a view provide the same `so:` or `virtual:` value, and no provider default chooses one of them. |
//! | KR008 | error | A package file whose metadata or manifest is invalid. |
//! | KR009 | error | A package file that is missing or differs in size or hash from its entry. |
//! | KR010 | error | An entry whose metadata or store id differs from its package file. |
//! | KR011 | error | A payload that ships a file a trigger generates. |
//! | CI001 | error | A package built from a dirty recipe tree or by a bootstrap build, unless allowed. |
//! | CI003 | error | A package without readable buildinfo. |
//!
//! Two packages can share a view when their names differ and neither
//! conflicts with or replaces the other.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;

use laft_core::{Dep, Name, PkgRef, Provide, StoreId, TRIGGERS, Variant, Version, VersionReq};
use laft_index::{Index, IndexEntry};
use laft_pkg::{Entry, Manifest, Meta, Reader};
use laft_resolve::{Request, ResolveInput, Universe};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fsx;
use crate::layout::Repository;
use crate::package::Buildinfo;

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Level {
    /// Blocks publishing.
    Error,
    /// Reported only.
    Warning,
}

/// One problem found by a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Finding {
    /// The check code, for example `KR001`.
    pub(crate) code: String,
    /// How serious it is.
    pub(crate) level: Level,
    /// One line that says what is wrong.
    pub(crate) message: String,
    /// Further lines: who is involved and how to fix it.
    pub(crate) details: Vec<String>,
    /// The packages involved, as `<name>[:static] <version>-r<rev>`.
    pub(crate) packages: Vec<String>,
    /// The payload or repository paths involved.
    pub(crate) paths: Vec<String>,
}

/// The result of checking one index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Report {
    /// The repository name of the index.
    pub(crate) repo: Name,
    /// Its channel.
    pub(crate) channel: Name,
    /// Its architecture.
    pub(crate) arch: laft_pkg::Arch,
    /// Its generation.
    pub(crate) generation: u64,
    /// How many findings are errors.
    pub(crate) errors: usize,
    /// How many findings are warnings.
    pub(crate) warnings: usize,
    /// The findings, sorted by code and message.
    pub(crate) findings: Vec<Finding>,
}

impl Report {
    /// Writes the findings one per block, then a summary line:
    ///
    /// ```text
    /// KR001 error   file conflict: usr/bin/vi
    ///               owned by: neovim 0.11.4-r1, vim 9.1.0800-r1
    /// 1 finding: 1 error, 0 warnings
    /// ```
    pub(crate) fn write_text(&self, out: &mut impl Write) -> io::Result<()> {
        for finding in &self.findings {
            let level = match finding.level {
                Level::Error => "error",
                Level::Warning => "warning",
            };
            writeln!(out, "{} {level:<7} {}", finding.code, finding.message)?;
            for line in &finding.details {
                writeln!(out, "{:14}{line}", "")?;
            }
        }
        writeln!(
            out,
            "{}: {}, {}",
            plural(self.findings.len(), "finding"),
            plural(self.errors, "error"),
            plural(self.warnings, "warning")
        )
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// What to check.
pub(crate) struct Input<'a> {
    /// The repository whose pool and buildinfo hold the packages.
    pub(crate) repo: &'a Repository,
    /// The candidate index.
    pub(crate) index: &'a Index,
    /// Package files not in the pool yet, by store id.
    pub(crate) staged: &'a BTreeMap<StoreId, PathBuf>,
    /// Buildinfo files not in the repository yet, by output store id.
    pub(crate) staged_buildinfo: &'a BTreeMap<StoreId, PathBuf>,
    /// The index of the same channel that was published before, if any.
    pub(crate) published: Option<&'a Index>,
    /// The indexes of the repository's other channels of the same
    /// architecture.
    pub(crate) others: &'a [Index],
    /// Indexes of base repositories that dependencies may resolve to.
    pub(crate) bases: &'a [Index],
    /// Whether packages from dirty or bootstrap builds are allowed.
    pub(crate) allow_bootstrap: bool,
}

/// Runs every check over `input` and reports the findings.
pub(crate) fn check(input: &Input<'_>) -> Report {
    let mut findings = Vec::new();
    let packages = load(input, &mut findings);
    file_conflicts(&packages, &mut findings);
    let dangling = dangling_dependencies(input, &mut findings);
    closure(input, &dangling, &mut findings);
    alternatives(input.index, &packages, &mut findings);
    revisions(input, &mut findings);
    downgrades(input, &mut findings);
    ambiguous_providers(input.index, &mut findings);
    buildinfo(input, &mut findings);
    findings.sort_by(|a, b| (&a.code, &a.message).cmp(&(&b.code, &b.message)));
    findings.dedup();
    let errors = findings.iter().filter(|f| f.level == Level::Error).count();
    Report {
        repo: input.index.repo.clone(),
        channel: input.index.channel.clone(),
        arch: input.index.arch,
        generation: input.index.generation,
        errors,
        warnings: findings.len() - errors,
        findings,
    }
}

/// The text by which findings name a package.
pub(crate) fn label(meta: &Meta) -> String {
    let variant = match meta.variant {
        Variant::Dynamic => "",
        Variant::Static => ":static",
    };
    format!("{}{variant} {}-r{}", meta.name, meta.version, meta.rev)
}

fn finding(code: &str, level: Level, message: String) -> Finding {
    Finding {
        code: code.into(),
        level,
        message,
        details: Vec::new(),
        packages: Vec::new(),
        paths: Vec::new(),
    }
}

impl Finding {
    fn detail(mut self, line: impl Into<String>) -> Self {
        self.details.push(line.into());
        self
    }

    fn packages<'m>(mut self, metas: impl IntoIterator<Item = &'m Meta>) -> Self {
        self.packages.extend(metas.into_iter().map(label));
        self
    }

    fn paths<S: Into<String>>(mut self, paths: impl IntoIterator<Item = S>) -> Self {
        self.paths.extend(paths.into_iter().map(Into::into));
        self
    }
}

/// A package of the candidate whose file was read.
struct Loaded<'a> {
    entry: &'a IndexEntry,
    manifest: Manifest,
}

/// Reads every package file of the index: KR008, KR009, KR010 and KR011.
fn load<'a>(input: &Input<'a>, findings: &mut Vec<Finding>) -> Vec<Loaded<'a>> {
    let mut loaded = Vec::new();
    for entry in &input.index.packages {
        let meta = &entry.meta;
        let staged = input.staged.get(&entry.id);
        let path = staged.cloned().unwrap_or_else(|| input.repo.pool(entry.id));
        let shown = staged.map_or_else(|| entry.file.clone(), |p| p.display().to_string());
        let kr009 = |message: String| {
            finding("KR009", Level::Error, message)
                .packages([meta])
                .paths([entry.file.clone()])
        };
        match fsx::size(&path) {
            Ok(Some(_)) => {}
            Ok(None) => {
                findings.push(kr009(format!(
                    "missing package file: {shown} ({})",
                    label(meta)
                )));
                continue;
            }
            Err(e) => {
                findings.push(kr009(format!("unreadable package file: {e}")));
                continue;
            }
        }
        match fsx::hash_file(&path) {
            Ok((size, blake3)) if size == entry.size && blake3 == entry.blake3 => {}
            Ok((size, blake3)) => {
                findings.push(
                    kr009(format!(
                        "package file differs from its entry: {shown} ({})",
                        label(meta)
                    ))
                    .detail(format!("file: {size} bytes, {blake3}"))
                    .detail(format!("entry: {} bytes, {}", entry.size, entry.blake3)),
                );
                continue;
            }
            Err(e) => {
                findings.push(kr009(format!("unreadable package file: {e}")));
                continue;
            }
        }
        let reader = File::open(&path)
            .map_err(|e| e.to_string())
            .and_then(|f| Reader::new(BufReader::new(f)).map_err(|e| e.to_string()));
        let reader = match reader {
            Ok(reader) => reader,
            Err(e) => {
                findings.push(
                    finding(
                        "KR008",
                        Level::Error,
                        format!("invalid package file: {shown} ({})", label(meta)),
                    )
                    .detail(e)
                    .packages([meta])
                    .paths([entry.file.clone()]),
                );
                continue;
            }
        };
        let manifest = reader.manifest().clone();
        let id = manifest.store_id();
        let mut differing = differing_fields(meta, reader.meta());
        if id != entry.id {
            differing.insert(0, format!("id (the manifest gives {id})"));
        }
        if !differing.is_empty() {
            findings.push(
                finding(
                    "KR010",
                    Level::Error,
                    format!("entry differs from its package file: {}", label(meta)),
                )
                .detail(format!("differing fields: {}", differing.join(", ")))
                .packages([meta])
                .paths([entry.file.clone()]),
            );
        }
        let generated: Vec<(&str, &str)> = manifest
            .entries()
            .iter()
            .filter(|e| !matches!(e, Entry::Dir { .. }))
            .filter_map(|e| {
                let trigger = TRIGGERS.iter().find(|t| t.generates(e.path()))?;
                Some((e.path(), trigger.name))
            })
            .collect();
        if !generated.is_empty() {
            let mut f = finding(
                "KR011",
                Level::Error,
                format!("payload ships generated files: {}", label(meta)),
            )
            .packages([meta])
            .paths(generated.iter().map(|(path, _)| *path));
            for (path, trigger) in &generated {
                f = f.detail(format!("{path} (generated by the {trigger} trigger)"));
            }
            findings
                .push(f.detail("fix: remove them from the payload, every view generates its own"));
        }
        loaded.push(Loaded { entry, manifest });
    }
    loaded
}

/// The names of the metadata fields whose values differ.
fn differing_fields(a: &Meta, b: &Meta) -> Vec<String> {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(Value::Object(a)), Ok(Value::Object(b))) => {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            keys.into_iter()
                .filter(|k| a.get(*k) != b.get(*k))
                .cloned()
                .collect()
        }
        _ if a == b => Vec::new(),
        _ => vec!["metadata".into()],
    }
}

/// Whether `meta` provides what `dep` asks for. A `pc:` dependency with
/// constraints needs a provide that states a version within them.
fn provides(meta: &Meta, dep: &Dep) -> bool {
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

/// Whether `by` conflicts with or replaces `other`.
fn excludes(by: &Meta, other: &Meta) -> bool {
    by.conflicts
        .iter()
        .chain(&by.replaces)
        .any(|dep| match dep {
            Dep::Pkg(name, req) => *name == other.name && req.matches(&other.version, other.rev),
            _ => provides(other, dep),
        })
}

/// Whether two packages can share a view.
fn coexist(a: &Meta, b: &Meta) -> bool {
    a.name != b.name && !excludes(a, b) && !excludes(b, a)
}

/// Whether both packages list `path` under the same alternatives group.
fn same_alternative(a: &Meta, b: &Meta, path: &str) -> bool {
    a.alternatives.iter().any(|x| {
        x.paths.iter().any(|p| p == path)
            && b.alternatives
                .iter()
                .any(|y| y.group == x.group && y.paths.iter().any(|p| p == path))
    })
}

/// KR001: paths owned by several packages that can share a view.
fn file_conflicts(packages: &[Loaded<'_>], findings: &mut Vec<Finding>) {
    let mut owners: BTreeMap<&str, Vec<(usize, bool)>> = BTreeMap::new();
    for (i, package) in packages.iter().enumerate() {
        for entry in package.manifest.entries() {
            let dir = matches!(entry, Entry::Dir { .. });
            owners.entry(entry.path()).or_default().push((i, dir));
        }
    }
    for (path, owners) in owners.iter().filter(|(_, o)| o.len() > 1) {
        let mut files = BTreeSet::new();
        let mut types = BTreeSet::new();
        for (n, &(i, i_dir)) in owners.iter().enumerate() {
            for &(j, j_dir) in &owners[n + 1..] {
                let (a, b) = (&packages[i].entry.meta, &packages[j].entry.meta);
                if !coexist(a, b) {
                    continue;
                }
                if i_dir != j_dir {
                    types.insert((i, i_dir));
                    types.insert((j, j_dir));
                } else if !i_dir && !same_alternative(a, b, path) {
                    files.insert(i);
                    files.insert(j);
                }
            }
        }
        let meta = |i: usize| &packages[i].entry.meta;
        if !files.is_empty() {
            let names: Vec<String> = files.iter().map(|&i| label(meta(i))).collect();
            findings.push(
                finding("KR001", Level::Error, format!("file conflict: {path}"))
                    .detail(format!("owned by: {}", names.join(", ")))
                    .detail("fix: declare a common alternatives group in their recipes, or make them conflict")
                    .packages(files.iter().map(|&i| meta(i)))
                    .paths([*path]),
            );
        }
        if !types.is_empty() {
            let kinds: Vec<String> = types
                .iter()
                .map(|&(i, dir)| {
                    let kind = if dir {
                        "a directory"
                    } else {
                        "not a directory"
                    };
                    format!("{kind} in {}", label(meta(i)))
                })
                .collect();
            findings.push(
                finding("KR001", Level::Error, format!("type conflict: {path}"))
                    .detail(format!("the path is {}", kinds.join(", ")))
                    .packages(types.iter().map(|&(i, _)| meta(i)))
                    .paths([*path]),
            );
        }
    }
}

/// The text a provide or dependency is looked up by: its kind and name,
/// without a version.
fn provide_key(provide: &Provide) -> String {
    match provide {
        Provide::So(soname) => format!("so:{soname}"),
        Provide::Pc(name, _) => format!("pc:{name}"),
        Provide::Virtual(name) => format!("virtual:{name}"),
    }
}

fn dep_key(dep: &Dep) -> Option<String> {
    match dep {
        Dep::So(soname) => Some(format!("so:{soname}")),
        Dep::Pc(name, _) => Some(format!("pc:{name}")),
        Dep::Virtual(name) => Some(format!("virtual:{name}")),
        Dep::Pkg(..) => None,
    }
}

/// KR002: dependencies nothing satisfies. Returns the ids of the packages
/// that have one, whose closure is not checked again.
fn dangling_dependencies(input: &Input<'_>, findings: &mut Vec<Finding>) -> HashSet<StoreId> {
    let all = || {
        std::iter::once(input.index)
            .chain(input.bases)
            .flat_map(|index| &index.packages)
            .map(|e| &e.meta)
    };
    let mut by_name: HashMap<&str, Vec<&Meta>> = HashMap::new();
    let mut by_key: HashMap<String, Vec<&Meta>> = HashMap::new();
    let mut replaced: HashSet<&str> = HashSet::new();
    for meta in all() {
        by_name.entry(meta.name.as_str()).or_default().push(meta);
        for provide in &meta.provides {
            by_key.entry(provide_key(provide)).or_default().push(meta);
        }
        for dep in &meta.replaces {
            if let Dep::Pkg(name, _) = dep {
                replaced.insert(name.as_str());
            }
        }
    }
    let satisfiable = |dep: &Dep| match dep {
        Dep::Pkg(name, req) => {
            by_name
                .get(name.as_str())
                .is_some_and(|metas| metas.iter().any(|m| req.matches(&m.version, m.rev)))
                || (*req == VersionReq::Any && replaced.contains(name.as_str()))
        }
        _ => dep_key(dep)
            .and_then(|key| by_key.get(&key))
            .is_some_and(|metas| metas.iter().any(|m| provides(m, dep))),
    };
    let mut dangling = HashSet::new();
    for entry in &input.index.packages {
        for dep in entry.meta.depends.iter().filter(|d| !satisfiable(d)) {
            dangling.insert(entry.id);
            findings.push(
                finding(
                    "KR002",
                    Level::Error,
                    format!("dangling dependency: {} needs {dep}", label(&entry.meta)),
                )
                .detail("nothing in this index or a base index satisfies it")
                .packages([&entry.meta]),
            );
        }
    }
    dangling
}

/// KR003: every package resolves on its own.
///
/// A package is requested by its name, its variant and its full version as
/// a prefix, so a newer version that starts with the same version tokens
/// is resolved in its place.
fn closure(input: &Input<'_>, dangling: &HashSet<StoreId>, findings: &mut Vec<Finding>) {
    let mut repos = vec![laft_resolve::Repository {
        index: input.index,
        priority: 1,
    }];
    repos.extend(
        input
            .bases
            .iter()
            .map(|index| laft_resolve::Repository { index, priority: 0 }),
    );
    let universe = Universe::new(&repos);
    for entry in &input.index.packages {
        if dangling.contains(&entry.id) {
            continue;
        }
        let meta = &entry.meta;
        let prefix = meta.version.as_str().parse().ok();
        let mut request = ResolveInput::new(label(meta));
        request.requests.push(Request {
            pkg: PkgRef::new(meta.name.clone(), prefix, meta.variant),
            origin: "laft-repo check".into(),
        });
        if let Err(e) = universe.resolve(&request) {
            let mut f = finding(
                "KR003",
                Level::Error,
                format!("closure: {} cannot be installed on its own", label(meta)),
            )
            .packages([meta]);
            for line in e.to_string().lines() {
                f = f.detail(line);
            }
            findings.push(f);
        }
    }
}

/// KR004: alternatives groups are consistent and have a default.
fn alternatives(index: &Index, packages: &[Loaded<'_>], findings: &mut Vec<Finding>) {
    let shipped: HashMap<StoreId, &Manifest> =
        packages.iter().map(|p| (p.entry.id, &p.manifest)).collect();
    let mut groups: BTreeMap<&Name, Vec<(&IndexEntry, BTreeSet<&str>)>> = BTreeMap::new();
    for entry in &index.packages {
        for alternative in &entry.meta.alternatives {
            let paths = alternative.paths.iter().map(String::as_str).collect();
            groups
                .entry(&alternative.group)
                .or_default()
                .push((entry, paths));
            if let Some(manifest) = shipped.get(&entry.id) {
                for path in &alternative.paths {
                    let ships = manifest
                        .entries()
                        .iter()
                        .any(|e| e.path() == path && !matches!(e, Entry::Dir { .. }));
                    if !ships {
                        findings.push(
                            finding(
                                "KR004",
                                Level::Error,
                                format!(
                                    "alternatives group {} of {} lists {path}, which the package does not ship",
                                    alternative.group,
                                    label(&entry.meta)
                                ),
                            )
                            .packages([&entry.meta])
                            .paths([path.as_str()]),
                        );
                    }
                }
            }
        }
    }
    for (group, members) in &groups {
        let names: BTreeSet<&Name> = members.iter().map(|(e, _)| &e.meta.name).collect();
        let listed = names
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let metas = || members.iter().map(|(e, _)| &e.meta);
        match index.defaults.alternatives.get(*group) {
            None => findings.push(
                finding(
                    "KR004",
                    Level::Error,
                    format!("alternatives group {group} has no default"),
                )
                .detail(format!("members: {listed}"))
                .detail(format!("fix: choose one with --alternative {group}=<member>"))
                .packages(metas()),
            ),
            Some(default) if !names.contains(default) => findings.push(
                finding(
                    "KR004",
                    Level::Error,
                    format!("the default of alternatives group {group} is {default}, which is not a member"),
                )
                .detail(format!("members: {listed}"))
                .packages(metas()),
            ),
            Some(_) => {}
        }
        let (_, first) = &members[0];
        if members.iter().any(|(_, paths)| paths != first) {
            let mut f = finding(
                "KR004",
                Level::Error,
                format!("members of alternatives group {group} ship different paths"),
            )
            .packages(metas());
            for (entry, paths) in members {
                let paths: Vec<&str> = paths.iter().copied().collect();
                f = f.detail(format!("{}: {}", label(&entry.meta), paths.join(", ")));
            }
            findings.push(f);
        }
    }
}

/// The identity a revision must not be reused for.
type Identity<'a> = (&'a Name, &'a Version, u32, Variant);

/// KR005: a published identity keeps its store id.
fn revisions(input: &Input<'_>, findings: &mut Vec<Finding>) {
    let mut published: HashMap<Identity<'_>, StoreId> = HashMap::new();
    for index in input.published.into_iter().chain(input.others) {
        for e in &index.packages {
            let m = &e.meta;
            published.insert((&m.name, &m.version, m.rev, m.variant), e.id);
        }
        for y in &index.yanked {
            published.insert((&y.name, &y.version, y.rev, y.variant), y.id);
        }
    }
    for entry in &input.index.packages {
        let m = &entry.meta;
        if let Some(id) = published.get(&(&m.name, &m.version, m.rev, m.variant))
            && *id != entry.id
        {
            findings.push(
                finding(
                    "KR005",
                    Level::Error,
                    format!(
                        "revision not bumped: {} is already published with another build",
                        label(m)
                    ),
                )
                .detail(format!("published id: {id}"))
                .detail(format!("new id: {}", entry.id))
                .detail("fix: increase rev in the recipe")
                .packages([m]),
            );
        }
    }
}

/// KR006: the newest version of a name does not go back.
fn downgrades(input: &Input<'_>, findings: &mut Vec<Finding>) {
    let Some(published) = input.published else {
        return;
    };
    let yanked: HashSet<StoreId> = input.index.yanked.iter().map(|y| y.id).collect();
    let before = newest(published);
    for (name, now) in newest(input.index) {
        let Some(old) = before.get(name) else {
            continue;
        };
        if (&now.meta.version, now.meta.rev) < (&old.meta.version, old.meta.rev)
            && !yanked.contains(&old.id)
        {
            findings.push(
                finding(
                    "KR006",
                    Level::Warning,
                    format!(
                        "downgrade: {} (published {}-r{})",
                        label(&now.meta),
                        old.meta.version,
                        old.meta.rev
                    ),
                )
                .packages([&now.meta, &old.meta]),
            );
        }
    }
}

/// The newest package of each name.
fn newest(index: &Index) -> BTreeMap<&Name, &IndexEntry> {
    let mut newest: BTreeMap<&Name, &IndexEntry> = BTreeMap::new();
    for e in &index.packages {
        let slot = newest.entry(&e.meta.name).or_insert(e);
        if (&e.meta.version, e.meta.rev) > (&slot.meta.version, slot.meta.rev) {
            *slot = e;
        }
    }
    newest
}

/// KR007: a shared library or virtual capability has one provider per
/// view unless a default chooses one of its providers.
fn ambiguous_providers(index: &Index, findings: &mut Vec<Finding>) {
    let mut providers: BTreeMap<String, Vec<&Meta>> = BTreeMap::new();
    for entry in &index.packages {
        for provide in &entry.meta.provides {
            if matches!(provide, Provide::So(_) | Provide::Virtual(_)) {
                providers
                    .entry(provide_key(provide))
                    .or_default()
                    .push(&entry.meta);
            }
        }
    }
    for (key, metas) in providers {
        let default = index.defaults.providers.get(&key);
        if default.is_some_and(|d| metas.iter().any(|m| m.name == *d)) {
            continue;
        }
        let mut involved: BTreeSet<usize> = BTreeSet::new();
        for (i, a) in metas.iter().enumerate() {
            for (j, b) in metas.iter().enumerate().skip(i + 1) {
                if coexist(a, b) {
                    involved.insert(i);
                    involved.insert(j);
                }
            }
        }
        if involved.is_empty() {
            continue;
        }
        let labels: Vec<String> = involved.iter().map(|&i| label(metas[i])).collect();
        let mut f = finding("KR007", Level::Error, format!("ambiguous provider: {key}"));
        if let Some(default) = default {
            f = f.detail(format!("the default {default} does not provide {key}"));
        }
        findings.push(
            f.detail(format!("provided by: {}", labels.join(", ")))
                .detail(format!(
                    "fix: choose one with --provider {key}=<name>, or make them conflict"
                ))
                .packages(involved.iter().map(|&i| metas[i])),
        );
    }
}

/// CI001 and CI003: every package has buildinfo from a clean, regular
/// build.
fn buildinfo(input: &Input<'_>, findings: &mut Vec<Finding>) {
    for entry in &input.index.packages {
        let meta = &entry.meta;
        let path = input
            .staged_buildinfo
            .get(&entry.id)
            .cloned()
            .unwrap_or_else(|| input.repo.buildinfo(entry.id));
        let ci003 = |message: String| {
            finding("CI003", Level::Error, message)
                .packages([meta])
                .paths([format!("buildinfo/{}.json", entry.id)])
        };
        let info = match fsx::size(&path) {
            Ok(Some(_)) => match Buildinfo::read(&path) {
                Ok(info) => info,
                Err(e) => {
                    findings.push(
                        ci003(format!("unreadable buildinfo for {}", label(meta)))
                            .detail(e.to_string()),
                    );
                    continue;
                }
            },
            Ok(None) => {
                findings.push(ci003(format!("no buildinfo for {}", label(meta))));
                continue;
            }
            Err(e) => {
                findings.push(
                    ci003(format!("unreadable buildinfo for {}", label(meta)))
                        .detail(e.to_string()),
                );
                continue;
            }
        };
        if input.allow_bootstrap {
            continue;
        }
        let reason = match (info.dirty(), info.bootstrap) {
            (true, _) => "a recipe tree with uncommitted changes",
            (false, true) => "a bootstrap build",
            (false, false) => continue,
        };
        findings.push(
            finding(
                "CI001",
                Level::Error,
                format!("unpublishable build: {} comes from {reason}", label(meta)),
            )
            .detail("fix: rebuild from a clean recipe tree, or pass --allow-bootstrap for a bootstrap repository")
            .packages([meta]),
        );
    }
}
