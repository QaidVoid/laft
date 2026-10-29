//! `laft plan`, `laft apply`, `laft upgrade` and `laft view build`: from
//! the configuration to a new, activated generation.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use laft_activate::{Assembly, Host};
use laft_config::Loaded;
use laft_core::Name;
use laft_resolve::Upgrade;
use laft_store::{
    GenerationInput, IndexRecord, Init, Lock, PackageInput, Plan as TxPlan, Transaction, ViewInput,
};
use serde_json::json;

use crate::activate::{self, Mode};
use crate::cli::{ApplyArgs, PlanArgs};
use crate::env::{Env, RepoIndex, SyncMode};
use crate::failure::{Failure, exit};
use crate::plan::{self, Plan};
use crate::resolve::{self, InstalledMetas, Resolved};
use crate::ui;

/// What an apply is asked to do.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// The command, for messages and the journal: `apply`, `upgrade` or
    /// `view build`.
    pub(crate) command: &'static str,
    /// The options.
    pub(crate) args: ApplyArgs,
    /// Which packages may move to newer versions.
    pub(crate) upgrade: Upgrade,
    /// Views to assemble again even when nothing about them changed.
    pub(crate) rebuild: BTreeSet<Name>,
}

/// The upgrade mode of `--upgrade`.
fn upgrade_mode(all: bool) -> Upgrade {
    if all { Upgrade::All } else { Upgrade::None }
}

/// The plan of `env`'s configuration and the inputs it was computed from.
struct Planned<'i> {
    loaded: Loaded,
    services: laft_svc::ServiceSet,
    plan: Plan<'i>,
    resolved: Resolved<'i>,
}

fn view_access(loaded: &Loaded) -> BTreeMap<Name, (Vec<String>, Vec<String>)> {
    loaded
        .config
        .views
        .iter()
        .map(|(name, view)| {
            let users = view.users.iter().cloned().collect();
            let groups = view.groups.iter().cloned().collect();
            (name.clone(), (users, groups))
        })
        .collect()
}

/// Resolves and plans. The resolutions borrow from `indexes` and
/// `installed`, which the caller keeps alive.
fn compute<'i>(
    env: &Env,
    loaded: Loaded,
    indexes: &'i [RepoIndex],
    installed: &'i InstalledMetas,
    active: Option<laft_store::Generation>,
    upgrade: &Upgrade,
) -> Result<Planned<'i>, Failure> {
    let services = resolve::services(&loaded.config)?;
    let universe = resolve::universe(indexes);
    let host = host(env, &loaded.config);
    let resolved = resolve::resolve(
        &universe,
        &loaded.config,
        &services,
        installed,
        upgrade,
        &host,
    )?;
    let mut names = vec!["system".to_owned()];
    names.extend(resolved.views.keys().map(ToString::to_string));
    env.ui.phase("resolving", names.join(", "));
    for warning in resolved
        .system
        .warnings
        .iter()
        .chain(resolved.views.values().flat_map(|r| r.warnings.iter()))
    {
        let text = warning.to_string();
        let text = text
            .split_once("]: ")
            .map_or(text.as_str(), |(_, rest)| rest);
        env.ui.warn_code(warning.code(), text);
    }
    let plan = plan::plan(&plan::Inputs {
        root: &env.root,
        active: active.as_ref(),
        resolved: &resolved,
        indexes,
        config: &loaded.value,
        services: &services,
        view_access: view_access(&loaded),
    })?;
    Ok(Planned {
        loaded,
        services,
        plan,
        resolved,
    })
}

/// `laft plan`: prints what `laft apply` would do and writes nothing but
/// synced indexes.
pub(crate) fn plan_command(env: &Env, args: &PlanArgs) -> Result<u8, Failure> {
    let mode = match (env.can_write(), args.refresh) {
        (false, _) => SyncMode::Cached,
        (true, true) => SyncMode::Always,
        (true, false) => SyncMode::IfStale,
    };
    let (loaded, indexes) = {
        let lock = if mode == SyncMode::Cached {
            None
        } else {
            Some(env.lock("plan")?)
        };
        env.load_with_indexes(mode, lock.as_ref())?
    };
    if mode == SyncMode::Cached {
        for repo in &indexes {
            if let Some(synced) = repo.synced {
                let age = std::time::SystemTime::now()
                    .duration_since(synced)
                    .unwrap_or_default();
                env.ui.note(format!(
                    "using the cached index of {} from {} ago",
                    repo.name,
                    ui::duration(age)
                ));
            }
        }
    }
    let active = env.active()?.map(|n| env.generation(n)).transpose()?;
    let installed = InstalledMetas::of(&env.root, active.as_ref());
    let planned = compute(
        env,
        loaded,
        &indexes,
        &installed,
        active,
        &upgrade_mode(args.upgrade),
    )?;
    if env.ui.json {
        env.ui.doc(planned.plan.to_json());
    } else {
        env.ui.out(planned.plan.render(&env.ui).trim_end());
    }
    Ok(if args.exit_code && planned.plan.has_changes() {
        exit::CHANGES
    } else {
        0
    })
}

