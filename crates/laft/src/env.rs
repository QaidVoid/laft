//! What every command works with: the root, the configuration, the lock,
//! the repository indexes and the generations.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use laft_config::{Loaded, PkgError, Repo, SystemConfig};
use laft_core::{Name, PkgRef, Timestamp};
use laft_fetch::{Config as FetchConfig, Fetcher, Mirrors, Source, SyncRequest};
use laft_index::{AcceptedIndex, IndexState};
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
        let recovered = laft_activate::recover(&lock, &self.esp_dir())?;
        for recovery in recovered {
            self.ui.note(recovery.to_string());
        }
        Ok(lock)
    }

    /// The directory holding the configuration.
    pub(crate) fn config_dir(&self) -> PathBuf {
        let dir = self.config.parent().unwrap_or(Path::new("/"));
        fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
    }

    /// The EFI system partition: `/boot` below the root.
    pub(crate) fn esp_dir(&self) -> PathBuf {
        self.path("/boot")
    }

    /// The EFI system partition the configuration names.
    pub(crate) fn esp(&self, config: &SystemConfig) -> PathBuf {
        self.path(&config.boot.boot.esp)
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
        let loaded = laft_config::load(&self.config_dir(), packages).map_err(Failure::config)?;
        if !loaded.warnings.is_empty() && !self.ui.json {
            crate::report::emit(&self.ui, &loaded.warnings);
        }
        Ok(loaded)
    }

    /// Loads the configuration and the indexes of its repositories,
    /// syncing them as `mode` says under `lock`, and checks every package
    /// reference against the indexes.
    pub(crate) fn load_with_indexes(
        &self,
        mode: SyncMode,
        lock: Option<&Lock>,
    ) -> Result<(Loaded, Vec<RepoIndex>), Failure> {
        let started = std::time::Instant::now();
        let first = self.load(&|_| Ok(()))?;
        let indexes = self.indexes(&first.config, mode, lock)?;
        let loaded = self.load(&|pkg| known(&indexes, pkg))?;
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

    /// The accepted index of every enabled repository of `config`, synced
    /// as `mode` says. Syncing needs `lock`. Without it, only the cache is
    /// read.
    pub(crate) fn indexes(
        &self,
        config: &SystemConfig,
        mode: SyncMode,
        lock: Option<&Lock>,
    ) -> Result<Vec<RepoIndex>, Failure> {
        self.indexes_of(config, mode, lock, &|_| true)
    }

    /// As [`Self::indexes`], for the repositories `wanted` selects.
    pub(crate) fn indexes_of(
        &self,
        config: &SystemConfig,
        mode: SyncMode,
        lock: Option<&Lock>,
        wanted: &dyn Fn(&Name) -> bool,
    ) -> Result<Vec<RepoIndex>, Failure> {
        let fetcher = self.fetcher();
        let state = self.index_state();
        let mut out = Vec::new();
        let now = Timestamp::now();
        for (name, repo) in config.repos.iter().filter(|(n, r)| r.enable && wanted(n)) {
            let mirrors = Mirrors::new(&repo.url, &repo.mirrors, repo.allow_http)?;
            let trust = laft_index::Repo {
                name: name.clone(),
                channel: repo.channel.clone(),
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
