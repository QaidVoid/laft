//! Building recipes: sources, build view, phases, post-processing,
//! packages and buildinfo, and the run that feeds every result back into
//! the schedule.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use laft_core::{Blake3Hash, Name, PkgRef, StoreId, Timestamp, Variant};
use laft_pkg::Compression;
use laft_store::Store;

use crate::db::{Db, Stored};
use crate::error::{BuildError, Warning, io};
use crate::exec::{DEFAULT_TIMEOUT, Executor};
use crate::fsx::{self, Kind};
use crate::post::{self, Package};
use crate::view::{self, BuildView, StoreEntry};
use crate::{
    BuildGraph, BuildRecord, Buildinfo, BuildinfoHost, Built, Cutoff, Fetch, Item, KeyInputs,
    Output, Problem, Recipe, Repos, Schedule, Source, SourceCache, Split, Trigger, env, unpack,
};

/// The directory a builder keeps everything in, `$LAFT_BUILD_ROOT`:
///
/// | Path | Contents |
/// |---|---|
/// | `store/` | the build-side store, with every package builds may use |
/// | `seeds/<id>` | marks the store entry `<id>` as a seed, see [`crate::seed()`] |
/// | `sources/sha256/<hex>` | sources and vendor tarballs, by hash |
/// | `cache/` | the download cache |
/// | `db/` | the [`Db`] of finished builds |
/// | `out/` | package files and buildinfo |
/// | `work/<key>/` | the working tree of a build, removed after success |
/// | `logs/<name>+<variant>/<key>/<phase>.log` | the output of every phase |
/// | `tmp/` | staging for the store |
/// | `stages/<stage>/<step>` | the records of a stage that builds into the store |
/// | `lock` | the lock that every run that writes here holds, see [`BuildRoot::lock`] |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRoot {
    path: PathBuf,
}

impl BuildRoot {
    /// The build root at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The build-side store.
    pub fn store(&self) -> Store {
        Store::at(self.path.join("store"))
    }

    /// The source cache directory.
    pub fn sources(&self) -> PathBuf {
        self.path.join("sources")
    }

    /// The download cache directory.
    pub fn cache(&self) -> PathBuf {
        self.path.join("cache")
    }

    /// The build database.
    pub fn db(&self) -> Db {
        Db::new(self.path.join("db"))
    }

    /// The output directory.
    pub fn out(&self) -> PathBuf {
        self.path.join("out")
    }

    /// The directory of working trees.
    pub fn work(&self) -> PathBuf {
        self.path.join("work")
    }

    /// The directory of logs.
    pub fn logs(&self) -> PathBuf {
        self.path.join("logs")
    }

    /// The staging directory of the store.
    pub fn tmp(&self) -> PathBuf {
        self.path.join("tmp")
    }

    /// The directory that records the seeds of the store.
    pub fn seeds(&self) -> PathBuf {
        self.path.join("seeds")
    }
}

/// Where a build's tools come from and how isolated it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// The normal mode: each build view is assembled from the build-side
    /// store, every phase runs in a sandbox, and the packages a build makes
    /// enter the store for later builds.
    Store,
    /// Every phase runs in a sandbox whose `/usr` is this `usr` tree,
    /// prepared by the caller, instead of a view from the store. Nothing
    /// enters the store, and the buildinfo lists no build view. The command
    /// line never uses it. It serves callers that bring their own tools,
    /// such as tests that build without a populated store.
    View(PathBuf),
    /// Bootstrap mode: every phase runs directly on the host with the tools
    /// of `<root>/usr/bin`, as [`Executor::direct`] describes. Such a build
    /// is not hermetic, its buildinfo says `bootstrap`, and its packages
    /// must never be published. Nothing enters the store, and checks and
    /// smoke tests are skipped.
    Bootstrap(PathBuf),
}

/// One build in progress: its working tree, the executor of its phases and
/// their environment.
#[derive(Debug)]
pub struct Session {
    dir: PathBuf,
    build: PathBuf,
    out: PathBuf,
    exec: Executor,
    env: BTreeMap<String, String>,
    view: Option<BuildView>,
}

impl Session {
    /// The working tree, `work/<key>`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The executor of the build's commands.
    pub fn exec(&self) -> &Executor {
        &self.exec
    }

    /// The environment of the phases.
    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }
}

/// A finished build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// What the build database keeps.
    pub stored: Stored,
    /// The buildinfo, written next to the packages.
    pub buildinfo: Buildinfo,
    /// What the schedule needs to decide about dependents.
    pub built: Built,
    /// The package files, the main package first.
    pub packages: Vec<PathBuf>,
    /// What post-processing reported.
    pub warnings: Vec<Warning>,
}

