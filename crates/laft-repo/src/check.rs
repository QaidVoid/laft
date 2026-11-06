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
//! | KR004 | error | An alternatives group whose default is not a member, that has no default while the newest versions of several members share the highest priority, whose members ship different paths, or that lists a path its member does not ship. |
//! | KR005 | error | A package whose name, version, revision, variant and option values are already published with another store id. Profile indexes are exempt. |
//! | KR006 | warning | The newest version of a name is older than the newest one previously published. |
//! | KR007 | error | Packages of several names that can share a view provide the same `so:` or `virtual:` value, and no provider default chooses one of them. |
//! | KR008 | error | A package file whose metadata or manifest is invalid. |
//! | KR009 | error | A package file that is missing or differs in size or hash from its entry. |
//! | KR010 | error | An entry whose metadata or store id differs from its package file. |
//! | KR011 | error | A payload that ships a file a trigger generates. |
//! | KR012 | warning | The options of a recipe where no combination of their other values gives the recipe the plan the defaults give. Runs only with `--recipes`; it evaluates at most 64 combinations, and each one re-evaluates the recipe. |
//! | KR013 | error | A publish block or profile that names a recipe without options, an option the recipe does not declare, or a value the option does not take. Runs only with `--recipes`. |
//! | KR014 | error | A package whose metadata sets an option its recipe does not declare, misses one it declares, or sets one to a value its type does not take. |
//! | KR015 | error | A dependency whose option constraints name an option the target recipe does not declare, or a value the option does not take, or whose constraints cannot be verified. |
//! | KR016 | error | The `recipes` member names an archive the repository lacks, or one of another size or hash. |
//! | CI001 | error | A package built from a dirty recipe tree or by a bootstrap build, unless allowed. |
//! | CI003 | error | A package without readable buildinfo. |
//!
//! Two packages can share a view when their names differ and neither
//! conflicts with or replaces the other.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;

use laft_core::{
    Dep, Name, OptionConstraint, OptionValue, PkgRef, Provide, StoreId, TRIGGERS, Variant, Version,
    VersionReq,
};
use laft_index::{Index, IndexEntry, OptionType};
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
    /// The recipe repository the packages were built from, for the checks
    /// that read recipes.
    pub(crate) recipes: Option<&'a laft_build::Repos>,
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
    option_metadata(input, &mut findings);
    option_constraints(input, &mut findings);
    recipe_snapshot(input, &mut findings);
    if input.recipes.is_some() {
        publish_blocks(input, &mut findings);
        option_effect(input, &mut findings);
    }
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
            Dep::Pkg(name, req, _) => *name == other.name && req.matches(&other.version, other.rev),
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
            if let Dep::Pkg(name, _, _) = dep {
                replaced.insert(name.as_str());
            }
        }
    }
    let satisfiable = |dep: &Dep| match dep {
        Dep::Pkg(name, req, _) => {
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
        local: false,
    }];
    repos.extend(input.bases.iter().map(|index| laft_resolve::Repository {
        index,
        priority: 0,
        local: false,
    }));
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

/// KR004: alternatives groups are consistent, and the resolver can choose
/// a member of each without the configuration: the group has a default, or
/// the newest version of one member has a higher priority than the newest
/// versions of all others.
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
            None => {
                let ranked: Vec<(i64, &Name)> = newest(members.iter().map(|(e, _)| *e))
                    .into_iter()
                    .map(|(name, entry)| (priority(entry, group), name))
                    .collect();
                let top = ranked.iter().map(|(p, _)| *p).max().unwrap_or_default();
                let tied: Vec<&str> = ranked
                    .iter()
                    .filter(|(p, _)| *p == top)
                    .map(|(_, n)| n.as_str())
                    .collect();
                if tied.len() > 1 {
                    findings.push(
                        finding(
                            "KR004",
                            Level::Error,
                            format!(
                                "alternatives group {group} has no default, and {} share the highest priority {top}",
                                tied.join(", ")
                            ),
                        )
                        .detail(format!("members: {listed}"))
                        .detail(format!(
                            "fix: choose one with --alternative {group}=<member>, or give one a higher priority"
                        ))
                        .packages(metas()),
                    );
                }
            }
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

