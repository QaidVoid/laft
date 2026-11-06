//! Building the packages that no repository publishes, from the recipe
//! snapshots of their indexes, and publishing the results in the local
//! repository.
//!
//! One run proceeds in three steps:
//!
//! 1. Every request is assigned to the recipe snapshot that builds it,
//!    which is fetched, checked against the hash its index records and
//!    unpacked below the recipe cache. Planning one snapshot with
//!    `laft-build` resolves the recipes of that snapshot itself; every
//!    dependency of its graph that no recipe of the snapshot builds is
//!    either published, and then seeded from a repository, or buildable
//!    by another snapshot, and then becomes a build request of its own.
//! 2. The snapshot groups build in an order in which every group builds
//!    after the groups whose packages it waits for, through the
//!    `laft-build` binary, as the build account. A group seeds the
//!    published dependencies of its graph first, so it sees the packages
//!    that earlier groups built.
//! 3. The package files of every group are verified and published in the
//!    local repository, and seeded into the build-side store for the
//!    groups that follow.
//!
//! Everything that writes into the build root runs through the
//! `laft-build` binary, which a root caller starts as the build account,
//! `uid` 43; only the step that publishes runs as root, because it writes
//! the local repository and its key. A caller that is not root, as the
//! tests are, runs `laft-build` with its own ids and a build root below
//! the `--root` directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use laft_build::{BuildRoot, Builder, Buildinfo, Mode, Repos, SourceCache};
use laft_core::{Blake3Hash, Name, OptionConstraint, OptionValue, PkgRef};
use laft_store::read_regular;
use serde_json::Value;

use crate::env::{Env, RepoIndex};
use crate::failure::{Failure, exit};
use crate::local::{BUILDINFO_MAX_BYTES, Local, PackageFile, write_new};
use crate::resolve::{SnapshotMap, SnapshotSource};

/// The account local builds run as, `laft-build` of the system accounts.
const BUILD_UID: u32 = 43;
const BUILD_GID: u32 = 43;

/// The most requests one run discovers, and the most rounds one group
/// plans for.
const LIMIT: usize = 256;

/// How often `laft apply` builds what a resolution asks for and resolves
/// again, before it gives up.
pub(crate) const ROUNDS: usize = 8;

/// What a run built.
#[derive(Debug, Default)]
pub(crate) struct Built {
    /// The names that entered the local repository.
    pub(crate) names: Vec<Name>,
    /// The bytes downloaded for recipe snapshots.
    pub(crate) bytes: u64,
}

/// One package a run builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    /// The package.
    pub(crate) name: Name,
    /// The variant the snapshot builds.
    pub(crate) variant: laft_core::Variant,
    /// The option values the build gets.
    pub(crate) values: BTreeMap<String, OptionValue>,
    /// The view that needs the package.
    pub(crate) view: String,
    /// Why the view needs it, one line per reason.
    pub(crate) reasons: Vec<String>,
}

impl Request {
    /// The request of a resolution, for the view called `view`.
    pub(crate) fn of(build: &laft_resolve::BuildRequest, view: &str) -> Self {
        Self {
            name: build.buildable.name.clone(),
            variant: build.buildable.variant,
            values: build.values.clone(),
            view: view.to_owned(),
            reasons: build.reasons.iter().map(reason_text).collect(),
        }
    }

    /// Written like a package reference: `mesa` or `mesa:static`.
    pub(crate) fn item(&self) -> String {
        match self.variant {
            laft_core::Variant::Dynamic => self.name.to_string(),
            variant => format!("{}:{variant}", self.name),
        }
    }

    /// The `--set` arguments of the build, sorted.
    fn sets(&self) -> Vec<String> {
        self.values
            .iter()
            .map(|(option, value)| format!("{}.{}={}", self.name, option, value))
            .collect()
    }