/// A step of [`Builder::run`], for progress reports.
#[derive(Debug)]
pub enum Event<'a> {
    /// A build starts.
    Start {
        /// What is built.
        item: &'a Item,
        /// Why.
        triggers: &'a BTreeSet<Trigger>,
    },
    /// A build finished.
    Done {
        /// What was built.
        item: &'a Item,
        /// The result.
        outcome: &'a Outcome,
        /// What it means for the dependents.
        cutoff: &'a Cutoff,
    },
}

/// Builds recipes into packages.
///
/// A build runs these steps:
///
/// 1. A fresh working tree `work/<key>` gets `build/{src,obj,tmp,home}`,
///    `out` and, for a `static` build, the cross files in `build/cross`.
/// 2. Every source is fetched into the source cache, with the hosts
///    network but outside any sandbox, verified and unpacked into
///    `build/src`. The recipe directories and `common/` directories of the
///    contributing repositories are copied read-only to `build/recipes` and
///    `build/common`, and every file the recipe names must still have the
///    hash it had when the recipe was evaluated.
/// 3. The build view is assembled, see [`Mode`]. Vendor tarballs that the
///    cache lacks are produced in a sandbox with the host's network.
/// 4. The patches and the phases `prepare` to `post_install` run in the
///    sandbox without network, `check` only when both the recipe and the
///    builder enable it, each logged to its own file.
/// 5. The license files are installed into `usr/share/licenses/<name>/`
///    and [`post::process`] turns the output into packages, which are
///    written to the output directory with the buildinfo.
/// 6. In [`Mode::Store`], the packages enter the build-side store and the
///    recipe's smoke test runs in a view of the main package.
/// 7. The working tree is removed unless it is kept.
#[derive(Debug)]
pub struct Builder<'a> {
    /// The recipe repositories, for the files recipes name.
    pub repos: &'a Repos,
    /// The source cache.
    pub sources: &'a SourceCache,
    /// The build-side store.
    pub store: Store,
    /// The directory that records the seeds of the store, which stand in
    /// for packages until a build makes some of the same origin, as
    /// [`crate::seed()`] describes.
    pub seed_records: PathBuf,
    /// A staging directory on the store's filesystem.
    pub store_tmp: PathBuf,
    /// Where tools come from.
    pub mode: Mode,
    /// The directory of working trees.
    pub work: PathBuf,
    /// Where packages and buildinfo are written.
    pub out: PathBuf,
    /// The directory of logs.
    pub logs: PathBuf,
    /// Source mirrors tried after the repositories' own.
    pub mirrors: Vec<String>,
    /// The number of parallel jobs, `$JOBS`.
    pub jobs: u32,
    /// Whether check phases run.
    pub check: bool,
    /// Whether working trees are kept after a successful build.
    pub keep_work: bool,
    /// The longest a phase may run.
    pub timeout: Duration,
    /// How package files are compressed.
    pub compression: Compression,
    /// Whether the packages of a [`Mode::Store`] build enter the store. A
    /// rebuild that only checks reproducibility leaves the store alone.
    pub add_to_store: bool,
    /// The store ids of the packages a [`Mode::Store`] build view holds,
    /// as [`view::assemble`] describes. A reproducibility check pins the
    /// view of the build it repeats.
    pub pinned_view: Option<BTreeSet<StoreId>>,
    /// Directories that [`Mode::Bootstrap`] phases search for programs
    /// before the root's `usr/bin`, as [`Executor::search_first`]
    /// describes.
    pub search: Vec<PathBuf>,
}

impl<'a> Builder<'a> {
    /// A builder that keeps everything in `root`, with checks enabled, one
    /// job per CPU, the default timeout and development compression.
    pub fn new(repos: &'a Repos, sources: &'a SourceCache, root: &BuildRoot, mode: Mode) -> Self {
        let jobs = std::thread::available_parallelism().map_or(1, |n| n.get());
        Self {
            repos,
            sources,
            store: root.store(),
            seed_records: root.seeds(),
            store_tmp: root.tmp(),
            mode,
            work: root.work(),
            out: root.out(),
            logs: root.logs(),
            mirrors: Vec::new(),
            jobs: u32::try_from(jobs).unwrap_or(u32::MAX),
            check: true,
            keep_work: false,
            timeout: DEFAULT_TIMEOUT,
            compression: Compression::DEVELOPMENT,
            add_to_store: true,
            pinned_view: None,
            search: Vec::new(),
        }
    }

    /// The build key inputs of `recipe`. In [`Mode::Store`] they come from
    /// the link dependencies in the store, as [`view::key_inputs`]
    /// describes. Otherwise no dependency is resolved and the inputs hold
    /// only what the recipe itself says.
    ///
    /// # Errors
    ///
    /// Fails when the store or the seeds cannot be read.
    pub fn key_inputs(&self, recipe: &Recipe) -> Result<KeyInputs, BuildError> {
        match self.mode {
            Mode::Store => view::key_inputs(recipe, &self.store, &self.seeds()?),
            _ => Ok(KeyInputs::new(recipe, Vec::new(), Vec::new())),
        }
    }

