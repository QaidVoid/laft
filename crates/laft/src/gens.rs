//! `laft gens`, `laft switch`, `laft rollback` and `laft gc`.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::SystemTime;

use laft_activate::BootRecord;
use laft_store::{GcPolicy, Generation, Subject};
use serde_json::{Value, json};

use crate::activate::{self, Mode};
use crate::cli::GcArgs;
use crate::env::Env;
use crate::failure::Failure;
use crate::ui::{self, table};

/// The generation directory of `number`.
pub(crate) fn gen_dir(env: &Env, number: u64) -> std::path::PathBuf {
    env.root.path().join(format!("laft/gens/{number}"))
}

/// The kernel release of generation `number`, if its boot record says.
pub(crate) fn kernel_of(env: &Env, number: u64) -> Option<String> {
    BootRecord::read(&gen_dir(env, number))
        .ok()
        .map(|b| b.kernel_release)
}

/// Every complete generation, oldest first.
pub(crate) fn generations(env: &Env) -> Result<Vec<Generation>, Failure> {
    Ok(env
        .root
        .generations()?
        .into_iter()
        .filter_map(|n| env.root.generation(n).ok())
        .collect())
}

/// `laft gens list`.
pub(crate) fn list(env: &Env) -> Result<u8, Failure> {
    let active = env.active()?;
    let booted = env.booted()?;
    let default = env.root.default_generation()?;
    let mut rows = Vec::new();
    let mut docs = Vec::new();
    for generation in generations(env)?.into_iter().rev() {
        let n = generation.generation;
        let mut flags = Vec::new();
        if active == Some(n) {
            flags.push("*");
        }
        if booted == Some(n) {
            flags.push("b");
        }
        if default == Some(n) {
            flags.push("d");
        }
        let kernel = kernel_of(env, n);
        docs.push(json!({
            "generation": n,
            "created": generation.created.to_string(),
            "flags": flags,
            "packages": generation.packages.len(),
            "kernel": kernel,
            "message": generation.message,
            "parent": generation.parent,
        }));
        rows.push(vec![
            n.to_string(),
            flags.join(" "),
            ui::local_time(generation.created),
            generation.packages.len().to_string(),
            kernel.unwrap_or_else(|| "-".into()),
            generation.message.clone().unwrap_or_default(),
        ]);
    }
    if env.ui.json {
        env.ui.doc(json!({"generations": docs}));
    } else {
        let text = table(
            &["GEN", "FLAGS", "CREATED", "PKGS", "KERNEL", "MESSAGE"],
            &rows,
        );
        env.ui.page(&text);
    }
    Ok(0)
}

/// `laft gens show`.
pub(crate) fn show(env: &Env, number: u64, packages: bool) -> Result<u8, Failure> {
    let generation = env.generation(number)?;
    if env.ui.json {
        let mut value = serde_json::to_value(&generation).map_err(Failure::general)?;
        if let Value::Object(map) = &mut value {
            map.insert("boot".into(), boot_json(env, number));
        }
        env.ui.doc(value);
        return Ok(0);
    }
    let mut out = String::new();
    let _ = writeln!(out, "generation   {number}");
    let parent = generation
        .parent
        .map_or_else(|| "-".into(), |p| p.to_string());
    let _ = writeln!(out, "parent       {parent}");
    let _ = writeln!(out, "created      {}", ui::local_time(generation.created));
    if let Some(message) = &generation.message {
        let _ = writeln!(out, "message      {message}");
    }
    let _ = writeln!(out, "laft         {}", generation.laft_version);
    let _ = writeln!(out, "init         {}", generation.init.backend);
    let kernel = kernel_of(env, number).unwrap_or_else(|| "-".into());
    let _ = writeln!(out, "kernel       {kernel}");
    for index in &generation.indexes {
        let _ = writeln!(
            out,
            "index        {} {} generation {}",
            index.repo, index.channel, index.generation
        );
    }
    let _ = writeln!(out, "packages     {}", generation.packages.len());
    for (name, version) in &generation.views {
        let _ = writeln!(out, "view         {name}@{version}");
    }
    for (group, member) in &generation.alternatives {
        let _ = writeln!(out, "alternative  {group} = {member}");
    }
    for carried in &generation.carried_modules {
        let _ = writeln!(out, "carried      modules of {}", carried.kernel);
    }
    if packages {
        let rows: Vec<Vec<String>> = generation
            .packages
            .iter()
            .map(|p| {
                vec![
                    p.name.to_string(),
                    format!("{}-r{}", p.version, p.rev),
                    p.variant.to_string(),
                ]
            })
            .collect();
        out.push('\n');
        out.push_str(&table(&["PACKAGE", "VERSION", "VARIANT"], &rows));
    }
    env.ui.page(&out);
    Ok(0)
}

