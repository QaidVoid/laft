//! Planning how running services follow a switch to a new generation.
//!
//! The plan is pure data. Executing it against the supervisor is a separate
//! step.

use std::collections::{BTreeMap, BTreeSet};

use crate::runit::Unit;
use crate::{OnChange, ServiceKind, graph};

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
    /// generation.
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
/// ```
/// use laft_svc::{ServiceConfig, ServiceSet, reconcile, runit, standard_services};
///
/// let config = ServiceConfig { services: standard_services(), ..ServiceConfig::default() };
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
        match class {
            Class::Added => {
                start.insert(name.as_str(), Start::Start(name.clone()));
            }
            Class::Removed => {
                stop.insert(name.as_str());
            }
            Class::Restructured
                if new[name].spec.early && new[name].spec.on_change == OnChange::Ignore => {}
            Class::Restructured | Class::Changed(OnChange::Restart) => {
                stop.insert(name.as_str());
                start.insert(name.as_str(), Start::Start(name.clone()));
            }
            Class::Changed(OnChange::Rerun) if oneshot => {
                start.insert(name.as_str(), Start::Rerun(name.clone()));
            }
            Class::Changed(OnChange::Rerun) => {
                stop.insert(name.as_str());
                start.insert(name.as_str(), Start::Start(name.clone()));
            }
            Class::Changed(OnChange::Reboot) => plan.reboot.push(name.clone()),
            Class::Changed(OnChange::Ignore) | Class::Unchanged => {}
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