    /// The store ids of the seeds of the store.
    ///
    /// # Errors
    ///
    /// Fails when their records cannot be read.
    pub fn seeds(&self) -> Result<BTreeSet<StoreId>, BuildError> {
        crate::seed::seeds(&self.seed_records)
    }

    /// Plans a run of `recipes`: the build graph and the schedule of the
    /// items whose build key changed since their latest build in `db`, or
    /// that are `forced`. Dependents of `use_published` items use their
    /// published packages instead of waiting for them.
    ///
    /// In [`Mode::Store`], a dependency that no recipe of the run builds
    /// must have a package in the store. In the other modes nothing is
    /// resolved.
    ///
    /// # Errors
    ///
    /// Fails with the problems of [`BuildGraph::new`] and [`Schedule::new`],
    /// and when the store or the database cannot be read.
    pub fn plan(
        &self,
        recipes: Vec<Recipe>,
        db: &Db,
        forced: &BTreeSet<Item>,
        use_published: &BTreeSet<Item>,
    ) -> Result<(BuildGraph, Schedule), BuildError> {
        let entries = match self.mode {
            Mode::Store => view::usable(view::store_entries(&self.store)?, &self.seeds()?),
            _ => Vec::new(),
        };
        let names: BTreeSet<(Name, Variant)> = entries
            .iter()
            .map(|e| (e.meta.name.clone(), e.meta.variant))
            .collect();
        let unresolved = !matches!(self.mode, Mode::Store);
        let published = |r: &PkgRef| unresolved || names.contains(&(r.name().clone(), r.variant()));
        let graph = BuildGraph::new(recipes, &published).map_err(problems)?;
        let mut dirty = BTreeMap::new();
        let mut previous = BTreeMap::new();
        for item in graph.items() {
            let Some(recipe) = graph.recipe(item) else {
                continue;
            };
            let inputs = match self.mode {
                Mode::Store => view::key_inputs_from(recipe, &entries),
                _ => self.key_inputs(recipe)?,
            };
            let latest = db.latest(item)?.map(|s| s.record);
            dirty.insert(
                item.clone(),
                inputs.triggers(latest.as_ref(), forced.contains(item)),
            );
            if let Some(latest) = latest {
                previous.insert(item.clone(), latest);
            }
        }
        let schedule = Schedule::new(&graph, dirty, previous, use_published)?;
        Ok((graph, schedule))
    }