    /// What sets one build apart from another build of the same package:
    /// the values, as `option=value`, joined.
    pub(crate) fn signature(&self) -> String {
        self.values
            .iter()
            .map(|(option, value)| format!("{option}={value}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The build requests of one resolution: the system view's, then the
/// initrd closure's, then the views' in name order.
pub(crate) fn requests_of(resolved: &crate::resolve::Resolved<'_>) -> Vec<Request> {
    let mut out: Vec<Request> = resolved
        .system
        .builds
        .iter()
        .map(|build| Request::of(build, crate::resolve::SYSTEM))
        .collect();
    out.extend(
        resolved
            .initrd
            .builds
            .iter()
            .map(|build| Request::of(build, crate::resolve::INITRD)),
    );
    for (name, resolution) in &resolved.views {
        out.extend(
            resolution
                .builds
                .iter()
                .map(|build| Request::of(build, &name.to_string())),
        );
    }
    out
}

/// Runs every build `requests` needs and publishes the results.
///
/// `snapshots` says which repository and snapshot build each name,
/// `options` carries the build settings of the configuration, and
/// `values` holds the fixed and preferred option values of every view.
///
/// # Errors
///
/// Fails when a snapshot cannot be fetched or unpacked, when a dependency
/// is neither published nor buildable, when `laft-build` fails, or when a
/// package cannot be published.
pub(crate) fn run(
    env: &Env,
    requests: &[Request],
    snapshots: &SnapshotMap,
    indexes: &[RepoIndex],
    options: &laft_config::Build,
    values: &BTreeMap<Name, laft_config::OptionValues>,
) -> Result<Built, Failure> {
    let root = ensure_build_root(env)?;
    let mut run = Run {
        env,
        snapshots,
        indexes,
        options,
        values,
        built: Built::default(),
        seeded: BTreeSet::new(),
    };
    let mut groups: BTreeMap<(Blake3Hash, String), Vec<Request>> = BTreeMap::new();
    let mut waiting: BTreeMap<(Blake3Hash, String), BTreeSet<(Blake3Hash, String)>> =
        BTreeMap::new();
    let mut queued: BTreeSet<(Name, String)> = BTreeSet::new();
    let mut queue: Vec<Request> = requests.to_vec();
    let mut rounds = 0;
    while let Some(request) = queue.pop() {
        rounds += 1;
        if rounds > LIMIT {
            return Err(Failure::new(
                exit::FAILURE,
                "the builds the configuration needs keep asking for new dependencies, \
                 which is an internal error",
            ));
        }
        let Some(source) = snapshots.get(&(request.name.clone(), request.variant)) else {
            return Err(Failure::new(
                exit::FAILURE,
                format!(
                    "{} is not published, and no recipe snapshot builds it",
                    request.item()
                ),
            )
            .hint("add a repository that publishes it, or a recipe snapshot for it"));
        };
        let blake3 = source.snapshot.blake3;
        let here = (blake3, request.signature());
        let key = (request.name.clone(), request.signature());
        if !queued.insert(key) {
            // The same package with the same values, asked for by another
            // view: the group records it, so that the choice of published
            // dependencies counts every view of the group.
            if let Some(members) = groups.get_mut(&here) {
                members.push(request);
            }
            continue;
        }
        run.unpack(source)?;
        groups
            .entry(here.clone())
            .or_default()
            .push(request.clone());
        let view = request.view.clone();
        for reference in run.scan(&blake3, &[request], &root)? {
            match run.depends_on(&reference, &view)? {
                Where::Published => {}
                Where::Group { dependency, there } => {
                    // The edge carries the group key of the dependency,
                    // so that groups of one snapshot with different
                    // values still build in dependency order.
                    let before = (there, dependency.signature());
                    if before != here {
                        waiting.entry(here.clone()).or_default().insert(before);
                    }
                    queue.push(dependency);
                }
                Where::Nothing => {
                    return Err(Failure::new(
                        exit::FAILURE,
                        format!(
                            "{reference} is neither published nor built by any recipe snapshot"
                        ),
                    ));
                }
            }
        }
    }
    for key in order(&groups, &waiting)? {
        let requests = &groups[&key];
        run.group(&key.0, requests, &root)?;
    }
    Ok(run.built)
}

/// Where a dependency of a build graph comes from.
#[derive(Debug)]
enum Where {
    /// A repository publishes it, so the group seeds it.
    Published,
    /// Another snapshot of this run builds it, as `dependency`, whose
    /// snapshot is `there`.
    Group {
        dependency: Request,
        there: Blake3Hash,
    },
    /// Nothing provides it.
    Nothing,
}

/// The state of one run.
struct Run<'a> {
    env: &'a Env,
    snapshots: &'a SnapshotMap,
    indexes: &'a [RepoIndex],
    options: &'a laft_config::Build,
    values: &'a BTreeMap<Name, laft_config::OptionValues>,
    built: Built,
    seeded: BTreeSet<Name>,
}

impl<'a> Run<'a> {
    /// Where `reference` comes from, with the request for it when another
    /// snapshot builds it. A reference a repository publishes is not
    /// marked here: it is recorded only once it is actually seeded, so
    /// that the group that needs it seeds it.
    fn depends_on(&mut self, reference: &PkgRef, view: &str) -> Result<Where, Failure> {
        let name = reference.name().clone();
        if self.candidate(reference, view).is_some() {
            return Ok(Where::Published);
        }
        match self.snapshots.get(&(name, reference.variant())) {
            None => Ok(Where::Nothing),
            Some(source) => {
                let dependency = self.dependency(reference, source, view)?;
                Ok(Where::Group {
                    dependency,
                    there: source.snapshot.blake3,
                })
            }
        }
    }

    /// The request for the package `source` builds, with the values the
    /// option context of `view` gives it.
    fn dependency(
        &self,
        reference: &PkgRef,
        source: &SnapshotSource,
        view: &str,
    ) -> Result<Request, Failure> {
        let mut values = source.defaults.clone();
        let mut fixed = BTreeSet::new();
        if self.options.dep_options == laft_config::DepOptions::Context
            && let Some(context) = view_values(self.values, view)
        {
            if let Some(set) = context.fixed.get(&source.name) {
                for (option, setting) in set {
                    values.insert(option.clone(), setting.value.clone());
                    fixed.insert(option.clone());
                }
            }
            if let Some(preferred) = context.preferred.get(&source.name) {
                for (option, value) in preferred {
                    values
                        .entry(option.clone())
                        .or_insert_with(|| value.clone());
                }
            }
        }
        for constraint in reference.options().as_slice() {
            constrain(constraint, &mut values, &fixed, reference)?;
        }
        Ok(Request {
            name: source.name.clone(),
            variant: source.variant,
            values,
            view: view.to_owned(),
            reasons: vec![format!("{reference}, which no repository publishes")],
        })
    }

    /// The best published package of `reference`: the newest entry whose
    /// options hold every constraint of the reference and, with
    /// `dep_options = context`, every fixed value of `view`.
    fn candidate(
        &self,
        reference: &PkgRef,
        view: &str,
    ) -> Option<(&laft_index::IndexEntry, &RepoIndex)> {
        fn rank<'a>(
            e: &'a laft_index::IndexEntry,
            index: &'a RepoIndex,
        ) -> (&'a laft_core::Version, u32, i64) {
            (&e.meta.version, e.meta.rev, index.repo.priority)
        }
        let mut best: Option<(&laft_index::IndexEntry, &RepoIndex)> = None;
        for index in self.indexes {
            for entry in &index.accepted.index.packages {
                if entry.meta.name != *reference.name() || entry.meta.variant != reference.variant()
                {
                    continue;
                }
                if let Some(prefix) = reference.prefix()
                    && !prefix.matches(&entry.meta.version)
                {
                    continue;
                }
                if !entry.meta.options.iter().all(|(option, value)| {
                    reference
                        .options()
                        .as_slice()
                        .iter()
                        .all(|c| c.name() != option.as_str() || c.matches(value))
                }) {
                    continue;
                }
                if self.options.dep_options == laft_config::DepOptions::Context
                    && let Some(context) = view_values(self.values, view)
                    && let Some(fixed) = context.fixed.get(reference.name())
                    && fixed.iter().any(|(option, setting)| {
                        entry.meta.options.get(option) != Some(&setting.value)
                    })
                {
                    continue;
                }
                if best.is_none_or(|(current, current_index)| {
                    rank(entry, index) > rank(current, current_index)
                }) {
                    best = Some((entry, index));
                }
            }
        }
        best
    }

    /// The dependencies of `requests` that no recipe of the snapshot
    /// builds and no package in the store satisfies.
    fn scan(
        &self,
        blake3: &Blake3Hash,
        requests: &[Request],
        root: &BuildRoot,
    ) -> Result<Vec<PkgRef>, Failure> {
        let repos = Repos::open(&[snapshot_dir(self.env, blake3)]).map_err(usage)?;
        let cache = SourceCache::new(root.sources(), self.env.fetcher());
        let mut recipes = Vec::new();
        for request in requests {
            let evaluation = repos
                .evaluate_with(&request.name, &request.values)
                .map_err(usage)?;
            recipes.extend(evaluation.recipes);
        }
        let builder = Builder::new(&repos, &cache, root, Mode::Store);
        builder
            .plan_open(recipes, &root.db(), &BTreeSet::new(), &BTreeSet::new())
            .map(|(_, _, unsatisfied)| unsatisfied)
            .map_err(build_failure)
    }

    /// Fetches the snapshot of `source` and unpacks it into the recipe
    /// cache, unless that already happened. The archive goes into a
    /// sibling staging tree that is renamed into place, so an unpack
    /// that is interrupted leaves no tree a later run takes for
    /// complete.
    fn unpack(&mut self, source: &SnapshotSource) -> Result<(), Failure> {
        let dir = snapshot_dir(self.env, &source.snapshot.blake3);
        if dir.join("recipes").is_dir() {
            return Ok(());
        }
        let staging = dir.with_file_name(format!(
            "{}.part",
            dir.file_name().and_then(|n| n.to_str()).unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&staging);
        let archive = self.fetch(source)?;
        if let Err(e) = laft_build::unpack_normalized_tarball(&archive, &staging) {
            let _ = fs::remove_dir_all(&staging);
            return Err(Failure::new(
                exit::FAILURE,
                format!("{}: {e}", dir.display()),
            ));
        }
        if let Err(e) = fs::rename(&staging, &dir) {
            let _ = fs::remove_dir_all(&staging);
            if !dir.join("recipes").is_dir() {
                return Err(Failure::new(
                    exit::FAILURE,
                    format!("{}: {e}", dir.display()),
                ));
            }
        }
        self.built.bytes += source.snapshot.size;
        Ok(())
    }

    /// The downloaded, hash-checked snapshot archive of `source`.
    fn fetch(&self, source: &SnapshotSource) -> Result<PathBuf, Failure> {
        let cache = self
            .env
            .path("/laft/cache/recipes")
            .join(format!("{}.tar.zst", source.snapshot.blake3));
        if verified(&cache, source) {
            return Ok(cache);
        }
        let index = self
            .indexes
            .iter()
            .find(|index| index.name == source.repo)
            .ok_or_else(|| {
                Failure::new(
                    exit::FAILURE,
                    format!(
                        "repo {} is not enabled, but a build needs its recipe snapshot",
                        source.repo
                    ),
                )
            })?;
        let _ = fs::remove_file(&cache);
        let mut failures = Vec::new();
        for base in index.mirrors.bases() {
            let url = format!(
                "{}/{}",
                base.as_str().trim_end_matches('/'),
                source.snapshot.file
            );
            let fetched = self.env.fetcher().fetch(
                base,
                &source.snapshot.file,
                laft_fetch::ObjectKind::Index,
                &mut |_| {},
            );
            let bytes = match fetched {
                Ok(bytes) => bytes,
                Err(error) => {
                    failures.push(format!("{url}: {error}"));
                    continue;
                }
            };
            if bytes.len() as u64 != source.snapshot.size
                || Blake3Hash::of(&bytes) != source.snapshot.blake3
            {
                failures.push(format!("{url} does not match the hash the index records"));
                continue;
            }
            write_new(&cache, &bytes)?;
            return Ok(cache);
        }
        let mut message = String::new();
        for failure in failures {
            message.push_str(&format!("\n  {failure}"));
        }
        Err(Failure::new(
            exit::NETWORK,
            format!(
                "the recipe snapshot of repo {} cannot be fetched:{}",
                source.repo, message
            ),
        ))
    }

    /// Builds the requests whose snapshot is `blake3` with the values of
    /// `signature`: seeds the published dependencies of its graph, then
    /// builds, then publishes and seeds the results.
    fn group(
        &mut self,
        blake3: &Blake3Hash,
        requests: &[Request],
        root: &BuildRoot,
    ) -> Result<(), Failure> {
        // Several views may need the same build. The published
        // dependencies are chosen with the context of the first view of
        // the group that accepts a candidate.
        let mut views: Vec<&str> = Vec::new();
        for request in requests {
            if !views.contains(&request.view.as_str()) {
                views.push(request.view.as_str());
            }
        }
        for _ in 0..LIMIT {
            let references = self.scan(blake3, requests, root)?;
            let pending: Vec<&PkgRef> = references
                .iter()
                .filter(|reference| !self.seeded.contains(reference.name()))
                .collect();
            if pending.is_empty() {
                return self.build(blake3, requests, root);
            }
            for reference in pending {
                let mut found = None;
                for view in &views {
                    if let Some((entry, index)) = self.candidate(reference, view) {
                        found = Some((entry.clone(), index.mirrors.clone()));
                        break;
                    }
                }
                match found {
                    Some((entry, mirrors)) => {
                        self.seeded.insert(reference.name().clone());
                        self.seed(root, &entry, &mirrors)?;
                    }
                    None => {
                        return Err(Failure::new(
                            exit::FAILURE,
                            format!(
                                "{reference} is neither published nor built by any recipe snapshot"
                            ),
                        ));
                    }
                }
            }
        }
        Err(Failure::new(
            exit::FAILURE,
            "the dependencies of the builds do not settle, which is an internal error",
        ))
    }

    /// Downloads the package of `entry` from `mirrors` and seeds it into
    /// the build-side store.
    fn seed(
        &mut self,
        root: &BuildRoot,
        entry: &laft_index::IndexEntry,
        mirrors: &laft_fetch::Mirrors,
    ) -> Result<(), Failure> {
        let path = self
            .env
            .fetcher()
            .download_package(mirrors, entry, &mut |progress| {
                self.env
                    .ui
                    .progress("fetching", progress.done, progress.total.unwrap_or(0));
            })
            .map_err(|e| Failure::new(exit::NETWORK, format!("{}: {e}", entry.meta.name)))?;
        self.exec_seed(root, &[path.as_path()])
    }

    /// Builds the requests of `blake3` through the `laft-build` binary,
    /// then verifies and publishes the package files of the build. Only
    /// the outputs that `laft-build` names for the requested items are
    /// published, with the buildinfo that names them; when an item is up
    /// to date, its recorded outputs are published, and when those no
    /// longer match the record, the item is built again.
    fn build(
        &mut self,
        blake3: &Blake3Hash,
        requests: &[Request],
        root: &BuildRoot,
    ) -> Result<(), Failure> {
        let mut arguments = vec![
            "--json".to_owned(),
            "--root".to_owned(),
            root.path().to_string_lossy().into_owned(),
            "--recipes".to_owned(),
            snapshot_dir(self.env, blake3)
                .to_string_lossy()
                .into_owned(),
        ];
        for request in requests {
            for set in request.sets() {
                arguments.push("--set".to_owned());
                arguments.push(set);
            }
        }
        arguments.extend(Self::context_arguments(
            self.options.dep_options,
            self.values,
            &requests.iter().map(|r| r.view.as_str()).collect(),
        ));
        let items: BTreeSet<String> = requests.iter().map(Request::item).collect();
        arguments.push("build".to_owned());
        arguments.extend(items.iter().cloned());
        let names = items.iter().cloned().collect::<Vec<_>>().join(", ");
        self.env
            .ui
            .phase("building", format!("{names} from a recipe snapshot"));
        let report = self.build_once(&arguments, &names)?;
        let mut pairs = self.reported(root, &report)?;
        let mut stale: Vec<&Request> = Vec::new();
        for request in requests {
            if report.contains_key(&request.item()) {
                continue;
            }
            match self.recorded(root, request)? {
                Some(more) => pairs.extend(more),
                None => stale.push(request),
            }
        }
        if !stale.is_empty() {
            // The output directory holds other files where the recorded
            // outputs of these items should be, such as the packages of
            // another option set. Build them again, so that exactly what
            // this configuration asked for is published.
            let items = stale
                .iter()
                .map(|request| request.item())
                .collect::<BTreeSet<_>>();
            let names = items.iter().cloned().collect::<Vec<_>>().join(", ");
            let mut again = vec![
                "--json".to_owned(),
                "--root".to_owned(),
                root.path().to_string_lossy().into_owned(),
                "--recipes".to_owned(),
                snapshot_dir(self.env, blake3)
                    .to_string_lossy()
                    .into_owned(),
            ];
            for request in &stale {
                for set in request.sets() {
                    again.push("--set".to_owned());
                    again.push(set);
                }
            }
            again.extend(Self::context_arguments(
                self.options.dep_options,
                self.values,
                &stale.iter().map(|r| r.view.as_str()).collect(),
            ));
            again.push("build".to_owned());
            again.push("--rebuild".to_owned());
            again.extend(items.iter().cloned());
            let report = self.build_once(&again, &names)?;
            for item in &items {
                if !report.contains_key(item) {
                    return Err(Failure::new(
                        exit::FAILURE,
                        format!(
                            "the rebuild of {item} reports nothing, which is an internal error"
                        ),
                    ));
                }
            }
            pairs.extend(self.reported(root, &report)?);
        }
        let local = Local::of(self.env.root.path());
        let added = local.add(&pairs)?;
        let listed: Vec<laft_index::IndexEntry> = match local.index()? {
            Some(index) => index.packages,
            None => Vec::new(),
        };
        for request in requests {
            let found = added.iter().any(|added| {
                added.name == request.name
                    && added.variant == request.variant
                    && added.options == request.values
            }) || listed.iter().any(|entry| {
                entry.meta.name == request.name
                    && entry.meta.variant == request.variant
                    && entry.meta.options == request.values
            });
            if !found {
                return Err(Failure::new(
                    exit::FAILURE,
                    format!(
                        "the build of {} produced no package with the values {} the configuration asked for",
                        request.item(),
                        request.signature()
                    ),
                ));
            }
        }
        let mut names: Vec<Name> = Vec::new();
        for request in requests {
            if !names.contains(&request.name) {
                names.push(request.name.clone());
            }
        }
        self.built.names.extend(names);
        let refs: Vec<&Path> = pairs.iter().map(|pair| pair.path.as_path()).collect();
        self.exec_seed(root, &refs)
    }

    /// Runs `laft-build` with `arguments` and parses its JSON report,
    /// which names what it built. Standard output is the report; the
    /// progress lines stay on standard error.
    fn build_once(
        &self,
        arguments: &[String],
        names: &str,
    ) -> Result<BTreeMap<String, Reported>, Failure> {
        let output = self.exec_captured(arguments)?;
        if !output.status.success() {
            return Err(Failure::new(
                exit::FAILURE,
                format!(
                    "the build of {names} failed; its logs are below {}",
                    self.env.path("/laft/build/logs").display()
                ),
            ));
        }
        parse_report(&output.stdout)
    }

    /// The package files and buildinfo of every item `report` names.
    fn reported(
        &self,
        root: &BuildRoot,
        report: &BTreeMap<String, Reported>,
    ) -> Result<Vec<PackageFile>, Failure> {
        let mut pairs = Vec::new();
        for entry in report.values() {
            let buildinfo = self.output_file(root, &entry.buildinfo)?;
            for name in &entry.packages {
                pairs.push(PackageFile {
                    path: self.output_file(root, name)?,
                    buildinfo: buildinfo.clone(),
                });
            }
        }
        Ok(pairs)
    }

    /// The path of the file `name` of the output directory of `root`.
    /// Only plain file names are accepted.
    fn output_file(&self, root: &BuildRoot, name: &str) -> Result<PathBuf, Failure> {
        let plain = !name.is_empty()
            && name
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        if !plain {
            return Err(Failure::new(
                exit::FAILURE,
                format!("the build reports a file name this run cannot publish: {name}"),
            ));
        }
        Ok(root.out().join(name))
    }

    /// The package files the build database records for `request`, when
    /// the buildinfo in the output directory still names the recorded
    /// outputs. `None` when there is no record, or when the output
    /// directory holds other files there, which means the request must be
    /// built again.
    fn recorded(
        &self,
        root: &BuildRoot,
        request: &Request,
    ) -> Result<Option<Vec<PackageFile>>, Failure> {
        let item = laft_build::Item::new(request.name.clone(), request.variant);
        let stored = root.db().latest(&item).map_err(build_failure)?;
        let Some(stored) = stored else {
            return Ok(None);
        };
        let path = self.output_file(root, &stored.buildinfo)?;
        let bytes = read_regular(&path, BUILDINFO_MAX_BYTES)
            .map_err(|e| Failure::new(exit::FAILURE, format!("{}: {e}", path.display())))?;
        let info = Buildinfo::from_json(&bytes)
            .map_err(|e| Failure::new(exit::FAILURE, format!("{}: {e}", path.display())))?;
        let named = stored.record.outputs.len() == info.outputs.len()
            && stored
                .record
                .outputs
                .iter()
                .all(|(name, id)| info.outputs.iter().any(|o| &o.name == name && &o.id == id));
        if !named {
            return Ok(None);
        }
        let mut pairs = Vec::new();
        for name in &stored.files {
            pairs.push(PackageFile {
                path: self.output_file(root, name)?,
                buildinfo: path.clone(),
            });
        }
        Ok(Some(pairs))
    }

    /// The arguments that carry the option context of `views` to
    /// `laft-build`, so that their build views resolve with the features
    /// the views use. With `dep_options = defaults` no context is passed
    /// and the views resolve with the declared defaults instead.
    fn context_arguments(
        dep_options: laft_config::DepOptions,
        values: &BTreeMap<Name, laft_config::OptionValues>,
        views: &BTreeSet<&str>,
    ) -> Vec<String> {
        if dep_options != laft_config::DepOptions::Context {
            return vec!["--dep-options".to_owned(), "defaults".to_owned()];
        }
        let mut out = Vec::new();
        for view in views {
            let Some(values) = view_values(values, view) else {
                continue;
            };
            for (package, options) in &values.fixed {
                for (option, setting) in options {
                    out.push("--option".to_owned());
                    out.push(format!("{}.{}={}", package, option, setting.value));
                }
            }
            for (package, options) in &values.preferred {
                for (option, value) in options {
                    out.push("--prefer".to_owned());
                    out.push(format!("{}.{}={}", package, option, value));
                }
            }
        }
        out
    }

    /// Runs `laft-build seed` on `files`, against the store of `root`.
    fn exec_seed(&mut self, root: &BuildRoot, files: &[&Path]) -> Result<(), Failure> {
        if files.is_empty() {
            return Ok(());
        }
        let arguments = seed_arguments(root.path(), files);
        let status = self.exec(&arguments)?;
        if !status.success() {
            return Err(Failure::new(
                exit::FAILURE,
                "the build-side store cannot take the packages",
            ));
        }
        Ok(())
    }

    /// Runs the `laft-build` binary next to this program with
    /// `arguments`, capturing its standard output, as the build account
    /// when this process is root.
    fn exec_captured(&self, arguments: &[String]) -> Result<std::process::Output, Failure> {
        let exe = std::env::current_exe()
            .map_err(|e| Failure::new(exit::FAILURE, format!("the program directory: {e}")))?
            .parent()
            .map(|dir| dir.join("laft-build"))
            .ok_or_else(|| Failure::new(exit::FAILURE, "the program has no directory"))?;
        let mut command = Command::new(&exe);
        command.args(arguments);
        command.env_clear();
        command.env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        );
        command.env("HOME", self.env.path("/laft/build"));
        command.stdout(std::process::Stdio::piped());
        // `run_as` switches to the build account when this process is
        // root; a caller that is not root, as the tests are, runs the
        // build with its own ids.
        let output = if rustix::process::getuid().is_root() {
            laft_sandbox::run_as_output(&mut command, BUILD_UID, BUILD_GID)
        } else {
            command.output()
        };
        output.map_err(|e| Failure::new(exit::FAILURE, format!("{}: {e}", exe.display())))
    }

    /// Runs the `laft-build` binary next to this program with
    /// `arguments`, as the build account when this process is root.
    fn exec(&self, arguments: &[String]) -> Result<std::process::ExitStatus, Failure> {
        let exe = std::env::current_exe()
            .map_err(|e| Failure::new(exit::FAILURE, format!("the program directory: {e}")))?
            .parent()
            .map(|dir| dir.join("laft-build"))
            .ok_or_else(|| Failure::new(exit::FAILURE, "the program has no directory"))?;
        let mut command = Command::new(&exe);
        command.args(arguments);
        command.env_clear();
        command.env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        );
        command.env("HOME", self.env.path("/laft/build"));
        // `run_as` switches to the build account when this process is
        // root; a caller that is not root, as the tests are, runs the
        // build with its own ids.
        let status = if rustix::process::getuid().is_root() {
            laft_sandbox::run_as(&mut command, BUILD_UID, BUILD_GID)
        } else {
            command.status()
        };
        status.map_err(|e| Failure::new(exit::FAILURE, format!("{}: {e}", exe.display())))
    }
}

/// The arguments of `laft-build seed`: the build root the store lives
/// under, then the files.
fn seed_arguments(root: &Path, files: &[&Path]) -> Vec<String> {
    let mut arguments = vec![
        "--root".to_owned(),
        root.to_string_lossy().into_owned(),
        "seed".to_owned(),
    ];
    arguments.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
    arguments
}

/// What `laft-build` reports for one item it built: the file names of its
/// packages and of its buildinfo in the output directory.
#[derive(Debug)]
struct Reported {
    packages: Vec<String>,
    buildinfo: String,
}

/// Parses the JSON report of `laft-build --json build`: an object with a
/// `built` array that names, per item, the packages and the buildinfo in
/// the output directory.
fn parse_report(bytes: &[u8]) -> Result<BTreeMap<String, Reported>, Failure> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| {
        Failure::new(
            exit::FAILURE,
            format!("the build report cannot be read: {e}"),
        )
    })?;
    let built = value
        .get("built")
        .and_then(|built| built.as_array())
        .ok_or_else(|| Failure::new(exit::FAILURE, "the build report has no built list"))?;
    let mut out = BTreeMap::new();
    for entry in built {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .ok_or_else(|| {
                    Failure::new(
                        exit::FAILURE,
                        format!("the build report has no {key} for an item"),
                    )
                })
        };
        let packages = entry
            .get("packages")
            .and_then(|value| value.as_array())
            .ok_or_else(|| Failure::new(exit::FAILURE, "the build report has no packages"))?
            .iter()
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    Failure::new(exit::FAILURE, "the build report names a package strangely")
                })
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        out.insert(
            text("item")?,
            Reported {
                packages,
                buildinfo: text("buildinfo")?,
            },
        );
    }
    Ok(out)
}

