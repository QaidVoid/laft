//! Bootstrap stages: a list of steps in a Stav file that build packages
//! into a root directory, on the host or inside the root entered as a
//! chroot, or into the build-side store, and run scripts such as
//! checkpoints.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::os::unix::fs::symlink;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use laft_core::{Blake3Hash, PkgRef};
use laft_sandbox::RootSandbox;
use serde::Deserialize;
use stav::{AttrPath, ImportRoot, Limits, RootSpec};

use crate::build::{forget_key, installed_record, read_key, write_key};
use crate::error::{BuildError, io};
use crate::exec::{Executor, tail, wait};
use crate::loader::Scope;
use crate::{
    BuildRoot, Builder, Error, Item, KeyInputs, Mode, Problem, Recipe, Repos, Runner, SourceCache,
    fsx, host, installed, repo, schema,
};

/// Where a step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    /// On the host, with the host's tools and the stage's search path.
    Host,
    /// Inside the root, entered as a chroot through a [`RootSandbox`].
    Root,
    /// For a script of an [`Install::Store`] stage: in a sandbox whose
    /// `/usr` is a view of the build-side store.
    View,
}

/// How the packages of a [`Stage`] reach its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Install {
    /// Every build installs straight into the root with
    /// [`Builder::install`], and no package is made.
    Direct,
    /// Every build makes Laft packages, which stay in the output directory
    /// of the build root and are unpacked into the root with
    /// [`Builder::build_into`].
    Packages,
    /// Every build runs in [`Mode::Store`] on the host, with a build view
    /// of the build-side store, and its packages enter the store, where the
    /// builds after it find them. Such a stage has no root.
    Store,
}

/// A bootstrap stage, read from a stage file with [`Stage::load`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// The stage name, the label of its `stage` block.
    pub name: String,
    /// The recipe repository of its packages, with symbolic links
    /// resolved.
    pub recipes: PathBuf,
    /// How its packages reach the root.
    pub install: Install,
    /// Directories relative to the root that steps on the host search for
    /// programs before the host's own.
    pub path: Vec<PathBuf>,
    /// The steps, in order.
    pub steps: Vec<Step>,
}

/// One step of a [`Stage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The name, unique in the stage.
    pub name: String,
    /// Where it runs.
    pub place: Place,
    /// What it does.
    pub work: Work,
}

/// What a [`Step`] does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    /// Builds these packages in order and installs them into the root.
    Packages(Vec<PkgRef>),
    /// Runs a POSIX shell script with extra environment variables.
    Script {
        /// The script.
        script: String,
        /// The extra environment variables.
        env: BTreeMap<String, String>,
        /// For [`Place::View`], the packages of the view it runs in.
        view: Vec<PkgRef>,
    },
}

