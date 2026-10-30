use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use super::{Class, Plan, Start, etc_changed, plan};
use crate::runit::{Unit, render};
use crate::test_support::{config, dbus, example, myapp, valid};
use crate::{Log, OnChange, Ready, Service, ServiceConfig};

fn units(config: &ServiceConfig) -> BTreeMap<String, Unit> {
    render(&valid(config), 1).expect("renders").units().clone()
}

fn reconcile(old: &ServiceConfig, new: &ServiceConfig) -> Plan {
    plan(&units(old), &units(new), &BTreeSet::new())
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
fn an_early_oneshot_whose_etc_input_changed_is_rerun() {
    let units = units(&example());
    let plan = plan(&units, &units, &BTreeSet::from(["hostname".to_owned()]));
    assert_eq!(plan.classes["hostname"], Class::Changed(OnChange::Rerun));
    assert_eq!(plan.start, [Start::Rerun("hostname".into())]);
    assert!(plan.stop.is_empty() && plan.reboot.is_empty());
}

#[test]
fn etc_inputs_that_differ_name_their_services() {
    let dir = laft_test::tempdir();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    for (etc, name) in [(&old, "forge\n"), (&new, "anvil\n")] {
        fs::create_dir_all(etc.join("sysctl.d")).expect("writable");
        fs::write(etc.join("hostname"), name).expect("writable");
        fs::write(etc.join("sysctl.d/laft.conf"), "").expect("writable");
    }
    fs::write(new.join("vconsole.conf"), "KEYMAP=de\n").expect("writable");
    fs::create_dir_all(new.join("modules-load.d")).expect("writable");
    fs::write(new.join("modules-load.d/custom.conf"), "kvm\n").expect("writable");
    let changed: Vec<String> = etc_changed(&old, &new).into_iter().collect();
    assert_eq!(changed, ["console", "hostname", "kmod"]);
    assert!(etc_changed(&old, &old).is_empty());
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

#[test]
fn a_changed_early_longrun_waits_for_a_reboot() {
    let mut new = example();
    let udevd = new.services.get_mut("udevd").expect("standard");
    udevd.env.insert("UDEV_LOG".into(), "debug".into());
    let plan = reconcile(&example(), &new);
    assert_eq!(plan.classes["udevd"], Class::Changed(OnChange::Restart));
    assert!(plan.stop.is_empty() && plan.start.is_empty() && plan.wait.is_empty());
    assert_eq!(plan.reboot, ["udevd"]);
}

#[test]
fn a_removed_early_oneshot_waits_for_a_reboot() {
    let mut new = example();
    new.services.get_mut("swap").expect("standard").enable = false;
    let early = new.services.get_mut("early").expect("standard");
    early.after.retain(|dep| dep != "swap");
    let plan = reconcile(&example(), &new);
    assert_eq!(plan.classes["swap"], Class::Removed);
    assert!(plan.stop.is_empty() && plan.start.is_empty());
    assert_eq!(plan.reboot, ["swap"]);
}

/// A root with a fake `sv` that logs its calls and marks services it
/// brings up as ready, and the rendered services of `config`.
fn runit_root(config: &ServiceConfig) -> Option<laft_test::TempDir> {
    if !std::path::Path::new("/bin/sh").exists() {
        return None;
    }
    let dir = laft_test::tempdir();
    let root = dir.path();
    std::fs::create_dir_all(root.join("usr/bin")).expect("writable");
    std::fs::create_dir_all(root.join("run/runit/service")).expect("writable");
    let script = format!(
        "#!/bin/sh\necho \"$1 ${{2##*/}}\" >> {root}/sv.log\n\
         if [ \"$1\" = up ]; then mkdir -p {root}/run/runit/ready && touch {root}/run/runit/ready/${{2##*/}}; fi\n",
        root = root.display()
    );
    laft_test::write_program(&root.join("usr/bin/sv"), script.as_bytes());
    render(&valid(config), 2)
        .expect("renders")
        .write(&root.join("gen"))
        .expect("writes");
    Some(dir)
}

#[test]
fn applying_a_plan_stops_redefines_starts_and_waits() {
    let mut app = myapp();
    app.log = Log::Console;
    let new = config(&[("dbus", dbus()), ("myapp", app)]);
    let Some(dir) = runit_root(&new) else {
        println!("skipping: the host has no /bin/sh for the fake sv");
        return;
    };
    let root = dir.path();
    std::fs::create_dir_all(root.join("run/runit/service/myapp/old")).expect("writable");
    let plan = reconcile(&example(), &new);
    let applied = super::apply(&plan, root, &root.join("gen/services"));
    assert_eq!(applied, super::Applied::default());
    let log = std::fs::read_to_string(root.join("sv.log")).expect("sv ran");
    assert_eq!(log, "stop myapp\nup myapp\n");
    let service = root.join("run/runit/service/myapp");
    assert!(
        service.join("run").exists(),
        "the new directory is in place"
    );
    assert!(!service.join("old").exists(), "the old directory is gone");
}

#[test]
fn a_service_that_never_becomes_ready_is_reported() {
    let new = config(&[("dbus", dbus()), ("myapp", myapp())]);
    let Some(dir) = runit_root(&new) else {
        println!("skipping: the host has no /bin/sh for the fake sv");
        return;
    };
    let root = dir.path();
    laft_test::write_program(&root.join("usr/bin/sv"), b"#!/bin/sh\nexit 0\n");
    let mut plan = reconcile(&example(), &new);
    plan.start = vec![Start::Start("myapp".into())];
    plan.wait = vec![("myapp".into(), 200)];
    let applied = super::apply(&plan, root, &root.join("gen/services"));
    assert_eq!(applied.not_ready, ["myapp"]);
}

/// Writes a fake `sv` below `root` that runs `body` with `$1` the command
/// and `$name` the service.
fn fake_sv(root: &std::path::Path, body: &str) {
    let script = format!(
        "#!/bin/sh\nname=${{2##*/}}\necho \"$1 $name\" >> {root}/sv.log\n{body}\n",
        root = root.display()
    );
    laft_test::write_program(&root.join("usr/bin/sv"), script.as_bytes());
}

#[test]
fn each_service_waits_from_its_own_start_and_dependents_wait_for_it() {
    let new = config(&[("dbus", dbus()), ("myapp", myapp())]);
    let Some(dir) = runit_root(&new) else {
        println!("skipping: the host has no /bin/sh for the fake sv");
        return;
    };
    let root = dir.path();
    let ready = root.join("run/runit/ready");
    std::fs::create_dir_all(&ready).expect("writable");
    // `slow` becomes ready 300 ms after its start, `fast` at once. Each
    // has a 500 ms budget, so a shared clock would fail `fast`.
    fake_sv(
        root,
        &format!(
            "[ \"$1\" = up ] || exit 0\nif [ \"$name\" = slow ]; then (sleep 0.3; touch {r}/slow) & else ls {r} >> {root}/seen.log; touch {r}/$name; fi",
            r = ready.display(),
            root = root.display()
        ),
    );
    let plan = Plan {
        start: vec![Start::Start("slow".into()), Start::Start("fast".into())],
        wait: vec![("slow".into(), 500), ("fast".into(), 500)],
        after: [("fast".to_owned(), vec!["slow".to_owned()])].into(),
        ..Plan::default()
    };
    let applied = super::apply(&plan, root, &root.join("gen/services"));
    assert_eq!(applied, super::Applied::default());
    let seen = std::fs::read_to_string(root.join("seen.log")).expect("fast started");
    assert!(seen.contains("slow"), "fast started before slow was ready");
}

#[test]
fn a_service_whose_stop_failed_keeps_its_directory() {
    let new = config(&[("dbus", dbus())]);
    let Some(dir) = runit_root(&new) else {
        println!("skipping: the host has no /bin/sh for the fake sv");
        return;
    };
    let root = dir.path();
    fake_sv(root, "echo 'fail: myapp: timeout' >&2\nexit 1");
    let service = root.join("run/runit/service/myapp");
    std::fs::create_dir_all(&service).expect("writable");
    let plan = Plan {
        stop: vec!["myapp".into()],
        remove: vec!["myapp".into()],
        ..Plan::default()
    };
    let applied = super::apply(&plan, root, &root.join("gen/services"));
    assert!(service.exists(), "a running service stays supervised");
    assert_eq!(applied.failed.len(), 1);
    assert!(
        applied.failed[0].1.contains("fail: myapp: timeout"),
        "{:?}",
        applied.failed
    );
}

#[test]
fn a_service_directory_that_cannot_be_copied_is_not_left_half_made() {
    let new = config(&[("dbus", dbus())]);
    let Some(dir) = runit_root(&new) else {
        println!("skipping: the host has no /bin/sh for the fake sv");
        return;
    };
    let root = dir.path();
    let plan = Plan {
        add: vec!["missing".into()],
        ..Plan::default()
    };
    let applied = super::apply(&plan, root, &root.join("gen/services"));
    assert_eq!(applied.failed.len(), 1);
    assert!(!root.join("run/runit/service/missing").exists());
}

#[test]
fn copied_service_directories_keep_their_own_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = laft_test::tempdir();
    let from = dir.path().join("from");
    std::fs::create_dir_all(&from).expect("writable");
    std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o750)).expect("chmod");
    super::copy_tree(&from, &dir.path().join("to")).expect("copies");
    let mode = std::fs::metadata(dir.path().join("to"))
        .expect("exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o750);
}