fn boot_json(env: &Env, number: u64) -> Value {
    BootRecord::read(&gen_dir(env, number))
        .ok()
        .and_then(|b| serde_json::to_value(b).ok())
        .unwrap_or(Value::Null)
}

/// `laft switch`.
pub(crate) fn switch(env: &Env, number: u64, boot: bool, command: &str) -> Result<u8, Failure> {
    env.require_write(command)?;
    env.ui.start(command);
    let lock = env.lock(command)?;
    let mode = if boot { Mode::Boot } else { Mode::Switch };
    let activation = activate::switch_to(env, &lock, number, mode, &BTreeSet::new())?;
    activation.finish(env, number)
}

/// `laft rollback`.
pub(crate) fn rollback(env: &Env, boot: bool) -> Result<u8, Failure> {
    let active = env
        .active()?
        .ok_or_else(|| Failure::general("no generation is active"))?;
    let generation = env.generation(active)?;
    let parent = generation
        .parent
        .filter(|p| env.root.generation(*p).is_ok());
    let Some(parent) = parent else {
        let available: Vec<String> = env
            .root
            .generations()?
            .iter()
            .map(ToString::to_string)
            .collect();
        return Err(
            Failure::general(format!("generation {active} has no parent to roll back to"))
                .context(format!("available generations: {}", available.join(", ")))
                .hint("switch to one of them with `laft switch <generation>`"),
        );
    };
    switch(env, parent, boot, "rollback")
}

/// `laft gc`.
pub(crate) fn gc(env: &Env, args: &GcArgs) -> Result<u8, Failure> {
    env.require_write("gc")?;
    env.ui.start("gc");
    let lock = env.lock("gc")?;
    let config = env.load(&|_| Ok(()))?.config;
    let configured = config.gc.policy;
    let policy = GcPolicy {
        keep_generations: args.keep_generations.unwrap_or(configured.keep_generations),
        keep_days: args.keep_days.unwrap_or(configured.keep_days),
    };
    let plan = laft_store::plan_gc(&lock, &policy, SystemTime::now())?;
    let (mut gens, mut views, mut entries) = (0, 0, 0);
    for decision in plan.decisions().iter().filter(|d| !d.keep) {
        match decision.subject {
            Subject::Generation(_) => gens += 1,
            Subject::View { .. } => views += 1,
            Subject::Entry { .. } => entries += 1,
        }
    }
    let freed = freed_bytes(env, &plan);
    if args.explain && !env.ui.json {
        env.ui.out(plan.to_string().trim_end());
    }
    let mut boot_removed = 0;
    if !args.dry_run {
        plan.execute(&lock)?;
        let installed = activate::install_boot(env, &env.esp(Some(&config)))?;
        boot_removed = installed.removed.len();
    }
    let decisions: Vec<Value> = plan
        .decisions()
        .iter()
        .map(|d| json!({"subject": d.subject.to_string(), "keep": d.keep, "reasons": d.reasons}))
        .collect();
    env.ui.event(
        json!({"event": "result", "dry_run": args.dry_run, "generations": gens,
        "views": views, "entries": entries, "boot_files": boot_removed,
        "freed_bytes": freed, "decisions": decisions}),
    );
    if !env.ui.json {
        let verb = if args.dry_run {
            "would remove"
        } else {
            "removed"
        };
        env.ui.out(format!(
            "{verb} {gens} generation{}, {views} view version{}, {entries} store entr{}, {boot_removed} boot file{} {} freed {}",
            if gens == 1 { "" } else { "s" },
            if views == 1 { "" } else { "s" },
            if entries == 1 { "y" } else { "ies" },
            if boot_removed == 1 { "" } else { "s" },
            env.ui.dot(),
            ui::size(freed),
        ));
    }
    Ok(0)
}

