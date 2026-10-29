//! Planning how running services follow a switch to a new generation, and
//! applying the plan to runit.
//!
//! [`plan`] is pure data. [`apply`] executes it against the runit
//! supervisor of a running system.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::runit::Unit;
use crate::{Launcher, OnChange, ServiceKind, graph};

/// How long a new service directory may take until its supervisor
/// accepts commands. runsvdir scans its directory every five seconds.
const SUPERVISOR_WAIT: Duration = Duration::from_secs(7);

/// How often the ready markers and the supervisor are looked at.
const POLL: Duration = Duration::from_millis(100);

/// How a service differs between the old and the new generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Only the new generation has it.
    Added,
    /// Only the old generation has it.
    Removed,
    /// Its backend files differ, so it is redefined.
    Restructured,
    /// Only its spec differs, and it follows this policy.
    Changed(OnChange),
    /// Nothing differs.
    Unchanged,
}

/// One step that brings a service up in the new generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// Start the service.
    Start(String),
    /// Run the oneshot's `down` command, if any, then its `up` command.
    Rerun(String),
}

impl Start {
    /// The service the step is about.
    pub fn name(&self) -> &str {
        match self {
            Self::Start(name) | Self::Rerun(name) => name,
        }
    }
}

/// What reconciliation does, in the order it does it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Every service of either generation, by name.
    pub classes: BTreeMap<String, Class>,
    /// Services to stop, dependents before their dependencies in the old
    /// generation. Stopping one through the supervisor sends its stop
    /// signal, through the control hook of its service directory when that
    /// signal is not `SIGTERM`.
    pub stop: Vec<String>,
    /// Services whose service directory leaves the supervisor's scan
    /// directory, by name.
    pub remove: Vec<String>,
    /// Services whose service directory of the new generation enters the
    /// scan directory, by name.
    pub add: Vec<String>,
    /// Services to start or rerun, dependencies before their dependents in
    /// the new generation.
    pub start: Vec<Start>,
    /// The services to wait for after starting, in start order, each with
    /// the milliseconds to wait for its ready marker.
    pub wait: Vec<(String, u64)>,
    /// Services that changed in a way that only a reboot applies, by name.
    pub reboot: Vec<String>,
}

/// Plans how the running services of the `old` generation follow a switch
/// to the `new` one.
///
/// A service whose backend files changed is stopped, redefined and started,
/// except an early service whose policy is to ignore changes, which is only
/// redefined. A service whose spec alone changed follows its `on_change`
/// policy. Stops happen in reverse dependency order of the old generation,
/// starts in dependency order of the new one.
///
/// Services of the early boot graph are never stopped or started, because
/// no supervisor runs them: stage 1 starts them in a fixed order at boot.
/// The one change applied live is the rerun of a changed early oneshot,
/// whose `down` and `up` commands the launcher runs directly. Every other
/// change that would stop or start an early service is listed in
/// [`Plan::reboot`] instead.
///
/// ```
/// use laft_svc::{EARLY, Service, ServiceConfig, ServiceSet, TURNSTILED, reconcile, runit};
///
/// let mut config = ServiceConfig::default();
/// let mut early = Service::oneshot(&["/usr/bin/true"]);
/// early.early = true;
/// config.services.insert(EARLY.into(), early);
/// config.services.insert(TURNSTILED.into(), Service::longrun(&["/usr/bin/turnstiled"]));
/// let set = ServiceSet::new(&config).expect("the services are valid");
/// let units = runit::render(&set, 1)?.units().clone();
/// let plan = reconcile::plan(&units, &units);
/// assert!(plan.stop.is_empty() && plan.start.is_empty());
/// # Ok::<(), laft_svc::Error>(())
/// ```
pub fn plan(old: &BTreeMap<String, Unit>, new: &BTreeMap<String, Unit>) -> Plan {
    let mut plan = Plan::default();
    let mut stop = BTreeSet::new();
    let mut start = BTreeMap::new();
    let names: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for name in names {
        let class = match (old.get(name), new.get(name)) {
            (None, Some(_)) => Class::Added,
            (Some(_), None) => Class::Removed,
            (Some(o), Some(n)) if o.unit_hash != n.unit_hash => Class::Restructured,
            (Some(o), Some(n)) if o.spec_hash != n.spec_hash => Class::Changed(n.spec.on_change),
            _ => Class::Unchanged,
        };
        let supervised =
            |units: &BTreeMap<String, Unit>| units.get(name).is_some_and(|u| !u.spec.early);
        let redefine = matches!(class, Class::Added | Class::Removed | Class::Restructured);
        if redefine && supervised(old) {
            plan.remove.push(name.clone());
        }
        if redefine && supervised(new) {
            plan.add.push(name.clone());
        }
        let oneshot = new
            .get(name)
            .is_some_and(|u| u.spec.kind == ServiceKind::Oneshot);
        let early = [old.get(name), new.get(name)]
            .into_iter()
            .flatten()
            .any(|u| u.spec.early);
        match class {
            Class::Changed(OnChange::Ignore) | Class::Unchanged => {}
            Class::Changed(OnChange::Reboot) => plan.reboot.push(name.clone()),
            Class::Changed(OnChange::Rerun) if oneshot => {
                start.insert(name.as_str(), Start::Rerun(name.clone()));
            }
            Class::Restructured
                if new[name].spec.early && new[name].spec.on_change == OnChange::Ignore => {}
            _ if early => plan.reboot.push(name.clone()),
            Class::Added => {
                start.insert(name.as_str(), Start::Start(name.clone()));
            }
            Class::Removed => {
                stop.insert(name.as_str());
            }
            Class::Restructured | Class::Changed(OnChange::Restart | OnChange::Rerun) => {
                stop.insert(name.as_str());
                start.insert(name.as_str(), Start::Start(name.clone()));
            }
        }
        plan.classes.insert(name.clone(), class);
    }
    plan.stop = dependency_order(old)
        .into_iter()
        .rev()
        .filter(|n| stop.contains(n))
        .map(str::to_owned)
        .collect();
    for name in dependency_order(new) {
        if let Some(step) = start.remove(name) {
            plan.wait
                .push((name.to_owned(), new[name].spec.ready_timeout_ms()));
            plan.start.push(step);
        }
    }
    plan
}