impl Stage {
    /// Reads the stage file `file`, which holds exactly one `stage` block
    /// and imports nothing.
    ///
    /// # Errors
    ///
    /// Returns the [`crate::Report`] of a file that does not evaluate, and
    /// `R060` for a file without exactly one stage, a step that sets both or
    /// neither of `packages` and `script`, a stage name that does not start
    /// with a lowercase letter or digit followed by lowercase letters,
    /// digits and dashes, a step name used twice, a search
    /// directory that is not a plain relative path, a recipe repository
    /// that does not exist, a step of an [`Install::Store`] stage inside
    /// the root, a search directory of such a stage, a [`Place::View`] step
    /// that is not a script of such a stage or has an empty view, or a view
    /// on a step that does not run in one.
    pub fn load(file: &Path) -> Result<Self, Error> {
        let problem = |message: String| Error::Problems(vec![Problem::new("R060", message)]);
        let file = fs::canonicalize(file)
            .map_err(|e| problem(format!("{} cannot be opened: {e}", file.display())))?;
        let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let host = host::host(vec![dir.clone()], Scope::Stage(name));
        let spec = RootSpec {
            root_struct: "StageFile".to_owned(),
            entries: vec![file.clone()],
            import_roots: vec![ImportRoot::new(&dir)],
        };
        let checked = stav::check(&host, &spec).map_err(|e| Error::Report(repo::own_codes(&e)))?;
        let evaluated = checked.eval(&Limits::default()).map_err(|e| {
            Error::Report(checked.diagnostics(e.iter().chain(checked.warnings().iter()).cloned()))
        })?;
        let stages: BTreeMap<String, schema::Stage> = evaluated
            .get(&AttrPath::new(["stages"]))
            .map_err(Error::Report)?;
        let mut stages = stages.into_iter();
        let (Some((name, stage)), None) = (stages.next(), stages.next()) else {
            return Err(problem(format!(
                "{} does not declare exactly one stage block",
                file.display()
            )));
        };
        let plain = |text: &str| {
            text.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                && text
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        if !plain(&name) {
            return Err(problem(format!(
                "stage {name:?} does not start with a lowercase letter or digit followed by lowercase letters, digits and dashes"
            )));
        }
        let recipes = fs::canonicalize(dir.join(&stage.recipes)).map_err(|e| {
            problem(format!(
                "stage {name}: recipe repository {} cannot be opened: {e}",
                stage.recipes
            ))
        })?;
        let mut path = Vec::new();
        for dir in &stage.path {
            let relative = !dir.is_empty()
                && Path::new(dir)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)));
            if !relative {
                return Err(problem(format!(
                    "stage {name}: search directory {dir} is not a plain relative path"
                )));
            }
            path.push(PathBuf::from(dir));
        }
        let store = stage.install == Install::Store;
        if store && !path.is_empty() {
            return Err(problem(format!(
                "stage {name}: a stage that installs into the store has no root to search"
            )));
        }
        let mut names = BTreeSet::new();
        let mut steps = Vec::new();
        for step in stage.steps {
            if !names.insert(step.name.clone()) {
                return Err(problem(format!(
                    "stage {name}: step {} is named twice",
                    step.name
                )));
            }
            let work = match (step.packages.is_empty(), step.script) {
                (false, None) => Work::Packages(step.packages),
                (true, Some(script)) => Work::Script {
                    script,
                    env: step.env,
                    view: step.view.clone(),
                },
                _ => {
                    return Err(problem(format!(
                        "stage {name}: step {} sets both or neither of packages and script",
                        step.name
                    )));
                }
            };
            let script = matches!(work, Work::Script { .. });
            let wrong = match step.place {
                Place::Root if store => Some("runs inside the root of a stage that has none"),
                Place::View if !store => {
                    Some("runs in a view, which only a stage that installs into the store has")
                }
                Place::View if !script => Some("builds packages in a view"),
                Place::View if step.view.is_empty() => Some("runs in a view without packages"),
                Place::Host | Place::Root if !step.view.is_empty() => {
                    Some("names a view but does not run in one")
                }
                _ => None,
            };
            if let Some(wrong) = wrong {
                return Err(problem(format!("stage {name}: step {} {wrong}", step.name)));
            }
            steps.push(Step {
                name: step.name,
                place: step.place,
                work,
            });
        }
        Ok(Self {
            name,
            recipes,
            install: stage.install,
            path,
            steps,
        })
    }
}

/// How [`run_stage`] runs a stage.
#[derive(Debug, Clone)]
pub struct StageOptions {
    /// The root the packages are installed into and `root` steps run in,
    /// which every stage needs except one of [`Install::Store`].
    pub root: Option<PathBuf>,
    /// Whether the check phases of [`Install::Store`] builds run.
    pub check: bool,
    /// The number of parallel jobs inside one build.
    pub jobs: u32,
    /// The longest one phase or script may run.
    pub timeout: Duration,
    /// The available memory, in bytes, that a build waits for when its
    /// recipe's CI runner is `big`.
    pub min_available: u64,
    /// Report what would run instead of running it.
    pub dry_run: bool,
    /// Run every step, even one whose record shows it up to date.
    pub rebuild: bool,
    /// A statically linked `laft-build` that runs the builds of `root`
    /// steps inside the root.
    pub laft_build: PathBuf,
    /// Extra source mirrors, tried after the repository's own.
    pub mirrors: Vec<String>,
    /// The token of the lock of the build root that this run holds, which
    /// a build inside the root receives in [`crate::LOCK_ENV`] so that it
    /// shares the lock.
    pub lock: Option<String>,
}

/// What became of one package or script of a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    /// It was installed or run before, unchanged.
    UpToDate,
    /// A dry run would install or run it.
    Pending,
    /// It was installed or run now.
    Done,
}

impl StageState {
    /// The state in words, as progress reports show it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up to date",
            Self::Pending => "pending",
            Self::Done => "done",
        }
    }
}

