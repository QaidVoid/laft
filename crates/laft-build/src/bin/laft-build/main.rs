//! `laft-build`, which turns recipes into Laft packages.
//!
//! ```text
//! laft-build --recipes ~/src/laft-recipes plan
//! laft-build --recipes ~/src/laft-recipes build zlib --shell-on-failure
//! laft-build --recipes ~/src/laft-recipes check-repro zlib
//! laft-build --recipes ~/src/laft-recipes bootstrap --root / zlib bzip2
//! laft-build --recipes ~/src/stage0 bootstrap --root / --install ~/sys \
//!     --path ~/sys/tools/bin binutils-pass1 gcc-pass1
//! ```
//!
//! Everything lives in the build root: the build-side store, the source
//! cache, the build database, the output directory with package files and
//! buildinfo, working trees and logs. Builds take their dependencies from
//! the build-side store, which every successful build adds its packages
//! to, and run every phase in a sandbox without network.
//!
//! With `--json`, results go to standard output as JSON and progress to
//! standard error.
//!
//! The exit status is 0 on success, 1 when a build fails, 2 for a usage or
//! evaluation error, and 3 when the machine lacks something builds need,
//! such as unprivileged user namespaces or free disk space.

#![forbid(unsafe_code)]

mod cli;

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::Parser;
use laft_build::{
    BuildError, BuildRoot, Builder, Buildinfo, Cutoff, Event, Item, Mode, Recipe, Repos,
    SourceCache, Stage, StageEvent, StageOptions, check_repro, record_repro, run_stage,
};
use laft_core::PkgRef;
use laft_fetch::{Config, Fetcher};
use serde_json::{Value, json};

use crate::cli::{BuildOptions, Cli, Command, Planning};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// The free space a build root needs before a build starts.
const MIN_FREE_BYTES: u64 = 10 << 30;

/// Why a command failed, which decides the exit status.
enum Failure {
    /// A usage or evaluation error.
    Usage(String),
    /// A build failed.
    Build(BuildError),
    /// The machine lacks something builds need.
    Environment(String),
}

impl Failure {
    fn code(&self) -> u8 {
        match self {
            Self::Build(e) if e.is_environment() => 3,
            Self::Build(_) => 1,
            Self::Usage(_) => 2,
            Self::Environment(_) => 3,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) | Self::Environment(m) => f.write_str(m),
            Self::Build(e) if e.refuses_namespaces() => write!(
                f,
                "{e}\nbuilds need unprivileged user namespaces: enable them with \
                 `sysctl kernel.unprivileged_userns_clone=1`, or lift the restriction \
                 that a security module places on them"
            ),
            Self::Build(e) => write!(f, "{e}"),
        }
    }
}

impl From<BuildError> for Failure {
    fn from(error: BuildError) -> Self {
        Self::Build(error)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("laft-build: {failure}");
            ExitCode::from(failure.code())
        }
    }
}

/// Writes a line to standard output, ignoring a closed pipe.
fn say(text: impl fmt::Display) {
    let _ = writeln!(io::stdout().lock(), "{text}");
}

/// Everything the commands share.
struct Context {
    root: BuildRoot,
    repos: Repos,
    cache: SourceCache,
    json: bool,
}