/// What applying a plan to runit did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Services that did not become ready before their timeout.
    pub not_ready: Vec<String>,
    /// Steps that failed, as the service and why.
    pub failed: Vec<(String, String)>,
}

/// Applies `plan` to the runit supervisor of the system below `root`,
/// once the system has switched to the generation whose rendered services
/// are in `services_dir`.
///
/// Services are stopped with `sv stop` in plan order, their service
/// directories leave the scan directory `run/runit/service`, the new ones
/// are copied in from `services_dir/runit/sv`, and the services to start
/// are brought up with `sv up` in dependency order. A changed early
/// oneshot is rerun through the launcher. Then every started service gets
/// until its timeout to create its ready marker in `run/runit/ready`.
/// `sv` is `usr/bin/sv` below `root`.
///
/// A step that fails is recorded and the others still run, so a failure
/// never leaves the remaining services in the old state.
pub fn apply(plan: &Plan, root: &Path, services_dir: &Path) -> Applied {
    let scan = root.join("run/runit/service");
    let sv = root.join("usr/bin/sv");
    let mut applied = Applied::default();
    let mut fail = |name: &str, why: String| applied.failed.push((name.to_owned(), why));
    for name in &plan.stop {
        let dir = scan.join(name);
        if dir.exists()
            && let Err(why) = run_sv(&sv, "stop", &dir)
        {
            fail(name, why);
        }
    }
    for name in &plan.remove {
        let dir = scan.join(name);
        match fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                fail(name, format!("removing {}: {e}", dir.display()));
            }
            _ => {}
        }
    }
    for name in &plan.add {
        let from = services_dir.join("runit/sv").join(name);
        if let Err(e) = copy_tree(&from, &scan.join(name)) {
            fail(name, format!("adding {}: {e}", from.display()));
        }
    }
    let launcher = Launcher::new(root);
    let mut started = BTreeSet::new();
    for step in &plan.start {
        let result = match step {
            Start::Start(name) => {
                let dir = scan.join(name);
                let deadline = Instant::now() + SUPERVISOR_WAIT;
                loop {
                    match run_sv(&sv, "up", &dir) {
                        Ok(()) => break Ok(()),
                        Err(_) if Instant::now() < deadline => std::thread::sleep(POLL),
                        Err(why) => break Err(why),
                    }
                }
            }
            Start::Rerun(name) => launcher
                .down(name)
                .and_then(|_| launcher.up(name))
                .map(|_| ())
                .map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => {
                started.insert(step.name().to_owned());
            }
            Err(why) => fail(step.name(), why),
        }
    }
    let ready = root.join("run/runit/ready");
    let since = Instant::now();
    for (name, timeout) in &plan.wait {
        if !started.contains(name) {
            continue;
        }
        let marker = ready.join(name);
        while !marker.exists() && since.elapsed() < Duration::from_millis(*timeout) {
            std::thread::sleep(POLL);
        }
        if !marker.exists() {
            applied.not_ready.push(name.clone());
        }
    }
    applied
}

/// Runs `sv <command> <dir>` and returns its complaint when it fails.
fn run_sv(sv: &Path, command: &str, dir: &Path) -> Result<(), String> {
    let output = Command::new(sv)
        .arg(command)
        .arg(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", sv.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(&output.stdout);
        let text = text.trim();
        Err(format!("sv {command} failed: {text}"))
    }
}

/// Copies the directory tree `from` to `to`, keeping modes and symlinks.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for item in fs::read_dir(from)? {
        let item = item?;
        let (source, target) = (item.path(), to.join(item.file_name()));
        let meta = fs::symlink_metadata(&source)?;
        if meta.file_type().is_symlink() {
            symlink(fs::read_link(&source)?, &target)?;
        } else if meta.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
        if !meta.file_type().is_symlink() {
            fs::set_permissions(
                &target,
                fs::Permissions::from_mode(meta.permissions().mode()),
            )?;
        }
    }
    Ok(())
}

/// The services in dependency order, ties broken by name. A cycle, which
/// only damaged input can contain, falls back to name order.
fn dependency_order(units: &BTreeMap<String, Unit>) -> Vec<&str> {
    let graph: BTreeMap<&str, Vec<&str>> = units
        .iter()
        .map(|(n, u)| {
            (
                n.as_str(),
                u.spec.after.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    graph::order(&graph, |_| 0).unwrap_or_else(|_| graph.keys().copied().collect())
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