    /// Builds every item `schedule` hands out, records each in `db`, and
    /// feeds its outputs back with [`Schedule::finish`], so that unchanged
    /// outputs stop at once and changed ones mark the dependents they
    /// affect. Stops at the first failure.
    ///
    /// # Errors
    ///
    /// Fails like [`Builder::build`], and when the database cannot be
    /// written.
    ///
    /// # Panics
    ///
    /// Panics when `schedule` hands out an item that `graph` does not hold,
    /// which never happens for a schedule made from that graph.
    pub fn run(
        &self,
        graph: &BuildGraph,
        schedule: &mut Schedule,
        db: &Db,
        progress: &mut dyn FnMut(Event<'_>),
    ) -> Result<Vec<Outcome>, BuildError> {
        let mut outcomes = Vec::new();
        while let Some(item) = schedule.next_build() {
            let recipe = graph
                .recipe(&item)
                .expect("the schedule only hands out items of its graph");
            let triggers = schedule.triggers(&item).cloned().unwrap_or_default();
            progress(Event::Start {
                item: &item,
                triggers: &triggers,
            });
            let inputs = self.key_inputs(recipe)?;
            let outcome = self.build(recipe, &inputs)?;
            db.record(&outcome.stored)?;
            let cutoff = schedule.finish(&item, &outcome.built);
            progress(Event::Done {
                item: &item,
                outcome: &outcome,
                cutoff: &cutoff,
            });
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    /// Fetches every URL source of `recipe` into the source cache and
    /// returns the cached files, in label order.
    ///
    /// # Errors
    ///
    /// Fails like [`SourceCache::fetch_source`].
    pub fn fetch(&self, recipe: &Recipe) -> Result<Vec<PathBuf>, BuildError> {
        let mirrors = self.mirrors_of(recipe);
        recipe
            .sources
            .iter()
            .map(|s| Ok(self.sources.fetch_source(recipe, s, &mirrors)?))
            .collect()
    }

    fn mirrors_of(&self, recipe: &Recipe) -> Vec<String> {
        let own = self
            .repos
            .repos()
            .iter()
            .filter(|r| recipe.repos.contains(&r.name))
            .flat_map(|r| r.source_mirrors.iter().cloned());
        own.chain(self.mirrors.iter().cloned()).collect()
    }

    /// Builds `recipe`, whose build key covers `inputs`, as the type
    /// documentation describes.
    ///
    /// # Errors
    ///
    /// Fails with the code of the step that failed: `B02x` for sources,
    /// `B030` for a phase that ran out of time, `B040` for a phase that
    /// failed, `B050` for a view that cannot be resolved, and the codes of
    /// [`post`]. A failed build keeps its working tree.
    pub fn build(&self, recipe: &Recipe, inputs: &KeyInputs) -> Result<Outcome, BuildError> {
        let started = Timestamp::now();
        let session = self.session(recipe, &inputs.build_key())?;
        self.phases(&session, recipe, None)?;
        let outcome = self.finish(&session, recipe, inputs, started)?;
        if !self.keep_work {
            fsx::remove_tree(&session.dir)?;
        }
        Ok(outcome)
    }

    /// Builds `recipe`, whose build key covers `inputs`, in bootstrap mode
    /// and installs it straight into `dest`, a system under construction,
    /// instead of making packages. The record of an earlier install goes
    /// before the first phase runs. Once every phase succeeded, the build
    /// key is recorded in `dest`, where [`installed`] finds it, and the
    /// working tree is removed unless it is kept.
    ///
    /// The phases run like those of a bootstrap build, except that
    /// `$DESTDIR` and `$OUT` are `dest` and that the environment names none
    /// of the compilers, binutils, pkg-config, sysroot or compiler flags of
    /// Laft's toolchain. A system under construction is built with other
    /// tools, so its build systems choose their own, for example from
    /// `--host`. The Go and Cargo settings stay, and so do the cross files
    /// of a `static` recipe, which name the musl cross toolchain. No
    /// license files are added, nothing is post-processed, and no package
    /// or buildinfo is written.
    ///
    /// # Errors
    ///
    /// Fails with `B041` outside [`Mode::Bootstrap`], and otherwise like
    /// [`Builder::build`] for the steps they share. A failed build keeps its
    /// working tree and may leave some of its files in `dest`.
    pub fn install(
        &self,
        recipe: &Recipe,
        inputs: &KeyInputs,
        dest: &Path,
    ) -> Result<(), BuildError> {
        let Mode::Bootstrap(root) = &self.mode else {
            return Err(BuildError::problem(
                "B041",
                "only bootstrap mode installs into a directory",
            ));
        };
        fs::create_dir_all(dest).map_err(io(dest))?;
        let key = inputs.build_key();
        let mut session = self.session(recipe, &key)?;
        session.exec = Executor::direct(root, &session.build, dest, session.exec.logs())
            .search_first(self.search.clone())
            .timeout(self.timeout)
            .shell(recipe.shell);
        session.env = env::install(recipe.environment(self.jobs));
        let record = installed_record(dest, &Item::of(recipe));
        forget_key(&record)?;
        self.phases(&session, recipe, None)?;
        write_key(&record, &key)?;
        if !self.keep_work {
            fsx::remove_tree(&session.dir)?;
        }
        Ok(())
    }

    /// Builds `recipe`, whose build key covers `inputs`, in bootstrap mode
    /// like [`Builder::build`], then unpacks the packages it made over
    /// `dest`, a system under construction, so that later builds there use
    /// them. The automatic debug info package, `<name>-dbg`, stays out,
    /// because no build needs it. A `dbg` subpackage that the recipe
    /// declares itself is unpacked like any other. The record of an earlier
    /// unpack goes before the first package is unpacked. Once every package
    /// is in place, the build key is recorded in `dest`, where [`installed`]
    /// finds it.
    ///
    /// Each package is unpacked into a staging directory inside `dest` and
    /// moved into place from there. A directory that `dest` lacks is made
    /// with the mode, owner and group the package declares, an existing one
    /// is kept, and every file and symbolic link replaces what is at its
    /// path with a rename. Symbolic links in `dest` are never followed.
    ///
    /// # Errors
    ///
    /// Fails with `B041` outside [`Mode::Bootstrap`], like
    /// [`Builder::build`] for the build, with `B043` when a payload path is
    /// a directory in a package and something else in `dest` or the other
    /// way round, and with [`BuildError::Package`] for a package that does
    /// not match its manifest. A failed unpack may leave some files of the
    /// package in `dest`.
    pub fn build_into(
        &self,
        recipe: &Recipe,
        inputs: &KeyInputs,
        dest: &Path,
    ) -> Result<Outcome, BuildError> {
        if !matches!(self.mode, Mode::Bootstrap(_)) {
            return Err(BuildError::problem(
                "B041",
                "only bootstrap mode unpacks packages into a directory",
            ));
        }
        let outcome = self.build(recipe, inputs)?;
        let record = installed_record(dest, &Item::of(recipe));
        forget_key(&record)?;
        let outputs = &recipe.outputs;
        let debug = (outputs.split.contains(&Split::Dbg)
            && !outputs.subpackages.contains_key("dbg"))
        .then(|| format!("{}-dbg", recipe.name));
        for (output, file) in outcome.buildinfo.outputs.iter().zip(&outcome.packages) {
            if debug.as_deref() != Some(output.name.as_str()) {
                crate::deploy::unpack_into(file, dest)?;
            }
        }
        write_key(&record, &inputs.build_key())?;
        Ok(outcome)
    }

    /// Prepares the build of `recipe` with the build key `key`: the working
    /// tree, the sources, the recipe files, the build view and the vendor
    /// trees. An existing working tree of the same key is replaced.
    ///
    /// # Errors
    ///
    /// Fails like [`Builder::build`].
    pub fn session(&self, recipe: &Recipe, key: &Blake3Hash) -> Result<Session, BuildError> {
        let key_hash = *key;
        let key = key.to_string();
        let key = key.trim_start_matches("b3:");
        let dir = self.work_dir(&key_hash);
        fsx::remove_tree(&dir)?;
        let build = dir.join("build");
        let out = dir.join("out");
        let scratch = dir.join("scratch");
        for sub in [
            "src", "obj", "tmp", "home", "vendor", "cross", "recipes", "common",
        ] {
            let path = build.join(sub);
            fs::create_dir_all(&path).map_err(io(&path))?;
        }
        for path in [&out, &scratch] {
            fs::create_dir_all(path).map_err(io(path))?;
        }
        let logs = self.logs.join(item_dir(&Item::of(recipe))).join(key);
        fsx::remove_tree(&logs)?;

        for (source, file) in recipe.sources.iter().zip(self.fetch(recipe)?) {
            unpack(source, &file, &build.join("src"), &scratch)?;
        }
        self.copy_recipe_files(recipe, &build)?;
        if recipe.variant == Variant::Static {
            write_cross_files(&build.join("cross"))?;
        }

        let check = self.check && recipe.options.check;
        let view = match &self.mode {
            Mode::Store => Some(view::assemble(
                &self.store,
                &self.seeds()?,
                recipe,
                check,
                &dir.join("root"),
                self.pinned_view.as_ref(),
            )?),
            _ => None,
        };
        let exec = self.executor(recipe, &key_hash);
        let env = recipe.environment(self.jobs);
        for vendor in &recipe.vendors {
            let what = format!("{} {}", recipe.name, recipe.version);
            let file = crate::vendor::vendor(&what, vendor, &exec, &env, &out, self.sources)?;
            let source = Source {
                label: format!("vendor-{}", vendor.kind.as_str()),
                fetch: Fetch::Url {
                    urls: Vec::new(),
                    sha256: vendor.sha256,
                    file: "vendor.tar".into(),
                    extract: true,
                },
                dest: vendor.kind.as_str().into(),
                strip_components: Some(0),
            };
            unpack(&source, &file, &build.join("vendor"), &scratch)?;
        }
        fsx::remove_tree(&scratch)?;
        Ok(Session {
            dir,
            build,
            out,
            exec,
            env,
            view,
        })
    }

    /// The working tree of the build of `recipe` with the build key `key`,
    /// `work/<key>`, which a failed build leaves behind.
    pub fn work_dir(&self, key: &Blake3Hash) -> PathBuf {
        self.work.join(key.to_string().trim_start_matches("b3:"))
    }

    /// The executor of the build of `recipe` with the build key `key`, for
    /// its working tree as [`Builder::session`] lays it out. It can enter
    /// the tree a failed build left behind.
    pub fn executor(&self, recipe: &Recipe, key: &Blake3Hash) -> Executor {
        let dir = self.work_dir(key);
        let (build, out) = (dir.join("build"), dir.join("out"));
        let key = key.to_string();
        let logs = self
            .logs
            .join(item_dir(&Item::of(recipe)))
            .join(key.trim_start_matches("b3:"));
        let exec = match &self.mode {
            Mode::Store => Executor::sandboxed(dir.join("root/usr"), build, out, logs),
            Mode::View(usr) => Executor::sandboxed(usr, build, out, logs),
            Mode::Bootstrap(root) => {
                Executor::direct(root, build, out, logs).search_first(self.search.clone())
            }
        };
        exec.timeout(self.timeout).shell(recipe.shell)
    }

    /// Copies the recipe directory and the `common/` directory of every
    /// repository that contributed to `recipe` below `build`, read-only,
    /// and checks that every file the recipe names is unchanged.
    fn copy_recipe_files(&self, recipe: &Recipe, build: &Path) -> Result<(), BuildError> {
        for repo in self.repos.repos() {
            if !recipe.repos.contains(&repo.name) {
                continue;
            }
            let dir = repo.path.join("recipes").join(recipe.name.as_str());
            if dir.is_dir() {
                let to = build
                    .join("recipes")
                    .join(repo.name.as_str())
                    .join(recipe.name.as_str());
                fsx::copy_tree(&dir, &to)?;
            }
            let common = repo.path.join("common");
            if common.is_dir() {
                fsx::copy_tree(&common, &build.join("common").join(repo.name.as_str()))?;
            }
        }
        for file in recipe.patches.iter().chain(&recipe.files) {
            let rel = file.path.strip_prefix("/build/").unwrap_or(&file.path);
            let mut bytes = Vec::new();
            fsx::open_beneath(build, rel)
                .and_then(|mut f| f.read_to_end(&mut bytes))
                .map_err(|e| {
                    BuildError::problem("B023", format!("{} cannot be read: {e}", file.path))
                })?;
            if Blake3Hash::of(&bytes) != file.blake3 {
                return Err(BuildError::problem(
                    "B023",
                    format!("{} changed since the recipe was evaluated", file.path),
                ));
            }
        }
        for sub in ["recipes", "common"] {
            read_only(&build.join(sub))?;
        }
        Ok(())
    }

    /// Runs the patches and the phases of `recipe` in `session`, in order,
    /// stopping after the phase named `until` when it is given. The check
    /// phase runs only when the builder and the recipe enable it, never in
    /// bootstrap mode, and without `DESTDIR`, which only installing needs.
    ///
    /// # Errors
    ///
    /// Fails with `B040` or `B030` for the phase that failed.
    pub fn phases(
        &self,
        session: &Session,
        recipe: &Recipe,
        until: Option<&str>,
    ) -> Result<(), BuildError> {
        let mut steps: Vec<(&str, String)> = Vec::new();
        if !recipe.patches.is_empty() {
            let mut script = format!("{}cd \"$SRC\"\n", recipe.shell.header());
            for patch in &recipe.patches {
                script.push_str(&format!(
                    "patch -p{} -N --no-backup-if-mismatch -i {}\n",
                    recipe.patch_strip,
                    crate::style::quote(&patch.path)
                ));
            }
            steps.push(("patch", script));
        }
        let check = self.check && recipe.options.check && !matches!(self.mode, Mode::Bootstrap(_));
        for (name, script) in recipe.phases.iter() {
            if name == "check" && !check {
                continue;
            }
            if let Some(script) = script {
                steps.push((name, script.to_owned()));
            }
        }
        // A test suite that installs into a directory of its own would
        // otherwise install into the output of the build.
        let mut check_env = session.env.clone();
        check_env.remove("DESTDIR");
        for (name, script) in steps {
            let env = if name == "check" {
                &check_env
            } else {
                &session.env
            };
            session.exec.run(name, &script, env)?;
            if until == Some(name) {
                break;
            }
        }
        Ok(())
    }

    /// Post-processes the output of `session`, writes the packages and the
    /// buildinfo, and in [`Mode::Store`] adds the packages to the store and
    /// runs the smoke test.
    ///
    /// # Errors
    ///
    /// Fails like [`Builder::build`].
    pub fn finish(
        &self,
        session: &Session,
        recipe: &Recipe,
        inputs: &KeyInputs,
        started: Timestamp,
    ) -> Result<Outcome, BuildError> {
        install_licenses(recipe, &session.build.join("src"), &session.out)?;
        let statics = session
            .view
            .as_ref()
            .map(|v| v.statics.clone())
            .unwrap_or_default();
        let output = post::process(&post::Input {
            recipe,
            out: &session.out,
            exec: &session.exec,
            view_sonames: session.view.as_ref().map(|v| &v.sonames),
            view_statics: &statics,
        })?;
        let packages =
            post::write_packages(&session.out, &output.packages, &self.out, self.compression)?;
        if self.mode == Mode::Store && self.add_to_store {
            for (package, path) in output.packages.iter().zip(&packages) {
                let id = package.manifest.store_id();
                self.store.insert(path, &id, &self.store_tmp)?;
                crate::seed::forget(&self.seed_records, &id)?;
            }
            if let (Some(script), Some(main)) = (&recipe.smoke, output.packages.first()) {
                self.smoke(session, main, script)?;
            }
        }

        let key = inputs.build_key();
        let host = BuildinfoHost {
            laft_build: env!("CARGO_PKG_VERSION").to_owned(),
            kernel: kernel_release(),
            jobs: if recipe.options.parallel {
                self.jobs.max(1)
            } else {
                1
            },
            started,
            finished: Timestamp::now(),
        };
        let mut buildinfo = Buildinfo::for_recipe(recipe, key, host);
        buildinfo.build_view = session
            .view
            .as_ref()
            .map(|v| v.packages.clone())
            .unwrap_or_default();
        buildinfo.env = recipe.env.clone();
        buildinfo.outputs = output
            .packages
            .iter()
            .map(|p| Output {
                name: p.meta.name.clone(),
                variant: p.meta.variant,
                id: p.manifest.store_id(),
                file: p.manifest.package().file_name(),
            })
            .collect();
        buildinfo.abi = output.abi.clone();
        buildinfo.bootstrap = matches!(self.mode, Mode::Bootstrap(_));
        let buildinfo_name = buildinfo.file_name();
        fsx::write_atomic(&self.out.join(&buildinfo_name), &buildinfo.to_json(), 0o644)?;

        let outputs: BTreeMap<Name, StoreId> = output
            .packages
            .iter()
            .map(|p| (p.meta.name.clone(), p.manifest.store_id()))
            .collect();
        let stored = Stored {
            record: BuildRecord {
                item: Item::of(recipe),
                key,
                inputs: inputs.clone(),
                outputs: outputs.clone(),
                imports: output.imports.clone(),
            },
            buildinfo: buildinfo_name,
            files: buildinfo.outputs.iter().map(|o| o.file.clone()).collect(),
        };
        Ok(Outcome {
            stored,
            buildinfo,
            built: Built {
                outputs,
                exports: output.exports,
            },
            packages,
            warnings: output.warnings,
        })
    }

    /// Runs the smoke test of the package `main` in a view of it and
    /// its runtime closure.
    fn smoke(&self, session: &Session, main: &Package, script: &str) -> Result<(), BuildError> {
        let identity = main.manifest.package();
        let id = main.manifest.store_id();
        let entry = StoreEntry {
            dir: id.dir_name(&identity.name, &identity.version, identity.variant),
            id,
            meta: main.meta.clone(),
        };
        let tree = session.dir.join("smoke-root");
        let usr = view::assemble_smoke(&self.store, &self.seeds()?, &entry, &tree)?;
        let result = post::smoke(
            &usr,
            script,
            &session.dir.join("smoke"),
            session.exec.logs(),
            self.timeout,
        );
        fsx::remove_tree(&tree)?;
        result
    }
}

/// Where [`Builder::install`] records what it installed, relative to the
/// destination: one record per item, named `<name>+<variant>`, as
/// [`write_key`] writes it.
const INSTALLED: &str = "var/lib/laft-build/installed";

/// The format of the records that [`write_key`] writes. Format 0 is a
/// bare build key line, which [`read_key`] still reads.
const KEY_RECORD_FORMAT: u32 = 1;

/// A record of a build key, written as one line of JSON.
#[derive(serde::Serialize, serde::Deserialize)]
struct KeyRecord {
    format: u32,
    key: String,
}

/// Records `key` at `path`: `{"format":1,"key":"b3:..."}` and a newline,
/// replaced atomically.
pub(crate) fn write_key(path: &Path, key: &Blake3Hash) -> Result<(), BuildError> {
    let record = KeyRecord {
        format: KEY_RECORD_FORMAT,
        key: key.to_string(),
    };
    let mut bytes = laft_core::to_canonical_json(&record)
        .map_err(|e| BuildError::problem("B023", format!("{}: {e}", path.display())))?;
    bytes.push(b'\n');
    fsx::write_atomic(path, &bytes, 0o644)
}

/// Removes the record at `path`, so that work which may leave a partial
/// result is never taken for done. A missing record is fine.
pub(crate) fn forget_key(path: &Path) -> Result<(), BuildError> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io(path)(e)),
    }
    let dir = fsx::dir_of(path);
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(io(dir))
}