/// `laft apply` and the commands built on it.
pub(crate) fn apply(env: &Env, request: &Request) -> Result<u8, Failure> {
    let args = &request.args;
    if args.dry_run {
        return plan_command(
            env,
            &PlanArgs {
                upgrade: args.upgrade,
                refresh: args.refresh,
                diff: args.diff,
                adopt: args.adopt.clone(),
                exit_code: false,
            },
        );
    }
    env.require_write(request.command)?;
    env.ui.start(request.command);
    let lock = env.lock(request.command)?;
    let mode = if args.refresh {
        SyncMode::Always
    } else {
        SyncMode::IfStale
    };
    let (loaded, indexes) = env.load_with_indexes(mode, Some(&lock))?;
    let active = env.active()?.map(|n| env.generation(n)).transpose()?;
    let installed = InstalledMetas::of(&env.root, active.as_ref());
    let planned = compute(env, loaded, &indexes, &installed, active, &request.upgrade)?;
    let plan = &planned.plan;
    let forced = args.force || !request.rebuild.is_empty();
    if plan.nothing_to_do && !forced {
        env.ui
            .event(json!({"event": "plan", "plan": plan.to_json()}));
        env.ui
            .event(json!({"event": "result", "generation": plan.previous,
            "active": true, "default": true, "nothing_to_do": true}));
        if !env.ui.json {
            env.ui.out("system is up to date");
        }
        return Ok(0);
    }
    if env.ui.json {
        env.ui
            .event(json!({"event": "plan", "plan": plan.to_json()}));
    } else if !env.ui.quiet {
        env.ui.out(plan.render(&env.ui).trim_end());
        env.ui.out("");
    }
    env.ui.confirm("proceed?")?;
    build_and_activate(env, &lock, request, &planned, &indexes)
}

