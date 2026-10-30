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

use serde::{Deserialize, Serialize};

use crate::runit::Unit;
use crate::{ETC_INPUTS, Launcher, OnChange, ServiceKind, graph};

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
    /// Only its spec or the `/etc` files it reads differ, and it follows
    /// this policy.
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
    /// the milliseconds to wait for its ready marker after its start.
    pub wait: Vec<(String, u64)>,
    /// The dependencies of every service to start in the new generation,
    /// by name.
    pub after: BTreeMap<String, Vec<String>>,
    /// Services that changed in a way that only a reboot applies, by name.
    pub reboot: Vec<String>,
}

/// The services of [`ETC_INPUTS`] whose inputs differ between the `etc`
/// trees `old` and `new` of two generations. An input that one tree lacks
/// differs from any input in the other.
pub fn etc_changed(old: &Path, new: &Path) -> BTreeSet<String> {
    ETC_INPUTS
        .iter()
        .filter(|(_, inputs)| {
            inputs
                .iter()
                .any(|input| content(&old.join(input)) != content(&new.join(input)))
        })
        .map(|(name, _)| (*name).to_owned())
        .collect()
}

/// The content of the file `path`, or of every file directly in the
/// directory `path` by name, or `None` when it cannot be read.
fn content(path: &Path) -> Option<BTreeMap<String, Vec<u8>>> {
    if !path.is_dir() {
        return fs::read(path)
            .ok()
            .map(|bytes| BTreeMap::from([(String::new(), bytes)]));
    }
    fs::read_dir(path)
        .ok()?
        .map(|item| {
            let item = item.ok()?;
            let name = item.file_name().to_string_lossy().into_owned();
            Some((name, fs::read(item.path()).ok()?))
        })
        .collect()
}

/// Plans how the running services of the `old` generation follow a switch
/// to the `new` one. `etc_changed` names the services whose `/etc` input
/// files changed, as [`etc_changed`] finds them.
///
/// A service whose backend files changed is stopped, redefined and started,
/// except an early service whose policy is to ignore changes, which is only
/// redefined. A service whose spec alone changed, or whose input files
/// alone changed, follows its `on_change` policy. Stops happen in reverse dependency order of the old generation,
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
/// let plan = reconcile::plan(&units, &units, &Default::default());
/// assert!(plan.stop.is_empty() && plan.start.is_empty());
/// # Ok::<(), laft_svc::Error>(())
/// ```
pub fn plan(
    old: &BTreeMap<String, Unit>,
    new: &BTreeMap<String, Unit>,
    etc_changed: &BTreeSet<String>,
) -> Plan {
    let mut plan = Plan::default();
    let mut stop = BTreeSet::new();
    let mut start = BTreeMap::new();
    let names: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for name in names {
        let class = match (old.get(name), new.get(name)) {
            (None, Some(_)) => Class::Added,
            (Some(_), None) => Class::Removed,
            (Some(o), Some(n)) if o.unit_hash != n.unit_hash => Class::Restructured,
            (Some(o), Some(n)) if o.spec_hash != n.spec_hash || etc_changed.contains(name) => {
                Class::Changed(n.spec.on_change)
            }
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
            plan.after
                .insert(name.to_owned(), new[name].spec.after.clone());
            plan.start.push(step);
        }
    }
    plan
}

/// What applying a plan to runit did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
/// oneshot is rerun through the launcher. Every started service gets until
/// its own timeout, counted from its own start, to create its ready marker
/// in `run/runit/ready`, and a service starts only once each dependency
/// started before it is ready or has run out of time. `sv` is
/// `usr/bin/sv` below `root`.
///
/// A step that fails is recorded and the others still run, so a failure
/// never leaves the remaining services in the old state. A service whose
/// stop failed keeps its service directory, so that it stays supervised.
pub fn apply(plan: &Plan, root: &Path, services_dir: &Path) -> Applied {
    let scan = root.join("run/runit/service");
    let sv = root.join("usr/bin/sv");
    let mut applied = Applied::default();
    let mut fail = |name: &str, why: String| applied.failed.push((name.to_owned(), why));
    let mut running = BTreeSet::new();
    for name in &plan.stop {
        let dir = scan.join(name);
        if dir.exists()
            && let Err(why) = run_sv(&sv, "stop", &dir)
        {
            fail(name, why);
            running.insert(name);
        }
    }
    for name in plan.remove.iter().filter(|n| !running.contains(n)) {
        let dir = scan.join(name);
        match fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                fail(name, format!("removing {}: {e}", dir.display()));
            }
            _ => {}
        }
    }
    for name in plan.add.iter().filter(|n| !running.contains(n)) {
        let from = services_dir.join("runit/sv").join(name);
        let to = scan.join(name);
        if let Err(e) = copy_tree(&from, &to) {
            let _ = fs::remove_dir_all(&to);
            fail(name, format!("adding {}: {e}", from.display()));
        }
    }
    let launcher = Launcher::new(root);
    let ready = root.join("run/runit/ready");
    let timeouts: BTreeMap<&str, u64> = plan.wait.iter().map(|(n, t)| (n.as_str(), *t)).collect();
    // Started services not yet known to be ready, with their deadlines.
    let mut pending: Vec<(String, Instant)> = Vec::new();
    let mut not_ready = Vec::new();
    let mut settle = |name: &str, pending: &mut Vec<(String, Instant)>| {
        if let Some(i) = pending.iter().position(|(n, _)| n == name) {
            let (name, deadline) = pending.remove(i);
            if !wait_ready(&ready, &name, deadline) {
                not_ready.push(name);
            }
        }
    };
    for step in &plan.start {
        for dep in plan.after.get(step.name()).into_iter().flatten() {
            settle(dep, &mut pending);
        }
        let started = Instant::now();
        let result = match step {
            Start::Start(name) => {
                let dir = scan.join(name);
                let deadline = started + SUPERVISOR_WAIT;
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
                if let Some(timeout) = timeouts.get(step.name()) {
                    let deadline = started + Duration::from_millis(*timeout);
                    pending.push((step.name().to_owned(), deadline));
                }
            }
            Err(why) => fail(step.name(), why),
        }
    }
    while let Some((name, _)) = pending.first().cloned() {
        settle(&name, &mut pending);
    }
    applied.not_ready = not_ready;
    applied
}

/// Waits until the ready marker of service `name` exists in `ready_dir`
/// or `deadline` passes, and tells whether the service became ready. A
/// caller gives every service its own deadline, counted from its own
/// start.
pub fn wait_ready(ready_dir: &Path, name: &str, deadline: Instant) -> bool {
    let marker = ready_dir.join(name);
    loop {
        if marker.exists() {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(POLL));
    }
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
        let text = [&output.stderr, &output.stdout]
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        Err(format!("sv {command} failed: {text}"))
    }
}

/// Copies the directory tree `from` to `to`, keeping modes and symlinks.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let mode = fs::metadata(from)?.permissions().mode();
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
    fs::set_permissions(to, fs::Permissions::from_mode(mode))
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
