use crate::test_support::{config, dbus, errors, example, myapp, valid};
use crate::{Dir, Getty, Init, OnChange, Ready, Service, ServiceKind, SpecLog, SpecReady, Warning};

#[test]
fn normal_services_depend_on_early_implicitly() {
    let set = valid(&example());
    assert_eq!(set.specs()["myapp"].after, ["dbus", "early"]);
    assert_eq!(set.specs()["dbus"].after, ["early"]);
    assert_eq!(set.specs()["kmod"].after, ["kernel-fs"]);
}

#[test]
fn after_is_sorted_and_deduplicated() {
    let app = Service {
        after: vec!["early".into(), "dbus".into(), "dbus".into()],
        ..Service::longrun(&["/usr/bin/app"])
    };
    let set = valid(&config(&[("dbus", dbus()), ("app", app)]));
    assert_eq!(set.specs()["app"].after, ["dbus", "early"]);
}

#[test]
fn defaults_are_resolved_into_the_spec() {
    let job = Service::oneshot(&["/usr/bin/job"]);
    let set = valid(&config(&[
        ("job", job),
        ("dbus", dbus()),
        ("myapp", myapp()),
    ]));
    assert_eq!(set.specs()["job"].on_change, OnChange::Rerun);
    assert_eq!(set.specs()["myapp"].on_change, OnChange::Restart);
    assert_eq!(set.specs()["myapp"].state_dirs[0].path, "/var/lib/myapp");
    assert_eq!(set.specs()["myapp"].restart_delay_ms, 1000);
    assert_eq!(set.specs()["myapp"].stop_signal, 15);
}

#[test]
fn readiness_and_runtime_dirs_are_resolved() {
    let set = valid(&example());
    let dbus = &set.specs()["dbus"];
    assert_eq!(dbus.ready, SpecReady::Notify { fd: 3 });
    assert_eq!(dbus.runtime_dirs[0].path, "/run/dbus");
    assert_eq!(dbus.runtime_dirs[0].owner.as_deref(), Some("messagebus"));
    let udevd = &set.specs()["udevd"];
    assert_eq!(
        udevd.ready,
        SpecReady::Check {
            cmd: vec!["/usr/bin/udevadm".into(), "control".into(), "--ping".into()],
            interval_ms: 1000,
            timeout_ms: 60_000,
        }
    );
}

#[test]
fn early_services_log_to_the_console() {
    let set = valid(&example());
    assert_eq!(set.specs()["udevd"].log, SpecLog::Console);
    assert_eq!(set.specs()["kmod"].log, SpecLog::Console);
    assert!(matches!(set.specs()["dbus"].log, SpecLog::Logged { .. }));
}

#[test]
fn disabled_services_are_left_out() {
    let mut off = myapp();
    off.enable = false;
    let set = valid(&config(&[("dbus", dbus()), ("myapp", off)]));
    assert!(!set.specs().contains_key("myapp"));
}

#[test]
fn packages_of_enabled_services_are_collected() {
    let mut with = dbus();
    with.packages.insert("dbus".parse().expect("valid name"));
    let mut without = myapp();
    without.enable = false;
    without
        .packages
        .insert("myapp".parse().expect("valid name"));
    let set = valid(&config(&[("dbus", with), ("myapp", without)]));
    let names: Vec<&str> = set.packages().iter().map(|n| n.as_str()).collect();
    assert_eq!(names, ["dbus"]);
}

#[test]
fn kinds_require_their_commands() {
    let sysctl = Service {
        kind: ServiceKind::Longrun,
        ..Service::default()
    };
    let job = Service {
        exec: vec!["/usr/bin/job".into()],
        ..Service::oneshot(&[])
    };
    let daemon = Service {
        up: vec!["/usr/bin/x".into()],
        ..Service::longrun(&["/usr/bin/daemon"])
    };
    assert_eq!(
        errors(&config(&[
            ("sysctl2", sysctl),
            ("job", job),
            ("daemon", daemon)
        ])),
        [
            r#"service daemon: a longrun cannot have "up""#,
            r#"service job: a oneshot requires "up""#,
            r#"service job: a oneshot cannot have "exec""#,
            r#"service sysctl2: a longrun requires "exec""#,
        ]
    );
}

#[test]
fn commands_need_absolute_programs() {
    let app = Service {
        ready: Ready::Check {
            cmd: vec!["probe".into()],
            interval: 1,
            timeout: 60,
        },
        cwd: "var".into(),
        ..Service::longrun(&["myapp"])
    };
    let job = Service {
        down: vec!["stop".into()],
        ..Service::oneshot(&["/usr/bin/start"])
    };
    assert_eq!(
        errors(&config(&[("myapp", app), ("job", job)])),
        [
            r#"service job: down[0] "stop" is not an absolute path"#,
            r#"service myapp: exec[0] "myapp" is not an absolute path"#,
            r#"service myapp: ready.cmd[0] "probe" is not an absolute path"#,
            r#"service myapp: cwd "var" is not an absolute path"#,
        ]
    );
}