/// The build key that [`write_key`] recorded at `path`, or `None` when
/// there is no record. A record of format 0, a bare build key line, is
/// read too.
///
/// # Errors
///
/// Fails with `B023` when the record cannot be read, is neither a build
/// key line nor a build key record, has a format other than the ones this
/// version reads or holds no build key.
pub(crate) fn read_key(path: &Path) -> Result<Option<Blake3Hash>, BuildError> {
    let bad = |why: String| BuildError::problem("B023", format!("{}: {why}", path.display()));
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(bad(e.to_string())),
    };
    if let Ok(key) = text.trim_end_matches('\n').parse() {
        return Ok(Some(key));
    }
    let record: KeyRecord =
        serde_json::from_str(&text).map_err(|e| bad(format!("is not a build key record: {e}")))?;
    if record.format != KEY_RECORD_FORMAT {
        return Err(bad(format!(
            "has record format {}, which this laft-build does not read",
            record.format
        )));
    }
    record
        .key
        .parse()
        .map(Some)
        .map_err(|e| bad(format!("{e}")))
}

/// The build key of the latest [`Builder::install`] of `item` into `dest`,
/// or `None` when nothing was installed for it there.
///
/// # Errors
///
/// Fails with `B023` when the record exists but cannot be read, has a
/// format this version does not read or holds no build key.
pub fn installed(dest: &Path, item: &Item) -> Result<Option<Blake3Hash>, BuildError> {
    read_key(&installed_record(dest, item))
}