fn run(cli: Cli) -> Result<(), Failure> {
    let root = BuildRoot::new(build_root(cli.root)?);
    let json = cli.json;
    match cli.command {
        Command::Gc { sources_older_than } => return gc(&root, sources_older_than),
        Command::Stage {
            file,
            root: target,
            min_free_gib,
            dry_run,
            rebuild,
            options,
        } => {
            if !cli.recipes.is_empty() {
                return Err(Failure::Usage(
                    "stage takes its recipe repository from the stage file, not from --recipes"
                        .into(),
                ));
            }
            let run = StageRun {
                min_free_gib,
                dry_run,
                rebuild,
            };
            return stage(&root, &file, target, &run, &options, json);
        }
        _ => {}
    }
    let paths = if cli.recipes.is_empty() {
        std::env::var("LAFT_RECIPES")
            .unwrap_or_default()
            .split(':')
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .collect()
    } else {
        cli.recipes
    };
    if paths.is_empty() {
        return Err(Failure::Usage(
            "no recipe repository: pass --recipes DIR or set LAFT_RECIPES".into(),
        ));
    }
    let repos = Repos::open(&paths).map_err(|e| Failure::Usage(e.to_string()))?;
    let cache = SourceCache::new(root.sources(), Fetcher::new(Config::new(root.cache())));
    let cx = Context {
        root,
        repos,
        cache,
        json: cli.json,
    };
    match cli.command {
        Command::Eval { package } => eval(&cx, &package),
        Command::Plan { packages, planning } => plan(&cx, &packages, &planning),
        Command::Build {
            packages,
            planning,
            options,
            keep_work,
            shell_on_failure,
        } => build(
            &cx,
            &packages,
            &planning,
            &options,
            keep_work,
            shell_on_failure,
        ),
        Command::Fetch { packages, mirrors } => fetch(&cx, &packages, mirrors),
        Command::Shell {
            package,
            after,
            options,
        } => shell(&cx, &package, after.as_deref(), &options),
        Command::CheckRepro { package } => repro(&cx, &package),
        Command::Lint { packages } => lint(&cx, &packages),
        Command::Bootstrap {
            root,
            install,
            search,
            dry_run,
            options,
            packages,
        } => bootstrap(
            &cx,
            &root,
            install.as_deref(),
            search,
            dry_run,
            &options,
            &packages,
        ),
        Command::Gc { .. } | Command::Stage { .. } => Ok(()),
    }
}

/// The build root: the flag, `$LAFT_BUILD_ROOT`, or `laft-build` in the
/// user's data directory.
fn build_root(flag: Option<PathBuf>) -> Result<PathBuf, Failure> {
    if let Some(path) = flag {
        return absolute(&path);
    }
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(path) = var("LAFT_BUILD_ROOT") {
        return Ok(path.into());
    }
    if let Some(data) = var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data).join("laft-build"));
    }
    var("HOME")
        .map(|home| PathBuf::from(home).join(".local/share/laft-build"))
        .ok_or_else(|| Failure::Usage("no build root: pass --root DIR".into()))
}

/// The recipes `texts` name, each `<name>` for every variant it builds or
/// `<name>:<variant>` for one. Without any, every package's recipes.
fn recipes(cx: &Context, texts: &[String]) -> Result<Vec<Recipe>, Failure> {
    let wanted: Vec<(String, Option<PkgRef>)> = if texts.is_empty() {
        cx.repos.packages().map(|n| (n.to_string(), None)).collect()
    } else {
        texts
            .iter()
            .map(|t| {
                let r: PkgRef = t
                    .parse()
                    .map_err(|e| Failure::Usage(format!("{t} is not a package: {e}")))?;
                let name = r.name().to_string();
                Ok((name, t.contains(':').then_some(r)))
            })
            .collect::<Result<_, Failure>>()?
    };
    let mut out = Vec::new();
    for (name, variant) in wanted {
        let parsed = name
            .parse()
            .map_err(|e| Failure::Usage(format!("{name}: {e}")))?;
        let evaluation = cx
            .repos
            .evaluate(&parsed)
            .map_err(|e| Failure::Usage(e.to_string()))?;
        let mut found: Vec<Recipe> = evaluation
            .recipes
            .into_iter()
            .filter(|r| variant.as_ref().is_none_or(|v| v.variant() == r.variant))
            .collect();
        if found.is_empty() && variant.is_some() {
            return Err(Failure::Usage(format!(
                "{name} does not build the variant you asked for"
            )));
        }
        out.append(&mut found);
    }
    Ok(out)
}

fn items(cx: &Context, texts: &[String]) -> Result<BTreeSet<Item>, Failure> {
    Ok(recipes(cx, texts)?.iter().map(Item::of).collect())
}

fn builder<'a>(cx: &'a Context, mode: Mode, options: Option<&BuildOptions>) -> Builder<'a> {
    let mut builder = Builder::new(&cx.repos, &cx.cache, &cx.root, mode);
    if let Some(options) = options {
        builder.check = !options.no_check;
        if let Some(jobs) = options.jobs {
            builder.jobs = jobs;
        }
        builder.timeout = Duration::from_secs(options.timeout_hours * 3600);
        builder.mirrors = options.mirrors.clone();
    }
    builder
}

