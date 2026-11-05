//! What every command works with: the root, the configuration, the lock,
//! the repository indexes and the generations.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use laft_config::{IndexOptions, Loaded, PkgError, Repo, SystemConfig};
use laft_core::{Blake3Hash, Name, PkgRef, Timestamp};
use laft_fetch::{Config as FetchConfig, Fetcher, Mirrors, Source, SyncRequest};
use laft_index::{AcceptedIndex, Index, IndexRecord, IndexState, MAX_INDEX_SIZE};
use laft_pkg::Arch;
use laft_store::{Error as StoreError, Lock, Root};
use serde_json::json;

use crate::cli::Global;
use crate::failure::{Failure, exit};
use crate::ui::{self, Ui};

/// How old an index may be before a command that writes syncs it again.
const STALE: Duration = Duration::from_secs(3600);

/// The architecture this build of Laft runs on.
pub(crate) const ARCH: Arch = Arch::X86_64V3;

/// The command's environment.
#[derive(Debug, Clone)]
pub(crate) struct Env {
    /// How to talk to the user.
    pub(crate) ui: Ui,
    /// The root of the system the command works on.
    pub(crate) root: Root,
    /// Whether `--root` was given, so that nothing is activated live.
    pub(crate) offline_root: bool,
    /// The configuration entry file.
    pub(crate) config: PathBuf,
    /// Whether the network must not be used.
    pub(crate) offline: bool,
    /// Whether a busy lock fails instead of being waited for.
    pub(crate) no_wait: bool,
}

/// When repository indexes are synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncMode {
    /// Only the cached indexes are used.
    Cached,
    /// Indexes older than an hour are synced.
    IfStale,
    /// Every index is synced.
    Always,
}

/// The cached index a state record pins, when its file still matches.
fn cached_index(path: &Path, record: &IndexRecord) -> Option<Index> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_INDEX_SIZE + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_INDEX_SIZE {
        return None;
    }
    let Some(hash) = &record.blake3 else {
        return None;
    };
    if Blake3Hash::of(&bytes) != *hash {
        return None;
    }
    Index::decode(&bytes).ok()
}

/// The option declarations and profile values of the synced indexes, by
/// the priority of their repositories: when several repositories declare
/// options for one package, those of the repository with the highest
/// priority win, and name order breaks a tie.
pub(crate) fn options_of(indexes: &[RepoIndex]) -> IndexOptions {
    let mut by_priority: Vec<&RepoIndex> = indexes.iter().collect();
    by_priority.sort_by_key(|repo| (repo.repo.priority, repo.name.clone()));
    let mut out = IndexOptions::default();
    for repo in by_priority {
        for (name, decls) in &repo.accepted.index.options {
            out.options.insert(name.clone(), decls.clone());
        }
        if let Some(profile) = &repo.accepted.index.profile {
            for (name, values) in &profile.values {
                out.profile.insert(name.clone(), values.clone());
            }
        }
    }
    out
}

/// The accepted index of one enabled repository.
#[derive(Debug)]
pub(crate) struct RepoIndex {
    /// The repository name.
    pub(crate) name: Name,
    /// Its configuration.
    pub(crate) repo: Repo,
    /// The accepted index.
    pub(crate) accepted: AcceptedIndex,
    /// The URLs to download packages from, best first.
    pub(crate) mirrors: Mirrors,
    /// When the index was last synced, if known.
    pub(crate) synced: Option<SystemTime>,
}

