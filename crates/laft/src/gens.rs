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
    let configured = env
        .load(&|_| Ok(()))
        .map(|l| l.config.gc.policy)
        .unwrap_or_default();
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
        let installed = activate::install_boot(env, &env.esp_dir())?;
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