/// Fails with `B031` when the build root has less than 10 GiB free.
fn check_space(root: &Path) -> Result<(), Failure> {
    fs::create_dir_all(root)
        .map_err(|e| Failure::Environment(format!("{}: {e}", root.display())))?;
    let stat = rustix::fs::statvfs(root)
        .map_err(|e| Failure::Environment(format!("{}: {e}", root.display())))?;
    let free = stat.f_bavail.saturating_mul(stat.f_frsize);
    if free < MIN_FREE_BYTES {
        return Err(Failure::Environment(format!(
            "error[B031]: {} has {} MiB free, but builds need 10 GiB",
            root.display(),
            free >> 20
        )));
    }
    Ok(())
}

fn eval(cx: &Context, package: &str) -> Result<(), Failure> {
    let recipes = recipes(cx, &[package.to_owned()])?;
    if cx.json {
        let value = serde_json::to_value(&recipes).map_err(|e| Failure::Usage(e.to_string()))?;
        say(value);
        return Ok(());
    }
    for recipe in recipes {
        say(format_args!(
            "{} {}-r{}\nplan hash: {}",
            Item::of(&recipe),
            recipe.version,
            recipe.rev,
            recipe.plan_hash()
        ));
        say("\n## environment");
        for (key, value) in &recipe.env {
            say(format_args!("{key}={value}"));
        }
        for (name, script) in recipe.phases.iter() {
            if let Some(script) = script {
                say(format_args!("\n## {name}\n{}", script.trim_end()));
            }
        }
        say("");
    }
    Ok(())
}

fn plan(cx: &Context, packages: &[String], planning: &Planning) -> Result<(), Failure> {
    let recipes = recipes(cx, packages)?;
    let builder = builder(cx, Mode::Store, None);
    let forced = if planning.rebuild {
        recipes.iter().map(Item::of).collect()
    } else {
        BTreeSet::new()
    };
    let use_published = items(cx, &planning.use_published)?;
    let (_, schedule) = builder
        .plan(recipes, &cx.root.db(), &forced, &use_published)
        .map_err(plan_failure)?;
    let planned = schedule.planned();
    if cx.json {
        let list: Vec<Value> = planned
            .iter()
            .map(|(item, triggers)| {
                json!({
                    "item": item.to_string(),
                    "triggers": triggers.iter().map(ToString::to_string).collect::<Vec<_>>(),
                })
            })
            .collect();
        say(json!({ "build": list }));
    } else if planned.is_empty() {
        say("up to date");
    } else {
        for (item, triggers) in planned {
            let triggers: Vec<String> = triggers.iter().map(ToString::to_string).collect();
            say(format_args!("{item}  {}", triggers.join(", ")));
        }
    }
    Ok(())
}

/// A problem found while planning is an error in the recipes, not a
/// failed build.
fn plan_failure(error: BuildError) -> Failure {
    match error {
        BuildError::Problem(problem) => Failure::Usage(problem.to_string()),
        other => other.into(),
    }
}