#[test]
fn unknown_dependency_suggests_a_close_name() {
    let mut app = myapp();
    app.after = vec!["dbs".into(), "zzzzzz".into()];
    assert_eq!(
        errors(&config(&[("dbus", dbus()), ("myapp", app)])),
        [
            r#"service myapp: no service "dbs" (did you mean "dbus"?)"#,
            r#"service myapp: no service "zzzzzz""#,
        ]
    );
}

#[test]
fn disabled_dependency_is_rejected() {
    let mut off = dbus();
    off.enable = false;
    assert_eq!(
        errors(&config(&[("dbus", off), ("myapp", myapp())])),
        [r#"service myapp: after references "dbus", which is disabled"#]
    );
}

#[test]
fn dependency_cycle_is_printed_in_order() {
    let mut app = myapp();
    app.after = vec!["worker".into()];
    let worker = Service {
        after: vec!["myapp".into()],
        ..Service::longrun(&["/usr/bin/worker"])
    };
    assert_eq!(
        errors(&config(&[("myapp", app), ("worker", worker)])),
        ["dependency cycle between services\n  myapp -> worker -> myapp"]
    );
}

#[test]
fn early_services_cannot_wait_for_normal_ones() {
    let mut services = config(&[("dbus", dbus())]);
    let kmod = services.services.get_mut("kmod").expect("kmod is standard");
    kmod.after.push("dbus".into());
    assert_eq!(
        errors(&services),
        [
            r#"service kmod: an early service cannot depend on "dbus", which is not early"#,
            "dependency cycle between services\n  \
             console -> udev-trigger -> kmod -> dbus -> early -> console",
        ]
    );
}

#[test]
fn early_outside_the_standard_services_is_a_warning() {
    let mut job = Service::oneshot(&["/usr/bin/job"]);
    job.early = true;
    job.after = vec!["kernel-fs".into()];
    let set = valid(&config(&[("job", job)]));
    assert_eq!(set.warnings(), [Warning::EarlyReserved("job".into())]);
    assert_eq!(
        set.warnings()[0].to_string(),
        "service job: early is reserved for the base part"
    );
}

#[test]
fn malformed_and_reserved_names_are_rejected() {
    let svc = || Service::longrun(&["/usr/bin/x"]);
    assert_eq!(
        errors(&config(&[
            ("Foo", svc()),
            ("app-log", svc()),
            ("laft-mine", svc()),
            ("9lives", svc()),
        ])),
        [
            r#"service "9lives": invalid service name, expected [a-z][a-z0-9-]* of at most 48 characters"#,
            r#"service "Foo": invalid service name, expected [a-z][a-z0-9-]* of at most 48 characters"#,
            r#"service "app-log": invalid service name, names ending in -log are reserved for loggers"#,
            r#"service "laft-mine": invalid service name, names starting with laft- are reserved"#,
        ]
    );
}

#[test]
fn ttys_are_checked_and_gettys_keep_their_names() {
    let mut services = config(&[("getty-tty1", Service::longrun(&["/usr/bin/x"]))]);
    services.getty = Getty {
        ttys: vec![
            "tty1".into(),
            "ttyS0".into(),
            "ttyS0".into(),
            "console".into(),
        ],
    };
    assert_eq!(
        errors(&services),
        [
            r#"getty: tty "ttyS0" is listed twice"#,
            r#"getty: tty "console" is not a terminal name like tty1, ttyS0 or ttyUSB0"#,
            r#"service "getty-tty1": invalid service name, the name belongs to a generated getty"#,
        ]
    );
}

#[test]
fn numbers_outside_their_range_are_rejected() {
    let app = Service {
        ready: Ready::Notify { fd: 1 },
        umask: 0o1000,
        stop_timeout: 0,
        state_dirs: vec![Dir {
            mode: 0o10000,
            ..Dir::new("app")
        }],
        ..Service::longrun(&["/usr/bin/app"])
    };
    assert_eq!(
        errors(&config(&[("app", app)])),
        [
            "service app: umask is 512, expected 0 to 511",
            "service app: stop_timeout is 0, expected 1 to 3600",
            "service app: ready.fd is 1, expected 3 to 1023",
            "service app: dir.mode is 4096, expected 0 to 4095",
        ]
    );
}

#[test]
fn directory_paths_stay_below_their_base() {
    let app = Service {
        state_dirs: vec![Dir::new("../etc"), Dir::new("a//b"), Dir::new("/abs")],
        runtime_dirs: vec![Dir::new("ok/sub.dir@x+y")],
        ..Service::longrun(&["/usr/bin/app"])
    };
    let found = errors(&config(&[("app", app)]));
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found[0].starts_with(r#"service app: invalid directory path "../etc""#));
}

#[test]
fn shared_directory_with_different_owners_is_rejected() {
    let shared = |user: &str| Service {
        user: user.into(),
        state_dirs: vec![Dir::new("shared")],
        ..Service::longrun(&["/usr/bin/x"])
    };
    assert_eq!(
        errors(&config(&[
            ("myapp", shared("myapp")),
            ("other", shared("other"))
        ])),
        ["services myapp and other both declare /var/lib/shared with different owners or modes"]
    );
    valid(&config(&[
        ("myapp", shared("app")),
        ("other", shared("app")),
    ]));
}

#[test]
fn shared_directory_with_different_modes_is_rejected() {
    let shared = |mode: u32| Service {
        user: "app".into(),
        state_dirs: vec![Dir {
            mode,
            ..Dir::new("shared")
        }],
        ..Service::longrun(&["/usr/bin/x"])
    };
    assert_eq!(
        errors(&config(&[
            ("myapp", shared(0o750)),
            ("other", shared(0o700))
        ])),
        ["services myapp and other both declare /var/lib/shared with different owners or modes"]
    );
}

#[test]
fn kind_specific_settings_are_rejected_elsewhere() {
    let daemon = Service {
        on_change: Some(OnChange::Rerun),
        ..Service::longrun(&["/usr/bin/daemon"])
    };
    let job = Service {
        ready: Ready::Notify { fd: 3 },
        ..Service::oneshot(&["/usr/bin/job"])
    };
    assert_eq!(
        errors(&config(&[("daemon", daemon), ("job", job)])),
        [
            "service daemon: on_change = rerun applies only to oneshots",
            "service job: a oneshot is ready when up succeeds, so ready must be immediate",
        ]
    );
}

#[test]
fn only_runit_is_supported() {
    let mut services = config(&[]);
    services.init = Init::S6;
    assert_eq!(
        errors(&services),
        ["init = s6 is not implemented in this version of Laft. Only runit is supported"]
    );
}

#[test]
fn configuration_deserializes_with_defaults() {
    let json = r#"{
        "init": "runit",
        "hostname": "ignored",
        "services": {
            "dbus": {
                "exec": ["/usr/bin/dbus-daemon"],
                "ready": {"type": "notify", "fd": 3},
                "runtime_dirs": [{"path": "dbus", "owner": "messagebus"}],
                "restart": "on-failure",
                "stop_signal": "HUP"
            },
            "probe": {
                "exec": ["/usr/bin/probe"],
                "ready": {"type": "check", "cmd": ["/usr/bin/true"]}
            }
        },
        "getty": {"ttys": ["tty1"]}
    }"#;
    let config: crate::ServiceConfig = serde_json::from_str(json).expect("the value deserializes");
    let dbus = &config.services["dbus"];
    assert_eq!(dbus.user, "root");
    assert_eq!(dbus.runtime_dirs[0].mode, 0o755);
    assert_eq!(dbus.stop_signal.number(), 1);
    assert_eq!(dbus.restart, crate::Restart::OnFailure);
    assert_eq!(
        config.services["probe"].ready,
        Ready::Check {
            cmd: vec!["/usr/bin/true".into()],
            interval: 1,
            timeout: 60
        }
    );
    assert_eq!(config.getty.ttys, ["tty1"]);
    let typo = r#"{"services": {"x": {"exce": []}}}"#;
    assert!(serde_json::from_str::<crate::ServiceConfig>(typo).is_err());
}

#[test]
fn environment_entries_that_cannot_be_passed_on_are_rejected() {
    let mut app = myapp();
    app.env.clear();
    for (key, value) in [("", "x"), ("A=B", "x"), ("A\0B", "x"), ("OK", "a\0b")] {
        app.env.insert(key.into(), value.into());
    }
    let errors = errors(&config(&[("dbus", dbus()), ("myapp", app)]));
    assert_eq!(
        errors,
        [
            "service myapp: env \"\" is not a valid variable name",
            "service myapp: env \"A\\0B\" is not a valid variable name",
            "service myapp: env \"A=B\" is not a valid variable name",
            "service myapp: env \"OK\" has a value with a NUL byte",
        ]
    );
}
