//! The base part that every system includes.

mod support;

use std::collections::BTreeSet;

use laft_config::RundirOwner;
use laft_core::{system_gid, system_uid};
use laft_svc::{ACTIVATE, BOOT_COMPLETE, EARLY, ServiceKind, ServiceSet, TURNSTILED};
use support::{codes, config_dir, load, load_body};

#[test]
fn the_base_part_declares_an_early_graph_that_starts_with_activation() {
    let loaded = load_body("").expect("an empty system is valid");
    assert!(loaded.warnings.is_empty(), "{}", loaded.warnings);
    let services = &loaded.config.services.services;
    for name in [ACTIVATE, EARLY, BOOT_COMPLETE, TURNSTILED, "udevd"] {
        assert!(services[name].enable, "{name}");
    }
    let oneshots: BTreeSet<&str> = services
        .iter()
        .filter(|(n, s)| s.early && s.kind == ServiceKind::Oneshot && *n != EARLY)
        .map(|(n, _)| n.as_str())
        .collect();
    let after: BTreeSet<&str> = services[EARLY].after.iter().map(String::as_str).collect();
    assert_eq!(after, oneshots, "early waits for every other early oneshot");
    for (name, service) in services.iter().filter(|(n, s)| s.early && *n != ACTIVATE) {
        let mut pending: Vec<&str> = service.after.iter().map(String::as_str).collect();
        let mut seen = BTreeSet::new();
        while let Some(dep) = pending.pop() {
            if seen.insert(dep) {
                pending.extend(services[dep].after.iter().map(String::as_str));
            }
        }
        assert!(
            seen.contains(ACTIVATE),
            "{name} does not wait for {ACTIVATE}"
        );
    }
}

#[test]
fn gettys_wait_for_the_session_tracker_only_when_it_is_enabled() {
    let getty_after = |body: &str| {
        let loaded = load_body(body).expect("valid");
        let set = ServiceSet::new(&loaded.config.services).expect("the services are valid");
        set.specs()["getty-ttys0"].after.clone()
    };
    let getty = r#"getty { ttys = ["ttyS0"] }"#;
    assert!(getty_after(getty).contains(&TURNSTILED.to_owned()));
    let without = format!("{getty}\nservice turnstiled {{ enable = false }}");
    assert!(!getty_after(&without).contains(&TURNSTILED.to_owned()));
}

#[test]
fn every_static_account_is_declared_with_its_fixed_id() {
    let users = load_body("").expect("valid").config.etc.users;
    for (name, user) in &users.accounts {
        assert_eq!(user.uid, system_uid(name), "user {name}");
    }
    for (name, group) in &users.groups {
        assert_eq!(group.gid, system_gid(name), "group {name}");
    }
    assert_eq!((users.accounts.len(), users.groups.len()), (13, 33));
    assert_eq!(users.accounts["nobody"].group.as_deref(), Some("nogroup"));
    assert_eq!(users.accounts["root"].home.as_deref(), Some("/root"));
}

#[test]
fn the_base_part_mounts_tmp_hardens_the_kernel_and_lists_privileged_files() {
    let config = load_body("").expect("valid").config;
    let tmp = &config.boot.fs["/tmp"];
    assert_eq!(tmp.fstype, "tmpfs");
    assert_eq!(tmp.options, ["mode=1777", "nosuid", "nodev", "size=50%"]);
    assert_eq!(config.etc.sysctl.len(), 22);
    assert_eq!(
        config.etc.sysctl["net.ipv4.ping_group_range"],
        "0 2147483647"
    );
    let privileged: Vec<&str> = config
        .security
        .privileged
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        privileged,
        [
            "/usr/bin/doas",
            "/usr/bin/fusermount3",
            "/usr/bin/laft-enter",
            "/usr/bin/newgidmap",
            "/usr/bin/newuidmap",
            "/usr/bin/passwd",
            "/usr/bin/unix_chkpwd",
        ]
    );
    let chkpwd = &config.security.privileged["/usr/bin/unix_chkpwd"];
    assert_eq!((chkpwd.group.as_str(), chkpwd.mode), ("shadow", 0o2755));
    assert!(config.repos[&"core".parse().expect("a name")].enable);
}

#[test]
fn plain_assignments_override_the_base_part() {
    let body = r#"
      sysctl { "kernel.sysrq" = 0 }
      security { privileged "/usr/bin/doas" { enable = false } }
      repo core { url = "https://mirror.example/core" }
      service boot-complete { after = [udevd] }
    "#;
    let config = load_body(body).expect("valid").config;
    assert_eq!(config.etc.sysctl["kernel.sysrq"], 0);
    assert!(!config.security.privileged["/usr/bin/doas"].enable);
    assert_eq!(
        config.repos[&"core".parse().expect("a name")].url,
        "https://mirror.example/core"
    );
    assert_eq!(config.services.services["boot-complete"].after, ["udevd"]);
}

#[test]
fn turnstile_manages_the_runtime_directory_unless_elogind_does() {
    let rundir = |config: &laft_config::SystemConfig| {
        let text = config.etc.files["/etc/turnstile/turnstiled.conf"]
            .text
            .clone();
        text.expect("the file has text")
            .lines()
            .find(|l| l.starts_with("manage_rundir"))
            .map(str::to_owned)
    };
    let plain = load_body("").expect("valid").config;
    assert_eq!(plain.session.rundir, RundirOwner::Turnstile);
    assert_eq!(rundir(&plain).as_deref(), Some("manage_rundir = yes"));

    let dir = config_dir(&[(
        "system.stav",
        &format!(
            "import \"std/services\" as services\nsystem {{\n{}\ninclude services.elogind()\n}}\n",
            support::DISKS
        ),
    )]);
    let elogind = load(dir.path()).expect("valid").config;
    assert_eq!(elogind.session.rundir, RundirOwner::Elogind);
    assert_eq!(rundir(&elogind).as_deref(), Some("manage_rundir = no"));
    assert!(elogind.services.services["dbus"].enable);
}

#[test]
fn a_user_without_a_group_gets_a_primary_group_that_references_may_name() {
    let body = r#"
      users { user qaid { } }
      file "/etc/qaid.conf" { text = "", group = qaid }
    "#;
    let config = load_body(body)
        .expect("the implicit group is a reference target")
        .config;
    let group = &config.etc.users.groups["qaid"];
    assert_eq!((group.gid, group.system), (None, false));
    assert_eq!(config.etc.files["/etc/qaid.conf"].group, "qaid");

    let declared = r#"users {
      user qaid { }
      group qaid { gid = 2000 }
    }"#;
    let config = load_body(declared).expect("valid").config;
    assert_eq!(config.etc.users.groups["qaid"].gid, Some(2000));
}

#[test]
fn a_group_reference_without_any_such_group_is_rejected() {
    let report =
        load_body(r#"file "/etc/x" { text = "", group = qaid }"#).expect_err("no group qaid");
    assert_eq!(codes(&report), ["E0802"]);
}