/// The installed size of the store entries `plan` deletes.
fn freed_bytes(env: &Env, plan: &laft_store::GcPlan) -> u64 {
    let store = env.root.store();
    plan.decisions()
        .iter()
        .filter(|d| !d.keep)
        .filter_map(|d| match &d.subject {
            Subject::Entry { dir, .. } => store.meta(dir).ok().map(|m| m.installed_size),
            _ => None,
        })
        .sum()
}

/// The leaves of a JSON value by dotted path.
fn flatten(value: &Value, prefix: &str, out: &mut std::collections::BTreeMap<String, Value>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(inner, &path, out);
            }
        }
        other => {
            out.insert(prefix.to_owned(), other.clone());
        }
    }
}

/// The `/etc` paths two generations manage that were added, removed or
/// changed in content, relative to `/etc`.
fn etc_changes(
    env: &Env,
    a: &Generation,
    b: &Generation,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let paths = |g: &Generation| -> BTreeSet<String> {
        g.etc.managed.iter().chain(&g.etc.copy).cloned().collect()
    };
    let (old, new) = (paths(a), paths(b));
    let read = |g: &Generation, p: &str| {
        let path = gen_dir(env, g.generation).join("etc").join(p);
        std::fs::read_link(&path)
            .map(|t| t.into_os_string().into_encoded_bytes())
            .or_else(|_| std::fs::read(&path))
            .ok()
    };
    let added = new.difference(&old).cloned().collect();
    let removed = old.difference(&new).cloned().collect();
    let changed = old
        .intersection(&new)
        .filter(|p| read(a, p) != read(b, p))
        .cloned()
        .collect();
    (added, removed, changed)
}