fn build(
    cx: &Context,
    packages: &[String],
    planning: &Planning,
    options: &BuildOptions,
    keep_work: bool,
    shell_on_failure: bool,
) -> Result<(), Failure> {
    check_space(cx.root.path())?;
    let recipes = recipes(cx, packages)?;
    let mut builder = builder(cx, Mode::Store, Some(options));
    builder.keep_work = keep_work;
    let forced = if planning.rebuild {
        recipes.iter().map(Item::of).collect()
    } else {
        BTreeSet::new()
    };
    let use_published = items(cx, &planning.use_published)?;
    let db = cx.root.db();
    let (graph, mut schedule) = builder
        .plan(recipes, &db, &forced, &use_published)
        .map_err(plan_failure)?;
    if schedule.planned().is_empty() {
        if cx.json {
            say(json!({ "built": [] }));
        } else {
            say("up to date");
        }
        return Ok(());
    }
    let mut current: Option<Item> = None;
    let mut report = Vec::new();
    let result = builder.run(&graph, &mut schedule, &db, &mut |event| match event {
        Event::Start { item, triggers } => {
            let triggers: Vec<String> = triggers.iter().map(ToString::to_string).collect();
            eprintln!("building {item} ({})", triggers.join(", "));
            current = Some(item.clone());
        }
        Event::Done {
            item,
            outcome,
            cutoff,
        } => {
            for warning in &outcome.warnings {
                eprintln!("{item}: {warning}");
            }
            let changes = match cutoff {
                Cutoff::Unchanged => "outputs unchanged".to_owned(),
                Cutoff::Changed(dirtied) if dirtied.is_empty() => "outputs changed".to_owned(),
                Cutoff::Changed(dirtied) => {
                    let dirtied: Vec<String> = dirtied
                        .iter()
                        .map(|d| format!("{} ({}: {})", d.item, d.trigger, d.reason))
                        .collect();
                    format!("outputs changed, now stale: {}", dirtied.join(", "))
                }
            };
            eprintln!("built {item}: {changes}");
            report.push(json!({
                "item": item.to_string(),
                "packages": outcome.stored.files,
                "buildinfo": outcome.stored.buildinfo,
                "unchanged": matches!(cutoff, Cutoff::Unchanged),
            }));
            if !cx.json {
                for file in &outcome.stored.files {
                    say(cx.root.out().join(file).display());
                }
            }
        }
    });
    if let Err(error) = result {
        if shell_on_failure
            && let Some(item) = &current
            && let Some(recipe) = graph.recipe(item)
        {
            eprintln!("laft-build: {error}");
            let key = builder
                .key_inputs(recipe)
                .ok()
                .map(|inputs| inputs.build_key())
                .filter(|key| builder.work_dir(key).is_dir());
            if let Some(key) = key {
                eprintln!("opening a shell in the sandbox of {item}; exit it to continue");
                let shell = builder
                    .executor(recipe, &key)
                    .interactive(&recipe.environment(builder.jobs));
                if let Err(shell) = shell {
                    eprintln!("laft-build: cannot open the shell: {shell}");
                }
            } else {
                eprintln!(
                    "laft-build: {item} failed before its working tree existed, so no shell opens"
                );
            }
        }
        return Err(error.into());
    }
    if cx.json {
        say(json!({ "built": report }));
    }
    Ok(())
}

fn fetch(cx: &Context, packages: &[String], mirrors: Vec<String>) -> Result<(), Failure> {
    let mut builder = builder(cx, Mode::Store, None);
    builder.mirrors = mirrors;
    let mut files = Vec::new();
    for recipe in recipes(cx, packages)? {
        for path in builder.fetch(&recipe)? {
            files.push(path.display().to_string());
        }
    }
    if cx.json {
        say(json!({ "sources": files }));
    } else {
        files.iter().for_each(say);
    }
    Ok(())
}

fn shell(
    cx: &Context,
    package: &str,
    after: Option<&str>,
    options: &BuildOptions,
) -> Result<(), Failure> {
    let recipe = one(recipes(cx, &[package.to_owned()])?, package)?;
    let builder = builder(cx, Mode::Store, Some(options));
    let key = builder.key_inputs(&recipe)?.build_key();
    let session = builder.session(&recipe, &key)?;
    if let Some(after) = after {
        builder.phases(&session, &recipe, Some(after))?;
    }
    eprintln!("working tree: {}", session.dir().display());
    session.exec().interactive(session.env())?;
    Ok(())
}

fn one(mut recipes: Vec<Recipe>, package: &str) -> Result<Recipe, Failure> {
    match recipes.len() {
        0 => Err(Failure::Usage(format!(
            "{package} builds nothing on this architecture"
        ))),
        1 => Ok(recipes.remove(0)),
        _ => Err(Failure::Usage(format!(
            "{package} builds several variants: name one, for example {package}:static"
        ))),
    }
}

