use std::collections::BTreeMap;

use super::{Class, Plan, Start, plan};
use crate::runit::{Unit, render};
use crate::test_support::{config, dbus, example, myapp, valid};
use crate::{Log, OnChange, Ready, Service, ServiceConfig};

fn units(config: &ServiceConfig) -> BTreeMap<String, Unit> {
    render(&valid(config), 1).expect("renders").units().clone()
}

fn reconcile(old: &ServiceConfig, new: &ServiceConfig) -> Plan {
    plan(&units(old), &units(new))
}

fn changed(class: Class, plan: &Plan) -> Vec<&str> {
    plan.classes
        .iter()
        .filter(|&(_, c)| *c == class)
        .map(|(n, _)| n.as_str())
        .collect()
}

#[test]
fn identical_generations_need_nothing() {
    let plan = reconcile(&example(), &example());
    assert_eq!(
        plan,
        Plan {
            classes: plan.classes.clone(),
            ..Plan::default()
        }
    );
    assert!(plan.classes.values().all(|c| *c == Class::Unchanged));
}

#[test]
fn environment_change_restarts_only_that_service() {
    let mut app = myapp();
    app.env.insert("RUST_LOG".into(), "debug".into());
    let plan = reconcile(&example(), &config(&[("dbus", dbus()), ("myapp", app)]));
    assert_eq!(plan.classes["myapp"], Class::Changed(OnChange::Restart));
    assert_eq!(plan.stop, ["myapp"]);
    assert_eq!(plan.start, [Start::Start("myapp".into())]);
    assert!(plan.add.is_empty() && plan.remove.is_empty());
    assert_eq!(plan.wait, [("myapp".to_owned(), 60_000)]);
}

#[test]
fn log_change_restructures_the_service() {
    let mut app = myapp();
    app.log = Log::Console;
    let plan = reconcile(&example(), &config(&[("dbus", dbus()), ("myapp", app)]));
    assert_eq!(changed(Class::Restructured, &plan), ["myapp"]);
    assert_eq!(plan.stop, ["myapp"]);
    assert_eq!(plan.remove, ["myapp"]);
    assert_eq!(plan.add, ["myapp"]);
    assert_eq!(plan.start, [Start::Start("myapp".into())]);
}

#[test]
fn disabled_service_is_stopped_and_removed() {
    let mut app = myapp();
    app.enable = false;
    let plan = reconcile(&example(), &config(&[("dbus", dbus()), ("myapp", app)]));
    assert_eq!(plan.classes["myapp"], Class::Removed);
    assert_eq!(plan.stop, ["myapp"]);
    assert_eq!(plan.remove, ["myapp"]);
    assert!(plan.add.is_empty() && plan.start.is_empty());
}

#[test]
fn added_service_is_added_then_started_with_its_check_timeout() {
    let probe = Service {
        ready: Ready::Check {
            cmd: vec!["/usr/bin/true".into()],
            interval: 1,
            timeout: 5,
        },
        ..Service::longrun(&["/usr/bin/probe"])
    };
    let plan = reconcile(
        &example(),
        &config(&[("dbus", dbus()), ("myapp", myapp()), ("probe", probe)]),
    );
    assert_eq!(plan.classes["probe"], Class::Added);
    assert_eq!(plan.add, ["probe"]);
    assert_eq!(plan.start, [Start::Start("probe".into())]);
    assert_eq!(plan.wait, [("probe".to_owned(), 5000)]);
}

#[test]
fn changed_early_oneshot_is_rerun_without_a_reboot() {
    let mut new = example();
    let hostname = new.services.get_mut("hostname").expect("standard");
    hostname.up.push("--verbose".into());
    let plan = reconcile(&example(), &new);
    assert_eq!(plan.classes["hostname"], Class::Changed(OnChange::Rerun));
    assert!(plan.stop.is_empty());
    assert_eq!(plan.start, [Start::Rerun("hostname".into())]);
    assert!(plan.add.is_empty() && plan.reboot.is_empty());
}

#[test]
fn reboot_policy_only_notes_the_service() {
    let mut app = myapp();
    app.on_change = Some(OnChange::Reboot);
    let old = config(&[("dbus", dbus()), ("myapp", app.clone())]);
    app.cwd = "/srv".into();
    let plan = reconcile(&old, &config(&[("dbus", dbus()), ("myapp", app)]));
    assert_eq!(plan.reboot, ["myapp"]);
    assert!(plan.stop.is_empty() && plan.start.is_empty());
}

#[test]
fn stops_run_against_and_starts_along_the_dependencies() {
    let (mut bus, mut app) = (dbus(), myapp());
    bus.env.insert("X".into(), "1".into());
    app.env.insert("X".into(), "1".into());
    let plan = reconcile(&example(), &config(&[("dbus", bus), ("myapp", app)]));
    assert_eq!(plan.stop, ["myapp", "dbus"]);
    assert_eq!(
        plan.start,
        [Start::Start("dbus".into()), Start::Start("myapp".into())]
    );
}

#[test]
fn restructured_early_service_that_ignores_changes_is_only_redefined() {
    let mut new = example();
    let seed = new.services.get_mut("random-seed").expect("standard");
    seed.kind = crate::ServiceKind::Longrun;
    seed.exec = std::mem::take(&mut seed.up);
    seed.down.clear();
    let plan = reconcile(&example(), &new);
    assert_eq!(plan.classes["random-seed"], Class::Restructured);
    assert!(plan.stop.is_empty() && plan.start.is_empty());
    assert!(plan.add.is_empty() && plan.remove.is_empty());
}