/// The identity a revision must not be reused for: the name, version,
/// revision, variant and option values of a build.
type Identity<'a> = (
    &'a Name,
    &'a Version,
    u32,
    Variant,
    &'a BTreeMap<String, OptionValue>,
);

/// KR005: a published identity keeps its store id, in the channel and in
/// every other channel of the architecture. Profile indexes are exempt:
/// building a profile rebuilds the packages built against its changed
/// ones, under the same identity and with new store ids.
fn revisions(input: &Input<'_>, findings: &mut Vec<Finding>) {
    if input.index.profile.is_some() {
        return;
    }
    let mut published: HashMap<Identity<'_>, BTreeSet<StoreId>> = HashMap::new();
    let mut withdrawn: HashMap<Identity<'_>, BTreeSet<StoreId>> = HashMap::new();
    for index in input.published.into_iter().chain(input.others) {
        if index.profile.is_some() {
            continue;
        }
        for e in &index.packages {
            let m = &e.meta;
            published
                .entry((&m.name, &m.version, m.rev, m.variant, &m.options))
                .or_default()
                .insert(e.id);
        }
        for y in &index.yanked {
            withdrawn
                .entry((&y.name, &y.version, y.rev, y.variant, &y.options))
                .or_default()
                .insert(y.id);
        }
    }
    for entry in &input.index.packages {
        let m = &entry.meta;
        let identity = (&m.name, &m.version, m.rev, m.variant, &m.options);
        let published_ids = published.get(&identity);
        let withdrawn_ids = withdrawn.get(&identity);
        let others: Vec<&StoreId> = published_ids
            .into_iter()
            .chain(withdrawn_ids)
            .flat_map(|ids| ids.iter())
            .filter(|id| **id != entry.id)
            .collect();
        if let [id, ..] = others[..] {
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
    let before = newest(&published.packages);
    for (name, now) in newest(&input.index.packages) {
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

/// The priority that `entry` gives itself in alternatives `group`.
fn priority(entry: &IndexEntry, group: &Name) -> i64 {
    entry
        .meta
        .alternatives
        .iter()
        .find(|a| &a.group == group)
        .map_or(0, |a| a.priority)
}

/// The newest of `entries` for each name.
fn newest<'a>(
    entries: impl IntoIterator<Item = &'a IndexEntry>,
) -> BTreeMap<&'a Name, &'a IndexEntry> {
    let mut newest: BTreeMap<&Name, &IndexEntry> = BTreeMap::new();
    for e in entries {
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

/// Whether `value` is of the type `kind` declares.
fn fits(kind: &OptionType, value: &OptionValue) -> bool {
    match (kind, value) {
        (OptionType::Bool(what), OptionValue::Bool(_)) => what == "bool",
        (OptionType::Tags(tags), OptionValue::Tag(tag)) => tags.iter().any(|t| t == tag),
        _ => false,
    }
}

/// KR014: every package sets exactly the options its recipe declares, with
/// values of the declared types, and the values its index's profile member
/// promises for it.
fn option_metadata(input: &Input<'_>, findings: &mut Vec<Finding>) {
    for entry in &input.index.packages {
        let meta = &entry.meta;
        let kr014 = |message: String| finding("KR014", Level::Error, message).packages([meta]);
        let Some(decls) = input.index.options.get(&meta.origin) else {
            if meta.options.is_empty() {
                continue;
            }
            let names: Vec<&str> = meta.options.keys().map(String::as_str).collect();
            findings.push(kr014(format!(
                "option metadata: {} sets the options {}, but the index records no option declarations for it",
                label(meta),
                names.join(", ")
            )));
            continue;
        };
        let mut problems = Vec::new();
        for (name, value) in &meta.options {
            let Some(decl) = decls.iter().find(|d| &d.name == name) else {
                problems.push(format!(
                    "the option {name} is set, which the recipe does not declare"
                ));
                continue;
            };
            if !fits(&decl.kind, value) {
                problems.push(format!(
                    "the option {name} is set to {value}, which does not fit its type"
                ));
            }
        }
        for decl in decls {
            if !meta.options.contains_key(&decl.name) {
                problems.push(format!(
                    "the option {} is declared, which the package does not set",
                    decl.name
                ));
            }
        }
        if let Some(profile) = &input.index.profile
            && let Some(values) = profile.values.get(&meta.origin)
        {
            for (option, value) in values {
                match meta.options.get(option) {
                    None => problems.push(format!(
                        "the profile {} sets the option {option}, which the build does not set",
                        profile.name
                    )),
                    Some(set) if set != value => problems.push(format!(
                        "the profile {} sets the option {option} to {value}, but the build sets {set}",
                        profile.name
                    )),
                    _ => {}
                }
            }
        }
        if !problems.is_empty() {
            let mut f = kr014(format!("option metadata: {}", label(meta)));
            for problem in problems {
                f = f.detail(problem);
            }
            findings.push(f);
        }
    }
}

/// KR015: every option constraint of a dependency names an option and tag
/// that the target recipe declares.
fn option_constraints(input: &Input<'_>, findings: &mut Vec<Finding>) {
    for entry in &input.index.packages {
        let meta = &entry.meta;
        let deps = meta
            .depends
            .iter()
            .chain(&meta.conflicts)
            .chain(&meta.replaces);
        for dep in deps {
            let Dep::Pkg(target, _, constraints) = dep else {
                continue;
            };
            if constraints.is_empty() {
                continue;
            }
            let kr015 = |message: String| {
                finding("KR015", Level::Error, message)
                    .packages([meta])
                    .detail(format!("the dependency: {dep}"))
            };
            let index_decls = input.index.options.get(target).map(Vec::as_slice);
            let base_decls = if index_decls.is_none() {
                input
                    .bases
                    .iter()
                    .find_map(|base| base.options.get(target).map(Vec::as_slice))
            } else {
                None
            };
            let recipe_decls = match input.recipes {
                Some(repos) if index_decls.is_none() && base_decls.is_none() => {
                    match repos.options(target) {
                        Ok(decls) => decls.map(|decls| decls.declarations),
                        Err(e) => {
                            findings.push(
                                kr015(format!(
                                    "option constraints: {} requires options of {target}, but the index records no option declarations for {target}, so they cannot be verified",
                                    label(meta)
                                ))
                                .detail(e.to_string()),
                            );
                            continue;
                        }
                    }
                }
                _ => None,
            };
            let Some(decls) = index_decls.or(base_decls).or(recipe_decls.as_deref()) else {
                let why = match input.recipes {
                    Some(repos) if repos.packages().any(|name| name == target) => {
                        format!("{target} declares no options")
                    }
                    Some(_) => format!("no repository defines the package {target}"),
                    None => format!(
                        "the index records no option declarations for {target}, so they cannot be verified"
                    ),
                };
                findings.push(kr015(format!(
                    "option constraints: {} requires options of {target}, but {why}",
                    label(meta)
                )));
                continue;
            };
            for constraint in constraints.as_slice() {
                let name = constraint.name();
                let Some(decl) = decls.iter().find(|d| d.name == name) else {
                    findings.push(kr015(format!(
                        "option constraints: the dependency on {target} requires the option {name}, which the recipe of {target} does not declare"
                    )));
                    continue;
                };
                let fits = match constraint {
                    OptionConstraint::True { .. } | OptionConstraint::False { .. } => {
                        matches!(&decl.kind, OptionType::Bool(_))
                    }
                    OptionConstraint::Tag { tag, .. } => {
                        fits(&decl.kind, &OptionValue::Tag(tag.clone()))
                    }
                };
                if !fits {
                    findings.push(kr015(format!(
                        "option constraints: the dependency on {target} requires {constraint}, which does not fit the type of the option {name}"
                    )));
                }
            }
        }
    }
}

/// KR013: every publish block and profile names recipes with options,
/// options they declare and tags they have.
fn publish_blocks(input: &Input<'_>, findings: &mut Vec<Finding>) {
    let repos = input.recipes.expect("the check runs with recipes");
    for repo in repos.repos() {
        for (recipe, set) in &repo.publish {
            let what = format!("the publish block for {recipe} of repository {}", repo.name);
            option_set(input, findings, &what, recipe, set);
        }
        for (profile, member) in &repo.profiles {
            for (recipe, set) in &member.package {
                let what = format!(
                    "the profile {profile} of repository {} for {recipe}",
                    repo.name
                );
                option_set(input, findings, &what, recipe, set);
            }
        }
    }
}

/// The KR013 findings for one `publish` block or profile entry, named by
/// `what`.
fn option_set(
    input: &Input<'_>,
    findings: &mut Vec<Finding>,
    what: &str,
    recipe: &Name,
    set: &BTreeMap<String, laft_build::OptionText>,
) {
    let repos = input.recipes.expect("the check runs with recipes");
    let kr013 = |message: String| finding("KR013", Level::Error, message).detail(what.to_owned());
    let declared = match repos.options(recipe) {
        Ok(declared) => declared,
        Err(e) => {
            findings.push(kr013(format!("{what}: {e}")));
            return;
        }
    };
    let Some(decls) = declared else {
        let defined = repos.packages().any(|name| name == recipe);
        let why = if defined {
            "the recipe declares no options".to_owned()
        } else {
            format!("no repository defines the recipe {recipe}")
        };
        findings.push(kr013(format!("{what}: {why}")));
        return;
    };
    for (option, text) in set {
        let value = match text {
            laft_build::OptionText::Bool(value) => OptionValue::Bool(*value),
            laft_build::OptionText::Tag(tag) => OptionValue::Tag(tag.clone()),
        };
        let Some(decl) = decls.declarations.iter().find(|d| &d.name == option) else {
            findings.push(kr013(format!(
                "{what}: sets the option {option}, which the recipe does not declare"
            )));
            continue;
        };
        if !fits(&decl.kind, &value) {
            findings.push(kr013(format!(
                "{what}: sets the option {option} to {value}, which does not fit its type"
            )));
        }
    }
}

/// The plan hashes of a recipe evaluation, one per variant.
fn plan_hashes(recipes: &[laft_build::Recipe]) -> Vec<laft_core::Blake3Hash> {
    recipes.iter().map(|recipe| recipe.plan_hash()).collect()
}

/// The most value combinations KR012 evaluates for one recipe. Each
/// combination re-evaluates the recipe, so the cap bounds the cost of the
/// check.
const KR012_COMBINATIONS: usize = 64;

/// KR012: options that give the recipe the plan the defaults give, as far
/// as the sweep can tell them apart.
///
/// The check enumerates combinations of the non-default values of every
/// declared option, including the ones that set a single option, because
/// an effect can need several values together, and evaluates the recipe
/// with each of the first [`KR012_COMBINATIONS`] of them. An option has
/// an effect when two evaluated combinations that differ only in that
/// option give different plan hashes. A recipe whose default evaluation
/// is empty, because it builds nothing for this architecture, has no plan
/// to compare and is skipped with a note.
fn option_effect(input: &Input<'_>, findings: &mut Vec<Finding>) {
    let repos = input.recipes.expect("the check runs with recipes");
    for name in repos.packages() {
        let decls = match repos.options(name) {
            Ok(Some(decls)) => decls,
            Ok(None) => continue,
            Err(e) => {
                findings.push(
                    finding(
                        "KR012",
                        Level::Error,
                        format!("cannot read the option declarations of {name}"),
                    )
                    .detail(e.to_string()),
                );
                continue;
            }
        };
        let mut hashes: HashMap<BTreeMap<String, OptionValue>, _> = HashMap::new();
        let default = match repos.evaluate_with(name, &BTreeMap::new()) {
            Ok(evaluation) => {
                let hash = plan_hashes(&evaluation.recipes);
                hashes.insert(BTreeMap::new(), hash.clone());
                hash
            }
            Err(e) => {
                findings.push(
                    finding(
                        "KR012",
                        Level::Error,
                        format!("cannot evaluate {name} to check its options"),
                    )
                    .detail(e.to_string()),
                );
                continue;
            }
        };
        if default.is_empty() {
            findings.push(finding(
                "KR012",
                Level::Warning,
                format!(
                    "option effect unchecked: {name} builds nothing on {}, so its options have no plan to compare",
                    input.index.arch
                ),
            ));
            continue;
        }
        let (combinations, total) = value_combinations(&decls.declarations);
        let truncated = total > combinations.len();
        let mut broken = false;
        for set in &combinations {
            match repos.evaluate_with(name, set) {
                Ok(evaluation) => {
                    hashes.insert(set.clone(), plan_hashes(&evaluation.recipes));
                }
                Err(e) => {
                    let shown = set
                        .iter()
                        .map(|(option, value)| format!("{option}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    findings.push(
                        finding(
                            "KR012",
                            Level::Error,
                            format!("cannot evaluate {name} with {shown} to check its options"),
                        )
                        .detail(e.to_string()),
                    );
                    broken = true;
                    break;
                }
            }
        }
        if broken {
            continue;
        }
        let mut useless = Vec::new();
        for decl in &decls.declarations {
            let mut has_effect = false;
            let mut undecided = false;
            for set in &combinations {
                if !set.contains_key(&decl.name) {
                    continue;
                }
                let mut base = set.clone();
                base.remove(&decl.name);
                match (hashes.get(set), hashes.get(&base)) {
                    (Some(hash), Some(base)) if hash != base => {
                        has_effect = true;
                        break;
                    }
                    (Some(_), Some(_)) => {}
                    _ => {
                        undecided = true;
                        break;
                    }
                }
            }
            if !has_effect && !undecided {
                useless.push(decl.name.as_str());
            }
        }
        if !useless.is_empty() {
            let options = useless.join(", ");
            let warned = finding(
                "KR012",
                Level::Warning,
                format!("option without effect: {name}: {options}"),
            );
            findings.push(if truncated {
                warned.detail(format!(
                    "evaluated the first {} of {total} combinations",
                    combinations.len()
                ))
            } else {
                warned
            });
        }
    }
}

/// The combinations of the non-default values of `decls`, as option sets,
/// capped at [`KR012_COMBINATIONS`] entries. Every option contributes the
/// combinations that leave it at its default and the ones that give it
/// each of its non-default values, so the result holds the all-defaults
/// combination alone when no option has one, and never otherwise. The
/// second element is the size of the uncapped enumeration.
fn value_combinations(
    decls: &[laft_index::OptionDecl],
) -> (Vec<BTreeMap<String, OptionValue>>, usize) {
    let mut combinations: Vec<BTreeMap<String, OptionValue>> = vec![BTreeMap::new()];
    let mut total: usize = 1;
    for decl in decls {
        let values: Vec<OptionValue> = match &decl.kind {
            OptionType::Bool(_) => vec![OptionValue::Bool(decl.default == OptionValue::FALSE)],
            OptionType::Tags(tags) => tags
                .iter()
                .filter(|tag| Some(tag.as_str()) != default_tag(&decl.default))
                .map(|tag| OptionValue::Tag(tag.clone()))
                .collect(),
        };
        if values.is_empty() {
            continue;
        }
        total = total.saturating_mul(values.len() + 1);
        let mut next: Vec<BTreeMap<String, OptionValue>> = combinations.clone();
        for combination in &combinations {
            for value in &values {
                let mut set = combination.clone();
                set.insert(decl.name.clone(), value.clone());
                next.push(set);
            }
        }
        combinations = next;
        combinations.truncate(KR012_COMBINATIONS);
    }
    (combinations, total)
}

/// The tag of a default value, for a tag option.
fn default_tag(default: &OptionValue) -> Option<&str> {
    match default {
        OptionValue::Tag(tag) => Some(tag),
        OptionValue::Bool(_) => None,
    }
}

/// KR016: the recipe snapshot member names an archive the repository
/// holds, with the size and hash the member records.
fn recipe_snapshot(input: &Input<'_>, findings: &mut Vec<Finding>) {
    let Some(snapshot) = &input.index.recipes else {
        return;
    };
    let path = input.repo.path(&snapshot.file);
    let kr016 =
        |message: String| finding("KR016", Level::Error, message).paths([snapshot.file.clone()]);
    match fsx::size(&path) {
        Ok(Some(_)) => {}
        Ok(None) => {
            findings.push(kr016(format!(
                "the recipe snapshot {} is missing",
                snapshot.file
            )));
            return;
        }
        Err(e) => {
            findings.push(kr016(format!("the recipe snapshot is unreadable: {e}")));
            return;
        }
    }
    let (size, blake3) = match fsx::hash_file(&path) {
        Ok(found) => found,
        Err(e) => {
            findings.push(kr016(format!("the recipe snapshot is unreadable: {e}")));
            return;
        }
    };
    if size != snapshot.size || blake3 != snapshot.blake3 {
        findings.push(
            kr016(format!(
                "the recipe snapshot {} differs from its member",
                snapshot.file
            ))
            .detail(format!("file: {size} bytes, {blake3}"))
            .detail(format!(
                "member: {} bytes, {}",
                snapshot.size, snapshot.blake3
            )),
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