fn repro(cx: &Context, package: &str) -> Result<(), Failure> {
    check_space(cx.root.path())?;
    let recipe = one(recipes(cx, &[package.to_owned()])?, package)?;
    let item = Item::of(&recipe);
    let db = cx.root.db();
    let previous = db.latest(&item)?.ok_or_else(|| {
        Failure::Build(BuildError::problem(
            "B060",
            format!("{item} has no successful build to check"),
        ))
    })?;
    let buildinfo = fs::read(cx.root.out().join(&previous.buildinfo))
        .ok()
        .and_then(|b| Buildinfo::from_json(&b).ok());
    let jobs = buildinfo.as_ref().map_or(2, |b| b.host.jobs);
    let mut second = builder(cx, Mode::Store, None);
    second.pinned_view = buildinfo.map(|b| b.build_view.iter().map(|p| p.id).collect());
    let base = cx.root.path().join("repro");
    second.work = base.join("work");
    second.out = base.join("out");
    second.logs = base.join("logs");
    second.jobs = (jobs / 2).max(1);
    second.add_to_store = false;
    let report = check_repro(&second, &recipe, &previous, &cx.root.out())?;
    record_repro(&db, &item, &report)?;
    if cx.json {
        let value = serde_json::to_value(&report).map_err(|e| Failure::Usage(e.to_string()))?;
        say(value);
    } else if report.reproducible {
        say(format_args!("{item}: reproducible"));
    } else {
        say(format_args!("{item}: not reproducible"));
        for file in &report.differing_files {
            say(format_args!(
                "  {} {}: {} / {}",
                file.package,
                file.path,
                file.first.as_deref().unwrap_or("missing"),
                file.second.as_deref().unwrap_or("missing")
            ));
        }
    }
    Ok(())
}

fn lint(cx: &Context, packages: &[String]) -> Result<(), Failure> {
    let names: Vec<String> = if packages.is_empty() {
        cx.repos.packages().map(ToString::to_string).collect()
    } else {
        packages.to_vec()
    };
    let mut failed = false;
    let mut results = Vec::new();
    for name in names {
        let parsed = name
            .parse()
            .map_err(|e| Failure::Usage(format!("{name}: {e}")))?;
        let (ok, text) = match cx.repos.evaluate(&parsed) {
            Ok(evaluation) => (true, evaluation.warnings.to_string()),
            Err(e) => (false, e.to_string()),
        };
        failed |= !ok;
        if !cx.json && !text.trim().is_empty() {
            eprintln!("{name}:\n{}", text.trim_end());
        }
        results.push(json!({ "package": name, "ok": ok, "messages": text }));
    }
    if cx.json {
        say(json!({ "lint": results }));
    }
    if failed {
        return Err(Failure::Usage("some recipes have errors".into()));
    }
    Ok(())
}

fn bootstrap(
    cx: &Context,
    root: &Path,
    install: Option<&Path>,
    search: Vec<PathBuf>,
    dry_run: bool,
    options: &BuildOptions,
    packages: &[String],
) -> Result<(), Failure> {
    if !dry_run {
        check_space(cx.root.path())?;
    }
    let search = search
        .iter()
        .map(|d| absolute(d))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(dir) = search.iter().find(|d| d.to_string_lossy().contains(':')) {
        return Err(Failure::Usage(format!(
            "{} contains a colon, which PATH cannot hold",
            dir.display()
        )));
    }
    let install = install.map(absolute).transpose()?;
    let install = install.as_deref();
    let mut builder = builder(cx, Mode::Bootstrap(absolute(root)?), Some(options));
    builder.check = false;
    builder.search = search;
    let db = cx.root.db();
    let mut report = Vec::new();
    for text in packages {
        for recipe in recipes(cx, std::slice::from_ref(text))? {
            let item = Item::of(&recipe);
            let inputs = builder.key_inputs(&recipe)?;
            let state = if let Some(dest) = install {
                if laft_build::installed(dest, &item)? == Some(inputs.build_key()) {
                    "up to date"
                } else if dry_run {
                    "would install"
                } else {
                    eprintln!("installing {item} into {}", dest.display());
                    builder.install(&recipe, &inputs, dest)?;
                    "installed"
                }
            } else if db.find(&inputs.build_key())?.is_some() {
                "up to date"
            } else if dry_run {
                "would build"
            } else {
                eprintln!("building {item} in bootstrap mode");
                let outcome = builder.build(&recipe, &inputs)?;
                db.record(&outcome.stored)?;
                for warning in &outcome.warnings {
                    eprintln!("{item}: {warning}");
                }
                "built"
            };
            if !cx.json {
                say(format_args!("{item}: {state}"));
            }
            report.push(json!({ "item": item.to_string(), "state": state }));
        }
    }
    if cx.json {
        say(json!({ "bootstrap": report }));
    }
    Ok(())
}

