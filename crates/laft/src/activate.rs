//! Activating generations: offline below `--root`, live on a running
//! system, at boot, and making them the default.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::SystemTime;

use laft_activate::{BootInstall, OnConflict};
use laft_store::{GcPolicy, Lock, Transaction};
use serde_json::json;

use crate::env::Env;
use crate::failure::{Failure, exit};
use crate::ui::Sym;

/// How a generation is activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Activate it now and make it the default.
    Switch,
    /// Only make it the default for the next boot.
    Boot,
}

/// What activating a generation did.
#[derive(Debug, Default)]
pub(crate) struct Activation {
    /// Whether the generation is the active one now.
    pub(crate) active: bool,
    /// The steps that completed.
    pub(crate) done: Vec<&'static str>,
    /// The steps that failed, with why.
    pub(crate) failed: Vec<(&'static str, String)>,
    /// The services that failed to become ready.
    pub(crate) services_failed: Vec<String>,
}

impl Activation {
    /// Reports the outcome of a command that ends with generation `number`
    /// as the default, and returns the exit status.
    pub(crate) fn finish(&self, env: &Env, number: u64) -> Result<u8, Failure> {
        env.ui.event(
            json!({"event": "result", "generation": number, "active": self.active,
            "default": true, "nothing_to_do": false,
            "failed_steps": self.failed.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            "failed_services": self.services_failed}),
        );
        for (step, why) in &self.failed {
            env.ui.warn(format!("activation step {step} failed: {why}"));
        }
        let state = if self.active {
            "is active and default"
        } else {
            "is the default for the next boot"
        };
        env.ui
            .note(format!("done        generation {number} {state}"));
        Ok(if !self.failed.is_empty() {
            exit::PARTIAL
        } else if !self.services_failed.is_empty() {
            exit::SERVICES
        } else {
            0
        })
    }

    fn report(&self, env: &Env) {
        if self.done.is_empty() && self.failed.is_empty() {
            return;
        }
        let mut line = Vec::new();
        for step in &self.done {
            line.push(format!("{} {step}", env.ui.sym_err(Sym::Pass)));
        }
        for (step, _) in &self.failed {
            line.push(format!("{} {step}", env.ui.sym_err(Sym::Fail)));
        }
        env.ui.phase("activating", line.join(" "));
        let status = if self.failed.is_empty() {
            "done"
        } else {
            "failed"
        };
        env.ui.event(
            json!({"event": "phase", "phase": "activate", "status": status,
            "steps": self.done,
            "failed": self.failed.iter().map(|(s, _)| *s).collect::<Vec<_>>()}),
        );
    }
}

/// Activates the generation `number` that `tx` just built, as `mode`
/// says, and finishes the transaction by making it the default.
pub(crate) fn activate_new(
    env: &Env,
    lock: &Lock,
    tx: Transaction<'_>,
    number: u64,
    mode: Mode,
    adopt: &BTreeSet<String>,
) -> Result<Activation, Failure> {
    let activation = match mode {
        Mode::Switch if env.live() => {
            let journaled = tx.activate(laft_activate::LIVE_STEPS)?;
            let activation = crate::live::activate(env, lock, journaled, adopt)?;
            activation.report(env);
            return Ok(activation);
        }
        Mode::Boot => Activation::default(),
        Mode::Switch => offline(env, lock, number, adopt)?,
    };
    activation.report(env);
    tx.make_default()?;
    Ok(activation)
}

/// Activates the existing generation `number` as `mode` says and makes it
/// the default.
pub(crate) fn switch_to(
    env: &Env,
    lock: &Lock,
    number: u64,
    mode: Mode,
    adopt: &BTreeSet<String>,
) -> Result<Activation, Failure> {
    env.generation(number)?;
    let activation = match mode {
        Mode::Switch if env.live() => {
            let journaled =
                lock.activate("switch", number, env.active()?, laft_activate::LIVE_STEPS)?;
            crate::live::activate(env, lock, journaled, adopt)?
        }
        Mode::Boot => Activation::default(),
        Mode::Switch => offline(env, lock, number, adopt)?,
    };
    activation.report(env);
    if !(mode == Mode::Switch && env.live()) {
        lock.set_default(number)?;
    }
    install_boot(env, &env.esp(None))?;
    Ok(activation)
}

/// Offline activation below `--root`, or on a system that runs no Laft
/// generation: `/etc` and the accounts. Nothing runs that could be
/// switched or restarted.
fn offline(
    env: &Env,
    lock: &Lock,
    number: u64,
    adopt: &BTreeSet<String>,
) -> Result<Activation, Failure> {
    let etc = laft_activate::reconcile_etc(lock, number, adopt, OnConflict::Stop)?;
    for warning in &etc.warnings {
        env.ui.warn(warning.to_string());
    }
    let mut activation = Activation {
        active: true,
        done: vec!["etc"],
        ..Activation::default()
    };
    match laft_activate::apply_accounts(lock, number) {
        Ok(report) => {
            for warning in &report.warnings {
                env.ui.warn(warning.to_string());
            }
            activation.done.push("accounts");
        }
        Err(e) => activation.failed.push(("accounts", e.to_string())),
    }
    Ok(activation)
}

/// Installs the boot files of every generation on the EFI system
/// partition `esp`, which must be a FAT filesystem on a running system.
pub(crate) fn install_boot(env: &Env, esp: &Path) -> Result<BootInstall, Failure> {
    if !env.offline_root {
        laft_activate::check_esp(esp)?;
    }
    Ok(laft_activate::install_boot(&env.root, esp)?)
}

/// Runs garbage collection after an apply. Its failures are warnings.
pub(crate) fn auto_gc(env: &Env, lock: &Lock, policy: &GcPolicy, esp: &Path) {
    let result = laft_store::plan_gc(lock, policy, SystemTime::now())
        .and_then(|plan| plan.execute(lock).map(|()| plan));
    match result {
        Ok(plan) => {
            let deleted = plan.decisions().iter().filter(|d| !d.keep).count();
            if deleted > 0 {
                if let Err(e) = install_boot(env, esp) {
                    env.ui.warn(format!("gc: {}", e.message));
                }
                env.ui.phase("gc", format!("removed {deleted} items"));
            }
        }
        Err(e) => env.ui.warn(format!("gc: {e}")),
    }
}