/// `laft gens diff`.
pub(crate) fn diff(env: &Env, a: u64, b: Option<u64>, etc_diffs: bool) -> Result<u8, Failure> {
    let b = match b {
        Some(b) => b,
        None => env
            .active()?
            .ok_or_else(|| Failure::general("no generation is active"))?,
    };
    let (old, new) = (env.generation(a)?, env.generation(b)?);
    let mut doc = serde_json::Map::new();
    let mut out = format!("generation {a} {} {b}\n", env.ui.arrow());

    let mut sections = vec![(
        "system".to_owned(),
        records(&old.packages),
        records(&new.packages),
    )];
    sections.push((
        "initrd".into(),
        records(&old.initrd_packages),
        records(&new.initrd_packages),
    ));
    let names: BTreeSet<&laft_core::Name> = old.views.keys().chain(new.views.keys()).collect();
    for name in names {
        let view = |g: &Generation| {
            g.views
                .get(name)
                .and_then(|v| env.root.view(name, *v).ok())
                .map(|m| records(&m.packages))
                .unwrap_or_default()
        };
        sections.push((format!("view {name}"), view(&old), view(&new)));
    }
    let mut packages = serde_json::Map::new();
    for (title, before, after) in sections {
        let changes = crate::plan::changes(&before, &after);
        packages.insert(title.clone(), crate::plan::changes_json(&changes));
        if !changes.is_empty() {
            out.push_str(&format!("  {title}\n"));
            out.push_str(&crate::plan::render_changes(&env.ui, &changes));
        }
    }
    doc.insert("packages".into(), Value::Object(packages));

    let groups: BTreeSet<&laft_core::Name> = old
        .alternatives
        .keys()
        .chain(new.alternatives.keys())
        .collect();
    let mut alternatives = Vec::new();
    for group in groups {
        let (x, y) = (old.alternatives.get(group), new.alternatives.get(group));
        if x != y {
            let show = |m: Option<&laft_core::Name>| m.map_or("-".to_owned(), ToString::to_string);
            out.push_str(&format!(
                "  alternative {group}: {} {} {}\n",
                show(x),
                env.ui.arrow(),
                show(y)
            ));
            alternatives.push(json!({"group": group, "from": x, "to": y}));
        }
    }
    doc.insert("alternatives".into(), json!(alternatives));

    let (added, removed, changed) = etc_changes(env, &old, &new);
    if !(added.is_empty() && removed.is_empty() && changed.is_empty()) {
        out.push_str("  etc\n");
        for (sym, list) in [
            (crate::ui::Sym::Changed, &changed),
            (crate::ui::Sym::Added, &added),
            (crate::ui::Sym::Removed, &removed),
        ] {
            for path in list {
                out.push_str(&format!("    {} {path}\n", env.ui.sym(sym)));
            }
        }
    }
    doc.insert(
        "etc".into(),
        json!({"added": added, "removed": removed, "changed": changed}),
    );

    let units = |g: &Generation| {
        laft_svc::runit::load_units(&gen_dir(env, g.generation).join("services"))
            .unwrap_or_default()
    };
    let classes = laft_svc::reconcile::plan(&units(&old), &units(&new)).classes;
    let mut services = serde_json::Map::new();
    let mut lines = String::new();
    for (name, class) in classes {
        use laft_svc::reconcile::Class;
        let (sym, kind) = match class {
            Class::Added => (crate::ui::Sym::Added, "added"),
            Class::Removed => (crate::ui::Sym::Removed, "removed"),
            Class::Restructured | Class::Changed(_) => (crate::ui::Sym::Changed, "changed"),
            Class::Unchanged => continue,
        };
        lines.push_str(&format!("    {} {name}\n", env.ui.sym(sym)));
        services.insert(name, json!(kind));
    }
    if !lines.is_empty() {
        out.push_str("  services\n");
        out.push_str(&lines);
    }
    doc.insert("services".into(), Value::Object(services));

    let config = |g: &Generation| {
        std::fs::read(gen_dir(env, g.generation).join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .unwrap_or(Value::Null)
    };
    let (mut x, mut y) = (Default::default(), Default::default());
    flatten(&config(&old), "", &mut x);
    flatten(&config(&new), "", &mut y);
    let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
    let mut config_changes = Vec::new();
    for key in keys {
        let (from, to) = (x.get(key), y.get(key));
        if from != to {
            config_changes.push(json!({"path": key, "from": from, "to": to}));
            let show = |v: Option<&Value>| v.map_or("-".to_owned(), ToString::to_string);
            out.push_str(&format!(
                "  config {key}: {} {} {}\n",
                show(from),
                env.ui.arrow(),
                show(to)
            ));
        }
    }
    doc.insert("config".into(), json!(config_changes));

    if etc_diffs {
        let mut diffs = Vec::new();
        for path in &changed {
            let read = |n: u64| {
                std::fs::read_to_string(gen_dir(env, n).join("etc").join(path)).unwrap_or_default()
            };
            let text = crate::diff::unified(
                &format!("generation {a}: /etc/{path}"),
                &format!("generation {b}: /etc/{path}"),
                &read(a),
                &read(b),
            );
            out.push_str(&text);
            diffs.push(json!({"path": path, "diff": text}));
        }
        doc.insert("etc_diffs".into(), json!(diffs));
    }
    if env.ui.json {
        doc.insert("from".into(), json!(a));
        doc.insert("to".into(), json!(b));
        env.ui.doc(Value::Object(doc));
    } else {
        env.ui.page(&out);
    }
    Ok(0)
}

fn records(
    records: &[laft_store::PackageRecord],
) -> std::collections::BTreeMap<laft_core::Name, crate::plan::Pkg> {
    records
        .iter()
        .map(|p| (p.name.clone(), crate::plan::Pkg::of_record(p)))
        .collect()
}

/// `laft gens rm`.
pub(crate) fn rm(env: &Env, numbers: &[u64], no_gc: bool) -> Result<u8, Failure> {
    env.require_write("gens rm")?;
    env.ui.start("gens rm");
    let lock = env.lock("gens rm")?;
    if let Some(active) = env.active()?.filter(|a| numbers.contains(a)) {
        return Err(Failure::new(
            crate::failure::exit::USAGE,
            format!("generation {active} is active and cannot be deleted"),
        ));
    }
    let config = if no_gc {
        None
    } else {
        Some(env.load(&|_| Ok(()))?.config)
    };
    let esp = env.esp(config.as_ref());
    let list: Vec<String> = numbers.iter().map(ToString::to_string).collect();
    env.ui
        .confirm(&format!("delete generation {}?", list.join(", ")))?;
    laft_store::remove_generations(&lock, numbers)
        .map_err(|e| Failure::new(crate::failure::exit::USAGE, e))?;
    if let Some(config) = &config {
        activate::auto_gc(env, &lock, &config.gc.policy, &esp);
    }
    activate::install_boot(env, &esp)?;
    env.ui.event(json!({"event": "result", "deleted": numbers}));
    env.ui.note(format!(
        "done        deleted generation {}",
        list.join(", ")
    ));
    Ok(0)
}