/// A step of [`run_stage`], for progress reports.
#[derive(Debug)]
pub enum StageEvent<'a> {
    /// A package build or a script starts.
    Start {
        /// The step.
        step: &'a str,
        /// The package or `script`.
        what: &'a str,
    },
    /// A build waits for memory.
    Waiting {
        /// The available memory, in bytes.
        available: u64,
        /// The memory it waits for, in bytes.
        needed: u64,
    },
    /// A package build or a script finished or was skipped.
    Finished {
        /// The step.
        step: &'a str,
        /// The package or `script`.
        what: &'a str,
        /// What became of it.
        state: StageState,
        /// How long it took.
        elapsed: Duration,
    },
}

/// Where the sandbox of a `root` step shows the build root, the recipe
/// repository and, for a build, `laft-build`.
const INSIDE_BUILD_ROOT: &str = "laft-build-root";
const INSIDE_RECIPES: &str = "laft-recipes";
const INSIDE_LAFT_BUILD: &str = "laft-build";

/// Where steps record that they ran, relative to the root: one record per
/// step, `<stage>/<step>`, that holds the key of the step. A stage without
/// a root keeps them in [`STORE_STEPS_RUN`] of the build root instead.
const STEPS_RUN: &str = "var/lib/laft-build/stages";

/// Where the steps of an [`Install::Store`] stage record that they ran,
/// relative to the build root, laid out like [`STEPS_RUN`].
const STORE_STEPS_RUN: &str = "stages";

/// The top-level links of Laft's merged `/usr`.
const ROOT_LINKS: [(&str, &str); 4] = [
    ("bin", "usr/bin"),
    ("sbin", "usr/bin"),
    ("lib", "usr/lib"),
    ("lib64", "usr/lib"),
];