/// The record of the latest [`Builder::install`] of `item` into `dest`.
pub(crate) fn installed_record(dest: &Path, item: &Item) -> PathBuf {
    dest.join(INSTALLED).join(item_dir(item))
}

/// `<name>+<variant>`, the directory of an item's logs.
fn item_dir(item: &Item) -> String {
    format!("{}+{}", item.name, item.variant)
}

fn problems(problems: Vec<Problem>) -> BuildError {
    let mut problems = problems.into_iter();
    let first = problems
        .next()
        .unwrap_or_else(|| Problem::new("R021", "the recipes cannot be planned"));
    let rest: Vec<String> = problems.map(|p| p.to_string()).collect();
    if rest.is_empty() {
        BuildError::Problem(first)
    } else {
        BuildError::problem(
            first.code,
            format!("{}\n{}", first.message, rest.join("\n")),
        )
    }
}

/// Makes every file below `dir` read-only and every directory `0o555`.
fn read_only(dir: &Path) -> Result<(), BuildError> {
    let mut entries = fsx::walk(dir)?;
    entries.reverse();
    for (rel, kind) in entries {
        match kind {
            Kind::Dir => fsx::set_mode(&dir.join(&rel), 0o555)?,
            Kind::File => {
                let path = dir.join(&rel);
                let mode = fs::metadata(&path).map_err(io(&path))?.permissions();
                let mode = std::os::unix::fs::PermissionsExt::mode(&mode) & 0o7777 & !0o222;
                fsx::set_mode(&path, mode)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// The release of the running kernel.
fn kernel_release() -> String {
    rustix::system::uname()
        .release()
        .to_string_lossy()
        .into_owned()
}

/// Installs the license files of `recipe` from the source tree `src` into
/// `out/usr/share/licenses/<name>/`: the files `license_files` lists, or
/// every top-level file whose name starts with `COPYING`, `LICENSE`,
/// `LICENCE`, `NOTICE` or `COPYRIGHT`.
fn install_licenses(recipe: &Recipe, src: &Path, out: &Path) -> Result<(), BuildError> {
    let files: Vec<String> = match &recipe.meta.license_files {
        Some(files) => files.clone(),
        None => {
            let mut found = Vec::new();
            for item in fs::read_dir(src).map_err(io(src))? {
                let item = item.map_err(io(src))?;
                let name = item.file_name().to_string_lossy().into_owned();
                let known = ["COPYING", "LICENSE", "LICENCE", "NOTICE", "COPYRIGHT"]
                    .iter()
                    .any(|p| name.starts_with(p));
                if known && item.file_type().map_err(io(src))?.is_file() {
                    found.push(name);
                }
            }
            found.sort();
            found
        }
    };
    if files.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(out).map_err(io(out))?;
    let mut dir = out.to_path_buf();
    for part in ["usr", "share", "licenses", recipe.name.as_str()] {
        dir.push(part);
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(BuildError::problem(
                    "B023",
                    format!("{} is not a directory", dir.display()),
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&dir).map_err(io(&dir))?;
            }
            Err(e) => return Err(io(&dir)(e)),
        }
    }
    for file in files {
        let mut from = fsx::open_beneath(src, &file).map_err(|_| {
            BuildError::problem(
                "B023",
                format!("license file {file} is not a file of the source tree"),
            )
        })?;
        let to = dir.join(fsx::file_name(&file));
        // The install phase may have left a symbolic link here, which a
        // plain create would follow out of the tree.
        match fs::remove_file(&to) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(&to)(e)),
            _ => {}
        }
        let mut dest = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&to)
            .map_err(io(&to))?;
        std::io::copy(&mut from, &mut dest).map_err(io(&to))?;
        fsx::set_mode(&to, 0o644)?;
    }
    Ok(())
}

