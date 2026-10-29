//! `laft boot` and `laft activate --boot`.

use std::fmt::Write as _;
use std::fs;

use laft_activate::BootRecord;
use laft_core::BootState;
use serde_json::{Value, json};

use crate::env::Env;
use crate::failure::Failure;
use crate::gens;
use crate::ui::{self, Sym, table};

/// The boot state of the root, or an empty one.
fn state(env: &Env) -> Result<BootState, Failure> {
    let path = env.path("/laft/state/boot.json");
    match fs::read(&path) {
        Ok(bytes) => BootState::parse(&bytes)
            .map_err(|e| Failure::general(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BootState::default()),
        Err(e) => Err(Failure::general(format!("{}: {e}", path.display()))),
    }
}

/// The trial, good or bad status of `number` in `state`.
fn status(state: &BootState, number: u64) -> &'static str {
    if state.trial.is_some_and(|t| t.generation == number) {
        "trial"
    } else if state.bad.contains(&number) {
        "bad"
    } else if state.good.contains(&number) {
        "good"
    } else {
        "untried"
    }
}

/// `laft boot list`.
pub(crate) fn list(env: &Env) -> Result<u8, Failure> {
    let esp = env.esp(None);
    let entries: Value = fs::read(esp.join("laft/entries.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let default = entries["default_gen"].as_u64();
    let mut rows = Vec::new();
    let mut docs = Vec::new();
    for entry in entries["entries"].as_array().into_iter().flatten() {
        let (Some(number), Some(index)) = (entry["gen"].as_u64(), entry["index"].as_u64()) else {
            continue;
        };
        let record = BootRecord::read(&gens::gen_dir(env, number)).ok();
        let files = record.as_ref().is_some_and(|r| {
            esp.join(format!("laft/{}.vmlinuz", r.kernel_id)).exists()
                && esp.join(format!("laft/{}.initramfs", r.initramfs)).exists()
        });
        let kernel = record.as_ref().map(|r| r.kernel_release.clone());
        docs.push(
            json!({"index": index, "generation": number, "kernel": kernel,
            "default": default == Some(number), "files": files}),
        );
        rows.push(vec![
            index.to_string(),
            number.to_string(),
            kernel.unwrap_or_else(|| "-".into()),
            if default == Some(number) {
                "yes".into()
            } else {
                String::new()
            },
            env.ui.sym(if files { Sym::Pass } else { Sym::Fail }),
        ]);
    }
    if env.ui.json {
        env.ui.doc(json!({"entries": docs}));
    } else {
        env.ui
            .out(table(&["#", "GEN", "KERNEL", "DEFAULT", "FILES"], &rows).trim_end());
    }
    Ok(0)
}

/// `laft boot show`.
pub(crate) fn show(env: &Env, generation: Option<u64>) -> Result<u8, Failure> {
    let number = match generation {
        Some(n) => n,
        None => env
            .booted()?
            .or(env.active()?)
            .ok_or_else(|| Failure::general("no generation is booted"))?,
    };
    env.generation(number)?;
    let record = BootRecord::read(&gens::gen_dir(env, number))?;
    let state = state(env)?;
    let status = status(&state, number);
    if env.ui.json {
        let mut value = serde_json::to_value(&record).map_err(Failure::general)?;
        if let Value::Object(map) = &mut value {
            map.insert("generation".into(), json!(number));
            map.insert("status".into(), json!(status));
        }
        env.ui.doc(value);
        return Ok(0);
    }
    let mut out = String::new();
    let _ = writeln!(out, "generation  {number}");
    let _ = writeln!(out, "status      {status}");
    let _ = writeln!(
        out,
        "kernel      {} ({})",
        record.kernel_release, record.kernel_id
    );
    let _ = writeln!(out, "initramfs   {}", record.initramfs);
    let _ = writeln!(out, "cmdline     {}", record.cmdline.join(" "));
    let _ = writeln!(out, "modules     {}", record.modules.join(" "));
    if !record.firmware.is_empty() {
        let _ = writeln!(out, "firmware    {}", record.firmware.join(" "));
    }
    env.ui.out(out.trim_end());
    Ok(0)
}

/// `laft boot log`.
pub(crate) fn log(env: &Env, generation: Option<u64>) -> Result<u8, Failure> {
    let state = state(env)?;
    let events: Vec<_> = state
        .events
        .iter()
        .filter(|e| generation.is_none_or(|g| e.from == g || e.to == g))
        .collect();
    if env.ui.json {
        let list: Vec<Value> = events
            .iter()
            .map(
                |e| json!({"time": e.time.to_string(), "kind": e.kind, "from": e.from, "to": e.to}),
            )
            .collect();
        env.ui
            .doc(json!({"events": list, "trial": state.trial.map(|t| json!({
            "generation": t.generation, "tries_left": t.tries_left})),
            "last_good": state.last_good}));
        return Ok(0);
    }
    let mut out = String::new();
    if let Some(trial) = state.trial {
        let _ = writeln!(
            out,
            "trial       generation {}, {} tries left",
            trial.generation, trial.tries_left
        );
    }
    if let Some(good) = state.last_good {
        let _ = writeln!(out, "last good   generation {good}");
    }
    for event in events {
        let _ = writeln!(
            out,
            "{}  fell back from generation {} to {}",
            ui::local_time(event.time),
            event.from,
            event.to
        );
    }
    if out.is_empty() {
        out.push_str("no boot events\n");
    }
    env.ui.out(out.trim_end());
    Ok(0)
}

/// `laft boot mark-good`.
pub(crate) fn mark_good(env: &Env) -> Result<u8, Failure> {
    let lock = env.lock("boot mark-good")?;
    let number = laft_activate::mark_booted_good(&lock)?;
    if env.ui.json {
        env.ui.doc(json!({"generation": number, "good": true}));
    } else {
        env.ui
            .note(format!("generation {number} booted completely"));
    }
    Ok(0)
}

/// `laft boot install`.
pub(crate) fn install(env: &Env) -> Result<u8, Failure> {
    env.ui.start("boot install");
    let _lock = env.lock("boot install")?;
    let installed = crate::activate::install_boot(env, &env.esp(None))?;
    env.ui
        .event(json!({"event": "result", "entries": installed.entries,
        "default": installed.default, "copied": installed.copied, "removed": installed.removed}));
    env.ui.note(format!(
        "boot        {} entries, {} files copied, {} removed",
        installed.entries.len(),
        installed.copied.len(),
        installed.removed.len()
    ));
    Ok(0)
}

/// `laft activate --boot`.
pub(crate) fn activate(env: &Env) -> Result<u8, Failure> {
    let lock = env.lock("activate")?;
    let activation = laft_activate::activate_boot(&lock)?;
    for warning in activation
        .etc
        .warnings
        .iter()
        .chain(&activation.accounts.warnings)
    {
        env.ui.warn(warning.to_string());
    }
    if env.ui.json {
        env.ui.doc(json!({"generation": activation.generation,
            "linked": activation.etc.linked, "unlinked": activation.etc.unlinked,
            "removed_users": activation.accounts.removed_users}));
    } else {
        env.ui.note(format!(
            "activated   generation {} at boot",
            activation.generation
        ));
    }
    Ok(0)
}