/// Runs `stage` with the packages of `repos`, the sources of `cache` and
/// the build root `build_root`, in order, and stops at the first failure.
///
/// The root gets Laft's top-level layout first: `usr/bin` and `usr/lib`,
/// with `bin` and `sbin` linking to `usr/bin` and `lib` and `lib64` to
/// `usr/lib`. Each package is built in bootstrap mode and reaches the root
/// as [`Stage::install`] says: with [`Builder::install`], or as packages
/// that [`Builder::build_into`] makes and unpacks, which the build
/// database records like any build. On the host, a build uses the
/// host's tools after the stage's search directories. Inside the root,
/// `laft-build` itself runs the build in a [`RootSandbox`], with the build
/// root and the recipe repository bound below `/run`, after the host
/// fetched the sources.
///
/// A stage of [`Install::Store`] has no root. Each of its packages is built
/// with [`Builder::build`] in [`Mode::Store`], with the check phase when
/// [`StageOptions::check`] says so, its packages enter the build-side store
/// and the build database records it, so every later build of the stage
/// sees it in its build view, in place of a seed of the same origin.
///
/// A script runs with POSIX `sh`, which stops at the first command that
/// fails or the first unset variable, and with `HOME`, `TMPDIR`, `LANG`,
/// `LC_ALL` and `TZ` besides its own variables, and with `ROOT` when the stage has
/// a root: on the host in a scratch directory with the stage's search
/// path, inside the root in `/` with `PATH=/usr/bin`, and in a view in a
/// sandbox like a build's, whose `/usr` is a view of the build-side store
/// with the step's packages and their runtime dependencies, with a
/// scratch directory as `/build` and `PATH=/usr/bin`. The log of a script
/// in a view starts with the packages of the view and their store ids.
///
/// Every step has a key that covers where it runs, on the host together
/// with the stage's search directories and in a view with its packages and
/// the store entries they resolve to, its script and environment or the
/// build keys of its packages and whether they are made into packages, and
/// the key of the step before it, so it covers every earlier step too.
/// Once a step succeeded, its key is recorded in the root. A step whose
/// recorded key matches is skipped unless [`StageOptions::rebuild`] is
/// set, so a changed step runs again together with every step after it.
/// Each package of a step is recorded too, with the key the step would
/// have if it ended with that package, so a failed run continues with the
/// package that failed. The build key of a package is taken right before
/// it builds, so that it covers the packages that the step built before
/// it. A record goes before the script or package it stands for runs, so
/// work that fails partway never counts as done, even when a later run
/// brings back the key it had before.
///
/// A build whose recipe has `ci.runner = big` first waits until the host
/// has [`StageOptions::min_available`] of memory available. Every script
/// and every build inside the root logs to
/// `logs/stage/<stage>/<nn>-<step>[-<package>].log` in the build root, and
/// builds on the host log every phase as [`Builder`] does.
///
/// # Errors
///
/// Fails like [`Builder::install`], [`Builder::build_into`] or
/// [`Builder::build`], with `B040` for a script or a build
/// inside the root that fails, with `B030` for a script that runs longer
/// than [`StageOptions::timeout`], with `B042` when the root's top-level
/// entries differ from Laft's layout or when `laft-build` is not static
/// but a step builds inside the root, with `B050` when the store cannot
/// satisfy the view of a script, and with `R060` when an
/// [`Install::Store`] stage is given a root or another stage none.
pub fn run_stage(
    stage: &Stage,
    repos: &Repos,
    cache: &SourceCache,
    build_root: &BuildRoot,
    options: &StageOptions,
    progress: &mut dyn FnMut(StageEvent<'_>),
) -> Result<(), BuildError> {
    let store = stage.install == Install::Store;
    match (&options.root, store) {
        (Some(_), true) => {
            return Err(BuildError::problem(
                "R060",
                format!(
                    "stage {} installs into the store and has no root",
                    stage.name
                ),
            ));
        }
        (None, false) => {
            return Err(BuildError::problem(
                "R060",
                format!("stage {} needs a root", stage.name),
            ));
        }
        _ => {}
    }
    if !options.dry_run {
        if let Some(root) = &options.root {
            layout(root)?;
        }
        if stage
            .steps
            .iter()
            .any(|s| s.place == Place::Root && matches!(s.work, Work::Packages(_)))
        {
            check_static(&options.laft_build)?;
        }
    }
    run_steps(stage, repos, cache, build_root, options, progress)
}

/// Runs the steps of `stage` like [`run_stage`], after its checks.
fn run_steps(
    stage: &Stage,
    repos: &Repos,
    cache: &SourceCache,
    build_root: &BuildRoot,
    options: &StageOptions,
    progress: &mut dyn FnMut(StageEvent<'_>),
) -> Result<(), BuildError> {
    let mode = match stage.install {
        Install::Store => Mode::Store,
        Install::Direct | Install::Packages => Mode::Bootstrap("/".into()),
    };
    let mut builder = Builder::new(repos, cache, build_root, mode);
    builder.check = options.check && stage.install == Install::Store;
    builder.jobs = options.jobs;
    builder.timeout = options.timeout;
    builder.mirrors = options.mirrors.clone();
    if let Some(root) = &options.root {
        builder.search = stage.path.iter().map(|p| root.join(p)).collect();
    }
    let logs = build_root.logs().join("stage").join(&stage.name);
    let runner = StageRun {
        stage,
        builder: &builder,
        build_root,
        options,
        logs: &logs,
    };
    let mut previous = None;
    for (index, step) in stage.steps.iter().enumerate() {
        let tag = format!("{:02}-{}", index + 1, step.name);
        let key = match &step.work {
            Work::Packages(packages) => {
                let mut found = Vec::new();
                for package in packages {
                    found.extend(recipes(repos, package)?);
                }
                runner.packages(step, &tag, &found, previous, progress)?
            }
            Work::Script { .. } => runner.script(step, &tag, previous, progress)?,
        };
        previous = Some(key);
    }
    Ok(())
}

/// The key of `step`, whose own work is `work`, after the step whose key is
/// `previous`. A step on the host also covers `path`, the stage's search
/// directories, which decide the programs it finds, and a step in a view
/// the packages of its view.
fn step_key(
    step: &Step,
    previous: Option<Blake3Hash>,
    path: &[PathBuf],
    work: serde_json::Value,
) -> Result<Blake3Hash, BuildError> {
    let mut value = serde_json::json!({
        "previous": previous.map(|k| k.to_string()),
        "place": "root",
        "work": work,
    });
    match step.place {
        Place::Host => {
            value["place"] = "host".into();
            value["path"] = path.iter().map(|p| p.to_string_lossy()).collect();
        }
        Place::View => {
            value["place"] = "view".into();
            if let Work::Script { view, .. } = &step.work {
                value["view"] = view.iter().map(ToString::to_string).collect();
            }
        }
        Place::Root => {}
    }
    let bytes = laft_core::to_canonical_json(&value)
        .map_err(|e| BuildError::problem("B023", format!("step {}: {e}", step.name)))?;
    Ok(Blake3Hash::of(&bytes))
}

/// The recipes of `package` in `repos`, in the variant it names.
fn recipes(repos: &Repos, package: &PkgRef) -> Result<Vec<Recipe>, BuildError> {
    let evaluation = repos
        .evaluate(package.name())
        .map_err(|e| BuildError::problem("R060", e.to_string()))?;
    let found: Vec<Recipe> = evaluation
        .recipes
        .into_iter()
        .filter(|r| r.variant == package.variant())
        .collect();
    if found.is_empty() {
        return Err(BuildError::problem(
            "R060",
            format!("{package} builds nothing for this architecture and variant"),
        ));
    }
    Ok(found)
}

/// Gives `root` Laft's top-level layout, refusing one that differs.
fn layout(root: &Path) -> Result<(), BuildError> {
    for dir in ["usr/bin", "usr/lib"] {
        let path = root.join(dir);
        fs::create_dir_all(&path).map_err(io(&path))?;
    }
    for (name, target) in ROOT_LINKS {
        let path = root.join(name);
        match fs::read_link(&path) {
            Ok(found) if found == Path::new(target) => {}
            Ok(_) => return Err(not_laft(&path, target)),
            Err(_) if path.symlink_metadata().is_ok() => return Err(not_laft(&path, target)),
            Err(_) => symlink(target, &path).map_err(io(&path))?,
        }
    }
    Ok(())
}

fn not_laft(path: &Path, target: &str) -> BuildError {
    BuildError::problem(
        "B042",
        format!("{} is not a symbolic link to {target}", path.display()),
    )
}

/// Fails with `B042` unless `program` is a static executable, which is the
/// only kind that runs inside a root without the host's libraries.
fn check_static(program: &Path) -> Result<(), BuildError> {
    let mut bytes = Vec::new();
    fs::File::open(program)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(io(program))?;
    let why = match laft_elf::inspect(&bytes) {
        Ok(Some(elf)) if elf.is_static() => return Ok(()),
        Ok(Some(_)) => "is not statically linked".to_owned(),
        Ok(None) => "is not an ELF file".to_owned(),
        Err(e) => format!("is not an ELF file that can be read: {e}"),
    };
    Err(BuildError::problem(
        "B042",
        format!(
            "{} {why}, so it cannot build inside the root",
            program.display()
        ),
    ))
}

/// The available memory of the host, from `MemAvailable` in
/// `/proc/meminfo`.
fn available_memory() -> Result<u64, BuildError> {
    let path = Path::new("/proc/meminfo");
    let text = fs::read_to_string(path).map_err(io(path))?;
    text.lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))
        .and_then(|rest| {
            rest.trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|kib| kib * 1024)
        .ok_or_else(|| BuildError::problem("B023", "/proc/meminfo has no MemAvailable line"))
}

/// What the steps of one run share.
struct StageRun<'a> {
    stage: &'a Stage,
    builder: &'a Builder<'a>,
    build_root: &'a BuildRoot,
    options: &'a StageOptions,
    logs: &'a Path,
}