/// The groups in an order in which every group comes after the groups
/// whose packages it waits for. The edges are the group keys `waiting`
/// carries.
fn order(
    groups: &BTreeMap<(Blake3Hash, String), Vec<Request>>,
    waiting: &BTreeMap<(Blake3Hash, String), BTreeSet<(Blake3Hash, String)>>,
) -> Result<Vec<(Blake3Hash, String)>, Failure> {
    let mut out = Vec::new();
    let mut left: BTreeSet<(Blake3Hash, String)> = groups.keys().cloned().collect();
    while !left.is_empty() {
        let next = left
            .iter()
            .find(|candidate| {
                waiting
                    .get(*candidate)
                    .is_none_or(|before| before.iter().all(|b| !left.contains(b)))
            })
            .cloned();
        match next {
            Some(key) => {
                left.remove(&key);
                out.push(key);
            }
            None => {
                return Err(Failure::new(
                    exit::FAILURE,
                    "the snapshots the builds need depend on each other in a cycle",
                ));
            }
        }
    }
    Ok(out)
}

/// The option values of the view called `view`.
fn view_values<'a>(
    values: &'a BTreeMap<Name, laft_config::OptionValues>,
    view: &str,
) -> Option<&'a laft_config::OptionValues> {
    values.get(&view.parse::<Name>().ok()?)
}