/// Writes the cross files of a `static` build into `dir`: `meson.ini` and
/// `toolchain.cmake`, which point the build systems at the musl cross
/// toolchain and its sysroot.
fn write_cross_files(dir: &Path) -> Result<(), BuildError> {
    let musl = crate::MUSL_TRIPLE;
    let sysroot = crate::SYSROOT;
    let meson = format!(
        "[binaries]\nc = '{musl}-gcc'\ncpp = '{musl}-g++'\nar = '{musl}-ar'\n\
         strip = '{musl}-strip'\npkg-config = 'pkgconf'\n\n[properties]\n\
         sys_root = '{sysroot}'\npkg_config_libdir = '{sysroot}/usr/lib/pkgconfig:{sysroot}/usr/share/pkgconfig'\n\n\
         [built-in options]\ndefault_library = 'static'\nprefer_static = true\n\n\
         [host_machine]\nsystem = 'linux'\ncpu_family = 'x86_64'\ncpu = 'x86_64'\nendian = 'little'\n"
    );
    let cmake = format!(
        "set(CMAKE_SYSTEM_NAME Linux)\nset(CMAKE_SYSTEM_PROCESSOR x86_64)\n\
         set(CMAKE_SYSROOT {sysroot})\nset(CMAKE_C_COMPILER {musl}-gcc)\n\
         set(CMAKE_CXX_COMPILER {musl}-g++)\nset(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)\n\
         set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)\nset(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)\n\
         set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)\nset(BUILD_SHARED_LIBS OFF)\n"
    );
    for (name, text) in [("meson.ini", meson), ("toolchain.cmake", cmake)] {
        let path = dir.join(name);
        fs::write(&path, text).map_err(io(&path))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;