/// How `stage` runs, besides the options of builds.
struct StageRun {
    min_free_gib: u64,
    dry_run: bool,
    rebuild: bool,
}

fn stage(
    build_root: &BuildRoot,
    file: &Path,
    root: PathBuf,
    run: &StageRun,
    options: &BuildOptions,
    json: bool,
) -> Result<(), Failure> {
    let dry_run = run.dry_run;
    let stage = Stage::load(file).map_err(|e| Failure::Usage(e.to_string()))?;
    if !dry_run {
        check_space(build_root.path())?;
    }
    let repos = Repos::open(&[&stage.recipes]).map_err(|e| Failure::Usage(e.to_string()))?;
    let cache = SourceCache::new(
        build_root.sources(),
        Fetcher::new(Config::new(build_root.cache())),
    );
    let laft_build = std::env::current_exe()
        .map_err(|e| Failure::Environment(format!("cannot find laft-build itself: {e}")))?;
    let options = StageOptions {
        root: absolute(&root)?,
        jobs: options.jobs.unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(1))
        }),
        timeout: Duration::from_secs(options.timeout_hours * 3600),
        min_available: run.min_free_gib << 30,
        dry_run,
        rebuild: run.rebuild,
        laft_build,
        mirrors: options.mirrors.clone(),
    };
    let mut report = Vec::new();
    let result = run_stage(
        &stage,
        &repos,
        &cache,
        build_root,
        &options,
        &mut |event| match event {
            StageEvent::Start { step, what } => eprintln!("{step}: {what}: starting"),
            StageEvent::Waiting { available, needed } => eprintln!(
                "waiting for memory: {} MiB available, {} MiB needed",
                available >> 20,
                needed >> 20
            ),
            StageEvent::Finished {
                step,
                what,
                state,
                elapsed,
            } => {
                if !json {
                    say(format_args!(
                        "{step}: {what}: {} ({}s)",
                        state.as_str(),
                        elapsed.as_secs()
                    ));
                }
                report.push(json!({
                    "step": step,
                    "what": what,
                    "state": state.as_str(),
                    "seconds": elapsed.as_secs(),
                }));
            }
        },
    );
    if json {
        say(json!({ "stage": stage.name, "steps": report }));
    }
    result.map_err(|e| match e.code() {
        Some(code) if code.starts_with('R') => Failure::Usage(e.to_string()),
        _ => Failure::from(e),
    })
}

/// `path` made absolute against the working directory, because builds
/// run their phases in other directories.
fn absolute(path: &Path) -> Result<PathBuf, Failure> {
    std::path::absolute(path).map_err(|e| Failure::Usage(format!("{}: {e}", path.display())))
}

fn gc(root: &BuildRoot, sources_older_than: Option<u64>) -> Result<(), Failure> {
    let remove = |path: &Path| -> Result<(), Failure> {
        remove_tree(path).map_err(|e| Failure::Environment(format!("{}: {e}", path.display())))
    };
    for dir in ["work", "out", "logs"]
        .iter()
        .map(|d| root.path().join("repro").join(d))
        .chain([root.work(), root.tmp()])
    {
        remove(&dir)?;
    }
    let mut removed = 0;
    if let Some(days) = sources_older_than {
        let limit = SystemTime::now()
            .checked_sub(Duration::from_secs(days * 86_400))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let dir = root.sources().join("sha256");
        for item in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let used = item
                .metadata()
                .and_then(|m| m.accessed().or_else(|_| m.modified()))
                .unwrap_or(SystemTime::now());
            if used < limit {
                remove(&item.path())?;
                removed += 1;
            }
        }
    }
    say(format_args!(
        "removed working trees and reproducibility check outputs{}",
        if sources_older_than.is_some() {
            format!(" and {removed} sources")
        } else {
            String::new()
        }
    ));
    Ok(())
}

/// Removes a file or a tree whose directories may be read-only.
fn remove_tree(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        other => other?,
    };
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    for item in fs::read_dir(path)? {
        remove_tree(&item?.path())?;
    }
    fs::remove_dir(path)
}