impl Env {
    /// The environment of `global`.
    pub(crate) fn new(global: &Global) -> Self {
        let ui = Ui::new(global);
        let root_dir = global.root.clone().unwrap_or_else(|| PathBuf::from("/"));
        let root_dir = fs::canonicalize(&root_dir).unwrap_or(root_dir);
        let config = global
            .config
            .clone()
            .or_else(|| {
                std::env::var_os("LAFT_CONFIG")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| root_dir.join("etc/laft/system.stav"));
        Self {
            ui,
            root: Root::new(root_dir),
            offline_root: global.root.is_some(),
            config,
            offline: global.offline,
            no_wait: global.no_wait,
        }
    }

    /// The path of `path`, absolute on the system, below the root.
    pub(crate) fn path(&self, path: &str) -> PathBuf {
        self.root.path().join(path.trim_start_matches('/'))
    }

    /// Whether this process may write to the system: as root, or below a
    /// root given with `--root`.
    pub(crate) fn can_write(&self) -> bool {
        self.offline_root || rustix::process::geteuid().is_root()
    }

    /// Fails unless this process may write to the system.
    pub(crate) fn require_write(&self, command: &str) -> Result<(), Failure> {
        if self.can_write() {
            Ok(())
        } else {
            Err(Failure::needs_root(command))
        }
    }

    /// Takes the transaction lock for `command`, waiting for it unless
    /// `--no-wait` was given, and finishes interrupted transactions.
    pub(crate) fn lock(&self, command: &str) -> Result<Lock, Failure> {
        self.require_write(command)?;
        let lock = match self.root.try_lock(command) {
            Ok(lock) => lock,
            Err(StoreError::Busy(holder)) => {
                let who = holder.map_or_else(
                    || "another process".to_owned(),
                    |h| {
                        format!(
                            "pid {} (laft {}, started {} ago)",
                            h.pid,
                            h.command,
                            ui::age(h.started, Timestamp::now())
                        )
                    },
                );
                if self.no_wait {
                    return Err(Failure::new(
                        exit::BUSY,
                        format!("the lock is held by {who}"),
                    ));
                }
                self.ui.note(format!("waiting for lock held by {who}"));
                self.root.lock(command)?
            }
            Err(e) => return Err(e.into()),
        };
        let recovered = laft_activate::recover(&lock, &self.esp(None))?;
        for recovery in recovered {
            self.ui.note(recovery.to_string());
        }
        crate::live::resume(self, &lock)?;
        Ok(lock)
    }

    /// The directory holding the configuration.
    pub(crate) fn config_dir(&self) -> PathBuf {
        let dir = self.config.parent().unwrap_or(Path::new("/"));
        fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
    }

    /// The EFI system partition below the root: `boot.esp` of `config`
    /// when the command loaded the configuration, otherwise as the
    /// configuration of the default generation recorded it, and `/boot`
    /// when no generation records one.
    pub(crate) fn esp(&self, config: Option<&SystemConfig>) -> PathBuf {
        let esp = match config {
            Some(config) => config.boot.boot.esp.clone(),
            None => recorded_esp(&self.root).unwrap_or_else(|| "/boot".to_owned()),
        };
        self.path(&esp)
    }

    /// Loads the configuration, resolving package references with
    /// `packages`, and prints its warnings.
    pub(crate) fn load(
        &self,
        packages: &dyn Fn(&PkgRef) -> Result<(), PkgError>,
    ) -> Result<Loaded, Failure> {
        let name = self.config.file_name().and_then(|n| n.to_str());
        if name != Some("system.stav") {
            return Err(Failure::new(
                exit::USAGE,
                format!(
                    "{}: the configuration entry file must be named system.stav",
                    self.config.display()
                ),
            ));
        }
        if !self.config.exists() {
            return Err(Failure::new(
                exit::CONFIG,
                format!("{} does not exist", self.config.display()),
            ));
        }
        let options = self.cached_options();
        self.load_with(&options, packages)
    }

    /// As [`Self::load`], with the option declarations `options` given
    /// instead of the cached ones.
    pub(crate) fn load_with(
        &self,
        options: &IndexOptions,
        packages: &dyn Fn(&PkgRef) -> Result<(), PkgError>,
    ) -> Result<Loaded, Failure> {
        let mut loaded =
            laft_config::load(&self.config_dir(), packages, options).map_err(Failure::config)?;
        if !loaded.warnings.is_empty() && !self.ui.json {
            crate::report::emit(&self.ui, &loaded.warnings);
        }
        let root = fs::canonicalize(self.root.path()).unwrap_or_else(|_| self.root.path().into());
        as_seen_from(&root, &mut loaded);
        Ok(loaded)
    }

    /// Loads the configuration and the indexes of its repositories,
    /// syncing them as `mode` says under `lock`, and checks every package
    /// reference against the indexes.
    ///
    /// The first pass type checks the configuration against the cached
    /// option declarations. When that fails, the repositories are read
    /// with a prelude that accepts any package assignment, the indexes
    /// are synced as `mode` says, and the configuration is loaded again
    /// with their declarations. A failure of the second load is reported
    /// as authoritative, with a hint about the cache, because the cached
    /// declarations were the reason for the detour.
    pub(crate) fn load_with_indexes(
        &self,
        mode: SyncMode,
        lock: Option<&Lock>,
    ) -> Result<(Loaded, Vec<RepoIndex>), Failure> {
        let started = std::time::Instant::now();
        let (loaded, indexes) = match self.load(&|_| Ok(())) {
            Ok(first) => {
                let indexes = self.indexes(&first.config.repos, mode, lock)?;
                let options = options_of(&indexes);
                let loaded = self.load_with(&options, &|pkg| known(&indexes, pkg))?;
                (loaded, indexes)
            }
            Err(first) => {
                let repos = match laft_config::repos(&self.config_dir()) {
                    Ok(repos) => repos,
                    Err(_) => return Err(first),
                };
                let indexes = self.indexes(&repos, mode, lock)?;
                let options = options_of(&indexes);
                let loaded = self
                    .load_with(&options, &|pkg| known(&indexes, pkg))
                    .map_err(|failure| {
                        failure.hint(format!(
                            "the cached option declarations in {} may be stale; \
                             `laft repo sync` refreshes them",
                            self.path("/laft/cache/index").display()
                        ))
                    })?;
                (loaded, indexes)
            }
        };
        let files = loaded.files.len();
        self.ui.phase(
            "evaluating",
            format!(
                "{}  {files} file{}  {}",
                self.config.display(),
                if files == 1 { "" } else { "s" },
                ui::duration(started.elapsed())
            ),
        );
        self.ui.event(
            json!({"event": "phase", "phase": "evaluate", "status": "done",
            "ms": started.elapsed().as_millis() as u64}),
        );
        Ok((loaded, indexes))
    }

    /// The index state records.
    pub(crate) fn index_state(&self) -> IndexState {
        IndexState::new(self.path("/laft/state/index"))
    }

    /// The fetcher, with its cache below the root.
    pub(crate) fn fetcher(&self) -> Fetcher {
        let mut config = FetchConfig::new(self.path("/laft/cache"));
        config.offline = self.offline;
        Fetcher::new(config)
    }

    /// The option declarations and profile values the cached indexes
    /// publish, as the first guess a load type checks the configuration
    /// against.
    ///
    /// The scan takes every repository directory and every channel and
    /// profile record below the cache, with no filter against the
    /// configuration, so entries of repositories the configuration no
    /// longer lists and of channels or profiles it no longer follow keep
    /// contributing. Repositories are read in name order and the
    /// declarations of the later one win. The commands that rely on the
    /// guess are the first type check of every command that loads indexes
    /// (`laft check`, `laft plan`, `laft apply`, `laft upgrade`,
    /// `laft view build`), the `laft why` lookups that locate a file
    /// block, and `laft options`. A load that syncs replaces the guess
    /// with the declarations of the indexes it just accepted, and a cache
    /// that cannot be read contributes nothing.
    pub(crate) fn cached_options(&self) -> IndexOptions {
        let mut out = IndexOptions::default();
        let state = self.index_state();
        let cache = self.path("/laft/cache/index");
        let Ok(repos) = fs::read_dir(&cache) else {
            return out;
        };
        let mut names: Vec<String> = repos
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        for name in names {
            let Ok(name) = name.parse::<Name>() else {
                continue;
            };
            let Ok(records) = state.index_records(&name) else {
                continue;
            };
            let mut records: Vec<&IndexRecord> = records
                .iter()
                .filter(|record| record.arch == ARCH && record.blake3.is_some())
                .collect();
            records.sort_by(|a, b| (&a.channel, &a.profile).cmp(&(&b.channel, &b.profile)));
            for record in records {
                let path = cache
                    .join(name.as_str())
                    .join("channels")
                    .join(record.channel.as_str())
                    .join(ARCH.as_str())
                    .join("index.json.zst");
                let Some(index) = cached_index(&path, record) else {
                    continue;
                };
                for (pkg, decls) in &index.options {
                    out.options.insert(pkg.clone(), decls.clone());
                }
                if let Some(profile) = &index.profile {
                    for (pkg, values) in &profile.values {
                        out.profile.insert(pkg.clone(), values.clone());
                    }
                }
            }
        }
        out
    }

    /// The accepted index of every enabled repository of `repos`, synced
    /// as `mode` says. Syncing needs `lock`. Without it, only the cache is
    /// read.
    pub(crate) fn indexes(
        &self,
        repos: &BTreeMap<Name, Repo>,
        mode: SyncMode,
        lock: Option<&Lock>,
    ) -> Result<Vec<RepoIndex>, Failure> {
        let mut out = self.indexes_of(repos, mode, lock, &|_| true)?;
        if let Some(local) = crate::local::Local::of(self.root.path()).source()? {
            out.push(local);
        }
        Ok(out)
    }

    /// As [`Self::indexes`], for the repositories `wanted` selects.
    pub(crate) fn indexes_of(
        &self,
        repos: &BTreeMap<Name, Repo>,
        mode: SyncMode,
        lock: Option<&Lock>,
        wanted: &dyn Fn(&Name) -> bool,
    ) -> Result<Vec<RepoIndex>, Failure> {
        let fetcher = self.fetcher();
        let state = self.index_state();
        let mut out = Vec::new();
        let now = Timestamp::now();
        for (name, repo) in repos.iter().filter(|(n, r)| r.enable && wanted(n)) {
            let mirrors = Mirrors::new(&repo.url, &repo.mirrors, repo.allow_http)?;
            let trust = laft_index::Repo {
                name: name.clone(),
                channel: repo.channel.clone(),
                profile: repo.profile.clone(),
                arch: ARCH,
                key: repo.key.clone(),
                keys: repo.keys.clone(),
            };
            let request = SyncRequest {
                repo: &trust,
                mirrors: &mirrors,
                state: &state,
                now,
                allow_expired: false,
            };
            let cached = fetcher.cached(&request).ok();
            let stale = cached.as_ref().is_none_or(|c| {
                SystemTime::now()
                    .duration_since(c.synced)
                    .is_ok_and(|age| age > STALE)
            });
            let sync = match mode {
                SyncMode::Cached => false,
                SyncMode::IfStale => stale,
                SyncMode::Always => true,
            };
            let index = match (sync, lock, cached) {
                (true, Some(_), _) => {
                    let synced = fetcher.sync(&request, &mut |_| {})?;
                    let (mirrors, note) = match &synced.source {
                        Source::Mirror { base, .. } => (mirrors.preferring(base), None),
                        Source::Cache { failure } => (mirrors.clone(), Some(failure.to_string())),
                    };
                    if let Some(note) = note {
                        self.ui.warn(format!(
                            "repo {name}: using the cached index, the sync failed: {note}"
                        ));
                    }
                    let generation = synced.index.record.generation;
                    self.ui.phase(
                        "syncing",
                        format!(
                            "{name}  generation {generation} ({} old)",
                            ui::age(synced.index.index.timestamp, now)
                        ),
                    );
                    RepoIndex {
                        name: name.clone(),
                        repo: repo.clone(),
                        accepted: synced.index,
                        mirrors,
                        synced: Some(SystemTime::now()),
                    }
                }
                (_, _, Some(cached)) => {
                    if sync {
                        self.ui.warn(format!(
                            "repo {name}: the cached index is used, because syncing needs root"
                        ));
                    }
                    RepoIndex {
                        name: name.clone(),
                        repo: repo.clone(),
                        accepted: cached.index,
                        mirrors,
                        synced: Some(cached.synced),
                    }
                }
                (_, _, None) => {
                    return Err(Failure::new(
                        exit::NETWORK,
                        format!("repo {name}: no index has been synced yet"),
                    )
                    .hint("run `laft repo sync` as root"));
                }
            };
            out.push(index);
        }
        Ok(out)
    }

    /// The active system generation: the target of `run/laft/system`, or
    /// the default generation when nothing runs, as below `--root`.
    pub(crate) fn active(&self) -> Result<Option<u64>, Failure> {
        if !self.offline_root
            && let Some(number) = link_number(&self.path("/run/laft/system"))?
        {
            return Ok(Some(number));
        }
        Ok(self.root.default_generation()?)
    }

    /// The booted generation, the target of `run/laft/booted`.
    pub(crate) fn booted(&self) -> Result<Option<u64>, Failure> {
        link_number(&self.path("/run/laft/booted"))
    }

    /// Whether this command runs on a live system that it activates in
    /// place, rather than below `--root`.
    pub(crate) fn live(&self) -> bool {
        !self.offline_root && self.path("/run/laft/system").symlink_metadata().is_ok()
    }

    /// The generation `number`, or a failure that names it.
    pub(crate) fn generation(&self, number: u64) -> Result<laft_store::Generation, Failure> {
        self.root.generation(number).map_err(|e| {
            Failure::general(format!(
                "generation {number} does not exist or is incomplete"
            ))
            .context(e.to_string())
        })
    }
}

/// The `boot.esp` that the configuration of the default generation, or of
/// the newest one when there is no default, records.
fn recorded_esp(root: &Root) -> Option<String> {
    let number = match root.default_generation().ok()? {
        Some(number) => number,
        None => *root.generations().ok()?.last()?,
    };
    let path = root
        .path()
        .join("laft/gens")
        .join(number.to_string())
        .join("config.json");
    let config: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    config["boot"]["esp"].as_str().map(str::to_owned)
}

/// Whether some index lists the package `pkg` names, for the
/// configuration's reference check.
fn known(indexes: &[RepoIndex], pkg: &PkgRef) -> Result<(), PkgError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let name = pkg.name();
    let listed = indexes.iter().any(|r| {
        r.accepted
            .index
            .packages
            .iter()
            .any(|e| &e.meta.name == name)
            || r.accepted.index.yanked.iter().any(|y| &y.name == name)
    });
    if listed {
        return Ok(());
    }
    let close = indexes
        .iter()
        .flat_map(|r| {
            r.accepted
                .index
                .packages
                .iter()
                .map(|e| e.meta.name.as_str())
        })
        .filter(|candidate| similar(candidate, name.as_str()))
        .min();
    Err(PkgError {
        message: format!("no package `{name}` in the index"),
        help: close.map(|c| format!("did you mean `{c}`?")),
    })
}