/// Applies `constraint` to `values`. A fixed value wins; a constraint
/// that contradicts one is refused.
fn constrain(
    constraint: &OptionConstraint,
    values: &mut BTreeMap<String, OptionValue>,
    fixed: &BTreeSet<String>,
    reference: &PkgRef,
) -> Result<(), Failure> {
    let value = match constraint {
        OptionConstraint::True { .. } => OptionValue::Bool(true),
        OptionConstraint::False { .. } => OptionValue::Bool(false),
        OptionConstraint::Tag { tag, .. } => OptionValue::Tag(tag.clone()),
    };
    let name = constraint.name();
    match values.get(name) {
        Some(previous) if previous == &value => Ok(()),
        Some(previous) if fixed.contains(name) => Err(Failure::new(
            exit::FAILURE,
            format!(
                "{reference} needs {} = {value}, but the configuration fixes it to {previous}",
                name
            ),
        )),
        _ => {
            values.insert(name.to_owned(), value);
            Ok(())
        }
    }
}

/// Whether `path` has the size and hash the snapshot names.
fn verified(path: &Path, source: &SnapshotSource) -> bool {
    fs::read(path).is_ok_and(|bytes| {
        bytes.len() as u64 == source.snapshot.size
            && Blake3Hash::of(&bytes) == source.snapshot.blake3
    })
}

