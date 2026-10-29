//! Bootstrap stages: a list of steps in a Stav file that build packages
//! into a root directory, on the host or inside the root entered as a
//! chroot, and run scripts such as checkpoints.

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

use crate::build::{installed_record, read_key, write_key};
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
    /// neither of `packages` and `script`, a stage name that is not made of
    /// lowercase letters, digits and dashes, a step name used twice, a search
    /// directory that is not a plain relative path, or a recipe repository
    /// that does not exist.
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
                "stage {name:?} is not named with lowercase letters, digits and dashes"
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
            let plain = !dir.is_empty()
                && Path::new(dir)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)));
            if !plain {
                return Err(problem(format!(
                    "stage {name}: search directory {dir} is not a plain relative path"
                )));
            }
            path.push(PathBuf::from(dir));
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
                },
                _ => {
                    return Err(problem(format!(
                        "stage {name}: step {} sets both or neither of packages and script",
                        step.name
                    )));
                }
            };
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
    /// The root the packages are installed into and `root` steps run in.
    pub root: PathBuf,
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
/// step, `<stage>/<step>`, that holds the key of the step.
const STEPS_RUN: &str = "var/lib/laft-build/stages";

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
/// A script runs with POSIX `sh` and `ROOT`, `HOME`, `TMPDIR`, `LANG`,
/// `LC_ALL` and `TZ` besides its own variables: on the host in a scratch
/// directory with the stage's search path, inside the root in `/` with
/// `PATH=/usr/bin`.
///
/// Every step has a key that covers where it runs, its script and
/// environment or the build keys of its packages and whether they are
/// made into packages, and the key of the step before it, so it covers
/// every earlier step too. Once a step succeeded,
/// its key is recorded in the root. A step whose recorded key matches is
/// skipped unless [`StageOptions::rebuild`] is set, so a changed step runs
/// again together with every step after it, and a failed run continues
/// with the step that failed, from its first package.
///
/// A build whose recipe has `ci.runner = big` first waits until the host
/// has [`StageOptions::min_available`] of memory available. Every script
/// and every build inside the root logs to
/// `logs/stage/<stage>/<nn>-<step>[-<package>].log` in the build root, and
/// builds on the host log every phase as [`Builder`] does.
///
/// # Errors
///
/// Fails like [`Builder::install`] or [`Builder::build_into`], with
/// `B040` for a script or a build
/// inside the root that fails, with `B030` for a script that runs longer
/// than [`StageOptions::timeout`], and with `B042` when the root's top-level
/// entries differ from Laft's layout or when `laft-build` is not static
/// but a step builds inside the root.
pub fn run_stage(
    stage: &Stage,
    repos: &Repos,
    cache: &SourceCache,
    build_root: &BuildRoot,
    options: &StageOptions,
    progress: &mut dyn FnMut(StageEvent<'_>),
) -> Result<(), BuildError> {
    let root = &options.root;
    if !options.dry_run {
        layout(root)?;
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
    let root = &options.root;
    let mut builder = Builder::new(repos, cache, build_root, Mode::Bootstrap("/".into()));
    builder.check = false;
    builder.jobs = options.jobs;
    builder.timeout = options.timeout;
    builder.mirrors = options.mirrors.clone();
    builder.search = stage.path.iter().map(|p| root.join(p)).collect();
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
            Work::Script { script, env } => {
                runner.script(step, &tag, script, env, previous, progress)?
            }
        };
        previous = Some(key);
    }
    Ok(())
}