/// Whether two names differ by at most one edit.
fn similar(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + cost);
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()] <= 1
}

/// Names the configuration files of `loaded`, and the places of its
/// package requests, as the system mounted at `root` sees them, so that a
/// generation built below `--root /mnt` records `/etc/laft/system.stav`
/// rather than `/mnt/etc/laft/system.stav`.
fn as_seen_from(root: &Path, loaded: &mut Loaded) {
    let inside = |text: &mut String| {
        if let Some(rest) = seen_from(root, text) {
            *text = rest;
        }
    };
    loaded.files.iter_mut().for_each(|f| inside(&mut f.path));
    let config = &mut loaded.config;
    let views = config
        .views
        .values_mut()
        .flat_map(|v| v.packages.iter_mut());
    for request in config.packages.iter_mut().chain(views) {
        inside(&mut request.origin);
    }
}

/// The path `path`, or a location that starts with one, as the system
/// mounted at `root` sees it, or `None` when it is not below `root` or the
/// root is `/`.
fn seen_from(root: &Path, path: &str) -> Option<String> {
    let root = root.to_string_lossy();
    if root == "/" {
        return None;
    }
    path.strip_prefix(root.as_ref())
        .filter(|rest| rest.starts_with('/'))
        .map(str::to_owned)
}

/// The number at the end of the target of the symlink `link`, such as
/// `mounts/42`, or `None` when it does not exist.
pub(crate) fn link_number(link: &Path) -> Result<Option<u64>, Failure> {
    match fs::read_link(link) {
        Ok(target) => target
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse().ok())
            .map(Some)
            .ok_or_else(|| {
                Failure::general(format!(
                    "{} does not name a generation: {}",
                    link.display(),
                    target.display()
                ))
            }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Failure::general(format!("{}: {e}", link.display()))),
    }
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