/// The directory the unpacked snapshot `blake3` lives in, below the
/// recipe cache.
fn snapshot_dir(env: &Env, blake3: &Blake3Hash) -> PathBuf {
    env.path("/laft/cache/recipes").join(blake3.to_string())
}

/// The build root at `/laft/build`, owned by the build account with mode
/// `0700` when this process is root.
fn ensure_build_root(env: &Env) -> Result<BuildRoot, Failure> {
    let path = env.path("/laft/build");
    fs::create_dir_all(&path)
        .map_err(|e| Failure::new(exit::FAILURE, format!("{}: {e}", path.display())))?;
    if rustix::process::getuid().is_root() {
        rustix::fs::chown(
            &path,
            Some(rustix::process::Uid::from_raw(BUILD_UID)),
            Some(rustix::process::Gid::from_raw(BUILD_GID)),
        )
        .map_err(|e| Failure::new(exit::FAILURE, format!("{}: {e}", path.display())))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|e| Failure::new(exit::FAILURE, format!("{}: {e}", path.display())))?;
    }
    Ok(BuildRoot::new(path))
}

/// A failure about `laft-build` refusing its arguments.
fn usage(error: impl std::fmt::Display) -> Failure {
    Failure::new(exit::USAGE, error.to_string())
}

/// A failure of a build.
fn build_failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(exit::FAILURE, error.to_string())
}

#[cfg(test)]
#[path = "builds_tests.rs"]
mod tests;

/// One line for a reason of a build request: the request as written, or
/// the dependency that requires the package.
pub(crate) fn reason_text(reason: &laft_resolve::Reason) -> String {
    match reason {
        laft_resolve::Reason::Requested { text, .. } => text.clone(),
        laft_resolve::Reason::Dependency { by, dep } => format!("a dependency of {by}: {dep}"),
        laft_resolve::Reason::OptionMoved { .. } => String::new(),
    }
}