impl StageRun<'_> {
    /// The record of `step`, in the root or, for a stage without one, in
    /// the build root.
    fn record(&self, step: &Step) -> PathBuf {
        let dir = match &self.options.root {
            Some(root) => root.join(STEPS_RUN),
            None => self.build_root.path().join(STORE_STEPS_RUN),
        };
        dir.join(&self.stage.name).join(&step.name)
    }

    /// The root, which [`run_stage`] checked every stage but one of
    /// [`Install::Store`] has.
    fn root(&self) -> Result<&Path, BuildError> {
        self.options.root.as_deref().ok_or_else(|| {
            BuildError::problem("R060", format!("stage {} needs a root", self.stage.name))
        })
    }

    /// What becomes of `step` with the key `key` instead of running: up to
    /// date when its record shows `key` and records count, pending in a dry
    /// run, or `None` when it runs.
    fn skipped(&self, step: &Step, key: Blake3Hash) -> Result<Option<StageState>, BuildError> {
        if !self.options.rebuild && read_key(&self.record(step))? == Some(key) {
            Ok(Some(StageState::UpToDate))
        } else if self.options.dry_run {
            Ok(Some(StageState::Pending))
        } else {
            Ok(None)
        }
    }

    /// Builds and installs `recipes` for `step` unless it is up to date, and
    /// returns its key.
    fn packages(
        &self,
        step: &Step,
        tag: &str,
        recipes: &[Recipe],
        previous: Option<Blake3Hash>,
        progress: &mut dyn FnMut(StageEvent<'_>),
    ) -> Result<Blake3Hash, BuildError> {
        let mut keys = Vec::new();
        for recipe in recipes {
            keys.push(self.builder.key_inputs(recipe)?.build_key().to_string());
        }
        let work = |keys: &[String]| match self.stage.install {
            Install::Direct => serde_json::json!({ "packages": keys }),
            Install::Packages => serde_json::json!({ "packages": keys, "install": "packages" }),
            Install::Store => serde_json::json!({ "packages": keys, "install": "store" }),
        };
        let key = step_key(step, previous, &self.stage.path, work(&keys))?;
        if !self.options.rebuild && read_key(&self.record(step))? == Some(key) {
            for recipe in recipes {
                let what = Item::of(recipe).to_string();
                progress(finished(
                    &step.name,
                    &what,
                    StageState::UpToDate,
                    Duration::ZERO,
                ));
            }
            return Ok(key);
        }
        if !self.options.dry_run {
            forget_key(&self.record(step))?;
        }
        for (index, recipe) in recipes.iter().enumerate() {
            // A package that this step built before may have changed what
            // the store holds for this one.
            let inputs = self.builder.key_inputs(recipe)?;
            keys[index] = inputs.build_key().to_string();
            let done = step_key(step, previous, &self.stage.path, work(&keys[..=index]))?;
            let record = self.package_record(step, recipe);
            let state = if !self.options.rebuild && read_key(&record)? == Some(done) {
                Some(StageState::UpToDate)
            } else if self.options.dry_run {
                Some(StageState::Pending)
            } else {
                None
            };
            if let Some(state) = state {
                let what = Item::of(recipe).to_string();
                progress(finished(&step.name, &what, state, Duration::ZERO));
                continue;
            }
            forget_key(&record)?;
            self.package(step, tag, recipe, &inputs, progress)?;
            write_key(&record, &done)?;
        }
        let key = step_key(step, previous, &self.stage.path, work(&keys))?;
        if !self.options.dry_run {
            write_key(&self.record(step), &key)?;
        }
        Ok(key)
    }

    /// The record of `recipe` in the packages step `step`, which holds the
    /// key of the step as if it ended with `recipe`, so that a failed step
    /// resumes with the package that failed.
    fn package_record(&self, step: &Step, recipe: &Recipe) -> PathBuf {
        let mut record = self.record(step).into_os_string();
        record.push(".packages");
        PathBuf::from(record).join(format!("{}+{}", recipe.name, recipe.variant))
    }

    fn package(
        &self,
        step: &Step,
        tag: &str,
        recipe: &Recipe,
        inputs: &KeyInputs,
        progress: &mut dyn FnMut(StageEvent<'_>),
    ) -> Result<(), BuildError> {
        let item = Item::of(recipe);
        let what = item.to_string();
        if recipe.operations.ci.runner == Runner::Big {
            self.wait_for_memory(progress)?;
        }
        progress(StageEvent::Start {
            step: &step.name,
            what: &what,
        });
        let started = Instant::now();
        match step.place {
            Place::Host => match self.stage.install {
                Install::Direct => self.builder.install(recipe, inputs, self.root()?)?,
                Install::Packages => {
                    let outcome = self.builder.build_into(recipe, inputs, self.root()?)?;
                    self.build_root.db().record(&outcome.stored)?;
                }
                Install::Store => {
                    let outcome = self.builder.build(recipe, inputs)?;
                    self.build_root.db().record(&outcome.stored)?;
                }
            },
            Place::View => {
                return Err(BuildError::problem(
                    "R060",
                    format!("step {} builds packages in a view", step.name),
                ));
            }
            Place::Root => {
                let flag = match self.stage.install {
                    Install::Direct => "--install",
                    Install::Packages => "--unpack",
                    Install::Store => {
                        return Err(BuildError::problem(
                            "R060",
                            format!(
                                "step {} builds inside the root of a stage that has none",
                                step.name
                            ),
                        ));
                    }
                };
                let root = self.root()?;
                // The build inside skips a package whose record shows its
                // key, so the record goes first to make it build again.
                forget_key(&installed_record(root, &item))?;
                self.builder.fetch(recipe)?;
                let log = self.logs.join(format!("{tag}-{}.log", recipe.name));
                self.build_inside(&what, flag, &log)?;
                if installed(root, &item)? != Some(inputs.build_key()) {
                    return Err(BuildError::problem(
                        "B040",
                        format!(
                            "{what} built inside the root without recording its build key, see {}",
                            log.display()
                        ),
                    ));
                }
            }
        }
        progress(finished(
            &step.name,
            &what,
            StageState::Done,
            started.elapsed(),
        ));
        Ok(())
    }

    /// Runs `laft-build bootstrap` for `package` inside the root, with
    /// `flag`, `--install` or `--unpack`, for how it reaches the root.
    fn build_inside(&self, package: &str, flag: &str, log: &Path) -> Result<(), BuildError> {
        let hours = self.options.timeout.as_secs().div_ceil(3600).max(1);
        let jobs = self.options.jobs.to_string();
        let hours = hours.to_string();
        let run = |name: &str| format!("/run/{name}");
        let args = [
            "--root",
            &run(INSIDE_BUILD_ROOT),
            "--recipes",
            &run(INSIDE_RECIPES),
            "bootstrap",
            "--root",
            "/",
            flag,
            "/",
            "--jobs",
            &jobs,
            "--timeout-hours",
            &hours,
            package,
        ]
        .map(String::from);
        let command = self
            .sandbox()?
            .bind(&self.options.laft_build, INSIDE_LAFT_BUILD, false)
            .command(run(INSIDE_LAFT_BUILD))
            .args(args)
            .env("HOME", "/root");
        let command = match &self.options.lock {
            Some(token) => command.env(crate::LOCK_ENV, token),
            None => command,
        };
        self.run_logged(command, log, package, None)
    }

    fn sandbox(&self) -> Result<RootSandbox, BuildError> {
        Ok(RootSandbox::new(self.root()?)
            .bind(self.build_root.path(), INSIDE_BUILD_ROOT, true)
            .bind(&self.stage.recipes, INSIDE_RECIPES, false))
    }

    /// Runs `command` with its output in `log`, killing it after `timeout`
    /// if one is given.
    ///
    /// # Errors
    ///
    /// Fails with `B040` when it fails and with `B030` when it runs longer
    /// than `timeout`.
    fn run_logged(
        &self,
        command: laft_sandbox::Command,
        log: &Path,
        what: &str,
        timeout: Option<Duration>,
    ) -> Result<(), BuildError> {
        fs::create_dir_all(self.logs).map_err(io(self.logs))?;
        let file = fs::File::create(log).map_err(io(log))?;
        let err = file.try_clone().map_err(io(log))?;
        let mut child = command
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(err)
            .spawn()?;
        let status = match timeout {
            Some(timeout) => wait(child, timeout, log, what)?,
            None => child.wait().map_err(io(log))?,
        };
        if status.success() {
            return Ok(());
        }
        Err(BuildError::problem(
            "B040",
            format!(
                "{what} failed with {status} inside the root, see {}{}",
                log.display(),
                tail(log)
            ),
        ))
    }

    /// Runs the script of `step` unless it is up to date, and returns its
    /// key.
    fn script(
        &self,
        step: &Step,
        tag: &str,
        previous: Option<Blake3Hash>,
        progress: &mut dyn FnMut(StageEvent<'_>),
    ) -> Result<Blake3Hash, BuildError> {
        let Work::Script { script, env, view } = &step.work else {
            return Err(BuildError::problem(
                "R060",
                format!("step {} runs no script", step.name),
            ));
        };
        let root = &self.options.root;
        let mut work = serde_json::json!({ "script": script, "env": env });
        if step.place == Place::View {
            work["resolved"] = self.resolved(step, view)?;
        }
        let key = step_key(step, previous, &self.stage.path, work)?;
        let script = &format!("{}{script}", crate::Shell::Sh.header());
        if let Some(state) = self.skipped(step, key)? {
            progress(finished(&step.name, "script", state, Duration::ZERO));
            return Ok(key);
        }
        forget_key(&self.record(step))?;
        progress(StageEvent::Start {
            step: &step.name,
            what: "script",
        });
        let started = Instant::now();
        let mut vars = BTreeMap::from([
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("LC_ALL".to_owned(), "C.UTF-8".to_owned()),
            ("TZ".to_owned(), "UTC".to_owned()),
        ]);
        match step.place {
            Place::Host => {
                let scratch = self
                    .build_root
                    .work()
                    .join(format!("stage-{}-{}", self.stage.name, step.name));
                fsx::remove_tree(&scratch)?;
                for dir in ["home", "tmp"] {
                    let path = scratch.join(dir);
                    fs::create_dir_all(&path).map_err(io(&path))?;
                }
                if let Some(root) = root {
                    vars.insert("ROOT".to_owned(), root.display().to_string());
                }
                vars.insert(
                    "HOME".to_owned(),
                    scratch.join("home").display().to_string(),
                );
                vars.insert(
                    "TMPDIR".to_owned(),
                    scratch.join("tmp").display().to_string(),
                );
                vars.extend(env.clone());
                let out = root.clone().unwrap_or_else(|| scratch.join("out"));
                Executor::direct("/", &scratch, &out, self.logs)
                    .search_first(self.builder.search.clone())
                    .timeout(self.options.timeout)
                    .run(tag, script, &vars)?;
                fsx::remove_tree(&scratch)?;
            }
            Place::Root => {
                vars.insert("ROOT".to_owned(), "/".to_owned());
                vars.insert("HOME".to_owned(), "/root".to_owned());
                vars.insert("TMPDIR".to_owned(), "/tmp".to_owned());
                vars.extend(env.clone());
                let command = vars.iter().fold(
                    self.sandbox()?
                        .command("/bin/sh")
                        .args(["-c", script, &step.name]),
                    |c, (k, v)| c.env(k, v),
                );
                let log = self.logs.join(format!("{tag}.log"));
                self.run_logged(command, &log, &step.name, Some(self.options.timeout))?;
            }
            Place::View => {
                vars.insert("HOME".to_owned(), "/build/home".to_owned());
                vars.insert("TMPDIR".to_owned(), "/build/tmp".to_owned());
                vars.extend(env.clone());
                self.in_view(step, tag, script, view, &vars)?;
            }
        }
        write_key(&self.record(step), &key)?;
        progress(finished(
            &step.name,
            "script",
            StageState::Done,
            started.elapsed(),
        ));
        Ok(key)
    }

    /// The store ids that the packages `view` of `step` resolve to, or null
    /// when the store cannot satisfy them, which running the step reports.
    fn resolved(&self, step: &Step, view: &[PkgRef]) -> Result<serde_json::Value, BuildError> {
        let origin = format!("view of step {}", step.name);
        match crate::view::view_packages(&self.builder.store, &self.builder.seeds()?, view, &origin)
        {
            Ok(packages) => Ok(packages.iter().map(|p| p.id.to_string()).collect()),
            Err(e) if e.code() == Some("B050") => Ok(serde_json::Value::Null),
            Err(e) => Err(e),
        }
    }

    /// Runs `script` for `step` in a sandbox whose `/usr` is a view of the
    /// build-side store with the packages `view` and their runtime
    /// dependencies, with the environment `vars`. Its scratch directory,
    /// which holds the view and the sandbox's `/build` and `/out`, is
    /// removed after a success and kept after a failure.
    fn in_view(
        &self,
        step: &Step,
        tag: &str,
        script: &str,
        view: &[PkgRef],
        vars: &BTreeMap<String, String>,
    ) -> Result<(), BuildError> {
        let scratch = self
            .build_root
            .work()
            .join(format!("stage-{}-{}", self.stage.name, step.name));
        fsx::remove_tree(&scratch)?;
        let (build, out, tree) = (
            scratch.join("build"),
            scratch.join("out"),
            scratch.join("tree"),
        );
        for dir in [build.join("home"), build.join("tmp"), out.clone()] {
            fs::create_dir_all(&dir).map_err(io(&dir))?;
        }
        let packages = crate::view::assemble_view(
            &self.builder.store,
            &self.builder.seeds()?,
            view,
            &format!("view of step {}", step.name),
            &tree,
        )?;
        fs::create_dir_all(self.logs).map_err(io(self.logs))?;
        let log = self.logs.join(format!("{tag}.log"));
        let mut header = String::from("the view holds:\n");
        for p in &packages {
            header.push_str(&format!("  {} {}-r{} {}\n", p.name, p.version, p.rev, p.id));
        }
        fs::write(&log, header).map_err(io(&log))?;
        Executor::sandboxed(tree.join("usr"), &build, &out, self.logs)
            .timeout(self.options.timeout)
            .run(tag, script, vars)?;
        fsx::remove_tree(&scratch)
    }

    fn wait_for_memory(&self, progress: &mut dyn FnMut(StageEvent<'_>)) -> Result<(), BuildError> {
        let needed = self.options.min_available;
        loop {
            let available = available_memory()?;
            if available >= needed {
                return Ok(());
            }
            progress(StageEvent::Waiting { available, needed });
            std::thread::sleep(Duration::from_secs(60));
        }
    }
}

fn finished<'a>(
    step: &'a str,
    what: &'a str,
    state: StageState,
    elapsed: Duration,
) -> StageEvent<'a> {
    StageEvent::Finished {
        step,
        what,
        state,
        elapsed,
    }
}

#[cfg(test)]
#[path = "stage_tests.rs"]
mod tests;