fn build_and_activate(
    env: &Env,
    lock: &Lock,
    request: &Request,
    planned: &Planned<'_>,
    indexes: &[RepoIndex],
) -> Result<u8, Failure> {
    let args = &request.args;
    let config = &planned.loaded.config;
    let plan = &planned.plan;
    let new_views: BTreeSet<Name> = planned
        .resolved
        .views
        .keys()
        .filter(|name| {
            request.rebuild.contains(*name)
                || plan
                    .views
                    .iter()
                    .any(|v| v.name == name.as_str() && v.keep.is_none())
        })
        .cloned()
        .collect();
    let mut tx = Transaction::begin(
        lock,
        &TxPlan {
            command: request.command.to_owned(),
            previous: plan.previous,
            views: new_views.clone(),
        },
    )?;

    let fetcher = env.fetcher();
    let started = Instant::now();
    let total = plan.download_bytes;
    let mut done = 0;
    let mut files = Vec::new();
    for item in &plan.fetch {
        let mirrors = &indexes[item.repo].mirrors;
        let base = done;
        let path = fetcher.download_package(mirrors, item.entry, &mut |p| {
            env.ui.progress("fetching", base + p.done, total);
            env.ui.event(json!({"event": "progress", "phase": "fetch",
                "done_bytes": base + p.done, "total_bytes": total}));
        })?;
        done += item.entry.size;
        files.push((path, item.entry.id));
    }
    if !plan.fetch.is_empty() {
        env.ui.phase(
            "fetching",
            format!("{}  {}", ui::size(total), ui::duration(started.elapsed())),
        );
        env.ui
            .event(json!({"event": "phase", "phase": "fetch", "status": "done"}));
    }
    for (path, id) in &files {
        tx.add(path, id)?;
    }
    if !files.is_empty() {
        env.ui.phase(
            "storing",
            format!(
                "{} package{}",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
        );
        env.ui
            .event(json!({"event": "phase", "phase": "store", "status": "done"}));
    }

    let started = Instant::now();
    let declared = laft_activate::declare(lock, &config.etc.users)?;
    let mut views = BTreeMap::new();
    for (name, resolution) in &planned.resolved.views {
        if !new_views.contains(name) {
            let keep = plan
                .views
                .iter()
                .find(|v| v.name == name.as_str())
                .and_then(|v| v.keep);
            if let Some(version) = keep {
                views.insert(name.clone(), version);
            }
            continue;
        }
        let view = &config.views[name];
        let ids = |names: &BTreeSet<String>, ids: &dyn Fn(&str) -> Option<u32>| {
            names
                .iter()
                .filter_map(|n| Some((n.clone(), ids(n)?)))
                .collect::<BTreeMap<_, _>>()
        };
        let input = ViewInput {
            name: name.clone(),
            packages: package_inputs(resolution),
            alternatives: resolution.alternatives.clone(),
            users: ids(&view.users, &|n| declared.users.get(n).map(|u| u.uid)),
            groups: ids(&view.groups, &|n| declared.groups.get(n).map(|g| g.gid)),
        };
        views.insert(name.clone(), tx.assemble_view(&input)?);
    }
    let init = config.services.init;
    let input = GenerationInput {
        packages: package_inputs(&planned.resolved.system),
        initrd_packages: package_inputs(&planned.resolved.initrd),
        carry_over: Vec::new(),
        alternatives: planned.resolved.system.alternatives.clone(),
        views,
        message: args.message.clone(),
        laft_version: env!("CARGO_PKG_VERSION").to_owned(),
        arch: crate::env::ARCH.as_str().to_owned(),
        init: Init {
            backend: init.as_str().to_owned(),
            exec: init
                .pid1()
                .unwrap_or_default()
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        },
        config: planned.loaded.value.clone(),
        config_files: planned.loaded.files.clone(),
        indexes: indexes
            .iter()
            .map(|r| IndexRecord {
                repo: r.name.to_string(),
                channel: r.repo.channel.to_string(),
                generation: r.accepted.record.generation,
            })
            .collect(),
        etc: Default::default(),
    };
    let host = host(env, config);
    let assembled = laft_activate::assemble(
        lock,
        &mut tx,
        &Assembly {
            generation: &input,
            etc: &config.etc,
            services: &planned.services,
            boot: &config.boot,
            security: &config.security,
            host: &host,
        },
    )?;
    for warning in &assembled.warnings {
        env.ui.warn(warning.to_string());
    }
    let number = assembled.generation;
    env.ui.phase(
        "assembling",
        format!("generation {number}  {}", ui::duration(started.elapsed())),
    );
    env.ui.event(
        json!({"event": "phase", "phase": "assemble", "status": "done", "generation": number}),
    );

    let esp = env.esp(config);
    activate::install_boot(env, &esp)?;
    tx.boot_installed()?;
    env.ui.phase("boot", format!("entry {number} installed"));

    if args.build_only {
        tx.close()?;
        env.ui.event(
            json!({"event": "result", "generation": number, "active": false,
            "default": false, "nothing_to_do": false}),
        );
        env.ui.note(format!(
            "done        generation {number} is built. Use it with: laft switch {number}"
        ));
        return Ok(0);
    }
    let adopt: BTreeSet<String> = args.adopt.iter().cloned().collect();
    let mode = if args.boot { Mode::Boot } else { Mode::Switch };
    let activation = activate::activate_new(env, lock, tx, number, mode, &adopt)?;
    activate::install_boot(env, &esp)?;
    if config.gc.auto && !args.no_gc {
        activate::auto_gc(env, lock, &config.gc.policy, &esp);
    }
    activation.finish(env, number)
}

/// The machine the generation is assembled for. Below `--root`, the
/// running kernel is not the target's, so none of its modules are carried.
fn host(env: &Env, config: &laft_config::SystemConfig) -> Host {
    let mut host = Host::detect(&config.boot);
    if env.offline_root {
        host.kernel = None;
    }
    host
}

/// The store inputs of a resolution's packages.
fn package_inputs(resolution: &laft_resolve::Resolution<'_>) -> Vec<PackageInput> {
    resolution
        .packages
        .values()
        .map(|s| {
            let meta = s.candidate.meta;
            PackageInput {
                dir: s
                    .candidate
                    .id
                    .dir_name(&meta.name, &meta.version, meta.variant),
                reasons: resolve::reasons(&s.reasons),
            }
        })
        .collect()
}