/// The key of `step`, whose own work is `work`, after the step whose key is
/// `previous`.
fn step_key(
    step: &Step,
    previous: Option<Blake3Hash>,
    work: serde_json::Value,
) -> Result<Blake3Hash, BuildError> {
    let place = match step.place {
        Place::Host => "host",
        Place::Root => "root",
    };
    let value = serde_json::json!({
        "previous": previous.map(|k| k.to_string()),
        "place": place,
        "work": work,
    });
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
    /// The record of `step` in the root.
    fn record(&self, step: &Step) -> PathBuf {
        self.options
            .root
            .join(STEPS_RUN)
            .join(&self.stage.name)
            .join(&step.name)
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
        let mut inputs = Vec::new();
        for recipe in recipes {
            inputs.push(self.builder.key_inputs(recipe)?);
        }
        let keys: Vec<String> = inputs.iter().map(|i| i.build_key().to_string()).collect();
        let work = match self.stage.install {
            Install::Direct => serde_json::json!({ "packages": keys }),
            Install::Packages => serde_json::json!({ "packages": keys, "install": "packages" }),
        };
        let key = step_key(step, previous, work)?;
        if let Some(state) = self.skipped(step, key)? {
            for recipe in recipes {
                let what = Item::of(recipe).to_string();
                progress(finished(&step.name, &what, state, Duration::ZERO));
            }
            return Ok(key);
        }
        for (recipe, inputs) in recipes.iter().zip(&inputs) {
            self.package(step, tag, recipe, inputs, progress)?;
        }
        write_key(&self.record(step), &key)?;
        Ok(key)
    }

    fn package(
        &self,
        step: &Step,
        tag: &str,
        recipe: &Recipe,
        inputs: &KeyInputs,
        progress: &mut dyn FnMut(StageEvent<'_>),
    ) -> Result<(), BuildError> {
        let root = &self.options.root;
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
                Install::Direct => self.builder.install(recipe, inputs, root)?,
                Install::Packages => {
                    let outcome = self.builder.build_into(recipe, inputs, root)?;
                    self.build_root.db().record(&outcome.stored)?;
                }
            },
            Place::Root => {
                // The build inside skips a package whose record shows its
                // key, so the record goes first to make it build again.
                let record = installed_record(root, &item);
                match fs::remove_file(&record) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(io(&record)(e));
                    }
                    _ => {}
                }
                self.builder.fetch(recipe)?;
                let log = self.logs.join(format!("{tag}-{}.log", recipe.name));
                self.build_inside(&what, &log)?;
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

    /// Runs `laft-build bootstrap` for `package` inside the root.
    fn build_inside(&self, package: &str, log: &Path) -> Result<(), BuildError> {
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
            match self.stage.install {
                Install::Direct => "--install",
                Install::Packages => "--unpack",
            },
            "/",
            "--jobs",
            &jobs,
            "--timeout-hours",
            &hours,
            package,
        ]
        .map(String::from);
        let command = self
            .sandbox()
            .bind(&self.options.laft_build, INSIDE_LAFT_BUILD, false)
            .command(run(INSIDE_LAFT_BUILD))
            .args(args)
            .env("HOME", "/root");
        self.run_logged(command, log, package, None)
    }

    fn sandbox(&self) -> RootSandbox {
        RootSandbox::new(&self.options.root)
            .bind(self.build_root.path(), INSIDE_BUILD_ROOT, true)
            .bind(&self.stage.recipes, INSIDE_RECIPES, false)
    }

    /// Runs `command` with its output in `log`, killing it after `timeout`
    /// if one is given, and fails with `B040` when it fails.
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
            Some(timeout) => wait(child, timeout, log)?,
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
        script: &str,
        env: &BTreeMap<String, String>,
        previous: Option<Blake3Hash>,
        progress: &mut dyn FnMut(StageEvent<'_>),
    ) -> Result<Blake3Hash, BuildError> {
        let root = &self.options.root;
        let key = step_key(
            step,
            previous,
            serde_json::json!({ "script": script, "env": env }),
        )?;
        if let Some(state) = self.skipped(step, key)? {
            progress(finished(&step.name, "script", state, Duration::ZERO));
            return Ok(key);
        }
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
                vars.insert("ROOT".to_owned(), root.display().to_string());
                vars.insert(
                    "HOME".to_owned(),
                    scratch.join("home").display().to_string(),
                );
                vars.insert(
                    "TMPDIR".to_owned(),
                    scratch.join("tmp").display().to_string(),
                );
                vars.extend(env.clone());
                Executor::direct("/", &scratch, root, self.logs)
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
                    self.sandbox()
                        .command("/bin/sh")
                        .args(["-c", script, &step.name]),
                    |c, (k, v)| c.env(k, v),
                );
                let log = self.logs.join(format!("{tag}.log"));
                self.run_logged(command, &log, &step.name, Some(self.options.timeout))?;
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
