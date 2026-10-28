use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;

use serde_json::json;

use super::*;
use crate::accounts::resolve;
use crate::test_support::{Env, base_users, dump, fake_package, mode, users_with};
use crate::{Allocations, Kernel, User};

fn declared(config: &EtcConfig) -> Declared {
    resolve(
        &config.users,
        &mut Allocations::default(),
        &Default::default(),
    )
    .expect("valid accounts")
}

fn base() -> EtcConfig {
    EtcConfig {
        users: base_users(),
        ..EtcConfig::default()
    }
}

fn render_config(env: &Env, config: &EtcConfig, packages: &[String]) -> Result<EtcTree> {
    render(
        config,
        &declared(config),
        &env.root.store(),
        packages,
        &std::collections::BTreeMap::new(),
    )
}

fn codes(err: Error) -> Vec<Option<&'static str>> {
    match err {
        Error::Config(errors) => errors.iter().map(ConfigError::code).collect(),
        other => panic!("expected configuration errors, got {other}"),
    }
}

#[test]
fn an_empty_configuration_renders_the_derived_files() {
    let env = Env::new();
    let tree = render_config(&env, &base(), &[]).expect("renders");
    insta::assert_snapshot!(dump(&tree));
    assert!(tree.warnings().is_empty());
}

#[test]
fn identity_kernel_and_keys_render_their_files() {
    let env = Env::new();
    let config = EtcConfig {
        hostname: "forge".into(),
        timezone: "Europe/Berlin".into(),
        locale: "de_DE.UTF-8".into(),
        keymap: Some("de-latin1".into()),
        console_font: Some("ter-v22n".into()),
        users: users_with(&[(
            "alice",
            User {
                ssh_keys: vec![
                    "ssh-ed25519 AAAA alice@laptop".into(),
                    "ssh-rsa BBBB".into(),
                ],
                ..User::default()
            },
        )]),
        kernel: Kernel {
            modules: vec!["vfio-pci".into(), "kvm-amd".into()],
            blacklist: ["pcspkr".to_owned(), "nouveau".to_owned()].into(),
            options: BTreeMap::from([("snd_hda_intel".into(), "power_save=1".into())]),
        },
        sysctl: BTreeMap::from([
            ("vm.swappiness".into(), json!(10)),
            ("net.ipv4.ip_forward".into(), json!(true)),
            ("kernel.printk".into(), json!("3 4 1 3")),
            ("net/ipv6/conf/all/forwarding".into(), json!(false)),
        ]),
        ..EtcConfig::default()
    };
    let tree = render_config(&env, &config, &[]).expect("renders");
    insta::assert_snapshot!(dump(&tree));
}

#[test]
fn declared_files_render_with_their_format_mode_and_owner() {
    let env = Env::new();
    let mut config = base();
    let file = |f: File| f;
    config.files = BTreeMap::from([
        (
            "/etc/doas.conf".into(),
            file(File {
                text: Some("permit persist :wheel\n".into()),
                mode: 0o400,
                ..File::default()
            }),
        ),
        (
            "/etc/iwd/main.conf".into(),
            file(File {
                ini: Some(
                    serde_json::from_value(json!({
                        "General": { "EnableNetworkConfiguration": true },
                        "Network": { "NameResolvingService": "resolvconf" },
                    }))
                    .expect("sections"),
                ),
                ..File::default()
            }),
        ),
        (
            "/etc/containers/registries.conf".into(),
            file(File {
                toml: Some(json!({ "unqualified-search-registries": ["docker.io"] })),
                ..File::default()
            }),
        ),
        (
            "/etc/app/config.json".into(),
            file(File {
                json: Some(json!({ "debug": false })),
                owner: "sshd".into(),
                group: "wheel".into(),
                ..File::default()
            }),
        ),
        (
            "/etc/app/config.yaml".into(),
            file(File {
                yaml: Some(json!({ "listen": ["127.0.0.1", "::1"] })),
                ..File::default()
            }),
        ),
        (
            "/etc/default/app".into(),
            file(File {
                env: Some(BTreeMap::from([
                    ("A".into(), "x y".into()),
                    ("B".into(), "plain".into()),
                ])),
                ..File::default()
            }),
        ),
        (
            "/etc/resolv.conf".into(),
            file(File {
                link: Some("/run/resolvconf/resolv.conf".into()),
                ..File::default()
            }),
        ),
    ]);
    let tree = render_config(&env, &config, &[]).expect("renders");
    let declared_only: String = dump(&tree)
        .split("=== ")
        .filter(|entry| {
            ["doas", "iwd", "containers", "app", "default", "resolv"]
                .iter()
                .any(|p| entry.starts_with(p))
        })
        .map(|entry| format!("=== {entry}"))
        .collect();
    insta::assert_snapshot!(declared_only);
    assert_eq!(
        tree.warnings(),
        [Warning::PrivateFile {
            path: "/etc/doas.conf".into(),
            mode: 0o400
        }]
    );
}

#[test]
fn configuration_overrides_package_defaults_and_disabling_removes() {
    let env = Env::new();
    let pkg = fake_package(
        &env.root,
        "netbase",
        &[
            ("etc/hosts", "from the package\n"),
            ("etc/protocols", "ip 0 IP\n"),
            ("etc/default/grub", "GRUB=1\n"),
            ("usr/share/doc/netbase", "docs\n"),
        ],
    );
    let packages = [pkg];
    let tree = render_config(&env, &base(), &packages).expect("renders");
    let hosts = tree.entries().get("hosts");
    assert!(
        matches!(hosts, Some(Node::File { bytes, .. }) if bytes.starts_with(b"127.0.0.1 localhost")),
        "the derived hosts replaces the package's: {hosts:?}"
    );
    assert!(matches!(
        tree.entries().get("protocols"),
        Some(Node::Linked(_))
    ));
    assert!(!tree.entries().contains_key("usr/share/doc/netbase"));

    let mut config = base();
    config.files.insert(
        "/etc/hosts".into(),
        File {
            text: Some("x".into()),
            ..File::default()
        },
    );
    config.files.insert(
        "/etc/default".into(),
        File {
            enable: false,
            ..File::default()
        },
    );
    config.files.insert(
        "/etc/hostname".into(),
        File {
            enable: false,
            ..File::default()
        },
    );
    let tree = render_config(&env, &config, &packages).expect("renders");
    assert!(matches!(
        tree.entries().get("hosts"),
        Some(Node::File { bytes, .. }) if bytes == b"x"
    ));
    assert!(!tree.entries().contains_key("hostname"));
    assert!(!tree.entries().contains_key("default"));
    assert!(!tree.entries().contains_key("default/grub"));
}

#[test]
fn two_packages_shipping_the_same_path_conflict() {
    let env = Env::new();
    let a = fake_package(&env.root, "vim", &[("etc/vimrc", "a\n")]);
    let b = fake_package(&env.root, "neovim", &[("etc/vimrc", "b\n")]);
    let err = render_config(&env, &base(), &[a, b]).expect_err("conflicts");
    assert_eq!(
        err.to_string(),
        "/etc/vimrc is shipped by both vim 1.0-r1 and neovim 1.0-r1"
    );
}

#[test]
fn invalid_declared_files_and_kernel_settings_are_all_reported() {
    let env = Env::new();
    let mut config = base();
    let text = || File {
        text: Some("x".into()),
        ..File::default()
    };
    config.files = BTreeMap::from([
        ("/etc/passwd".into(), text()),
        ("/etc/static/x".into(), text()),
        ("etc/relative".into(), text()),
        ("/etc/a/../b".into(), text()),
        (
            "/etc/both".into(),
            File {
                source: Some("/nonexistent".into()),
                ..text()
            },
        ),
        ("/etc/foo".into(), text()),
        ("/etc/foo/bar".into(), text()),
        (
            "/etc/list.toml".into(),
            File {
                toml: Some(json!(["a"])),
                ..File::default()
            },
        ),
        (
            "/etc/bad.env".into(),
            File {
                env: Some(BTreeMap::from([("1A".into(), "x".into())])),
                ..File::default()
            },
        ),
    ]);
    config.kernel.modules = vec!["pcspkr".into()];
    config.kernel.blacklist = ["pcspkr".to_owned()].into();
    config.sysctl = BTreeMap::from([("vm..x".into(), json!(1)), ("vm.x".into(), json!([1]))]);
    let codes = codes(render_config(&env, &config, &[]).expect_err("invalid"));
    assert_eq!(
        codes,
        [
            Some("C0433"),
            Some("C0431"),
            Some("C0434"),
            Some("C0420"),
            Some("C0427"),
            Some("C0422"),
            Some("C0425"),
            Some("C0426"),
            Some("C0421"),
            Some("C0421"),
            Some("C0420"),
        ]
    );
}

#[test]
fn record_lists_managed_paths_and_copies_with_the_account_databases() {
    let env = Env::new();
    let mut config = base();
    config.files.insert(
        "/etc/machine.conf".into(),
        File {
            text: Some("x".into()),
            copy: true,
            ..File::default()
        },
    );
    let record = render_config(&env, &config, &[]).expect("renders").record();
    assert_eq!(
        record.copy,
        ["group", "gshadow", "machine.conf", "passwd", "shadow"]
    );
    assert!(record.managed.contains(&"hostname".to_owned()));
    assert!(record.managed.contains(&"sysctl.d/laft.conf".to_owned()));
    assert!(
        !record
            .managed
            .iter()
            .any(|p| p == "sysctl.d" || p == "machine.conf")
    );
}

#[test]
fn writing_the_tree_keeps_modes_and_links_store_files() {
    let env = Env::new();
    let pkg = fake_package(&env.root, "netbase", &[("etc/protocols", "ip 0 IP\n")]);
    let mut config = base();
    config.files.insert(
        "/etc/doas.conf".into(),
        File {
            text: Some("permit :wheel\n".into()),
            mode: 0o400,
            ..File::default()
        },
    );
    let tree = render_config(&env, &config, std::slice::from_ref(&pkg)).expect("renders");
    let generation = env.path("laft/gens/1");
    tree.write(&generation).expect("writes");

    let etc = generation.join("etc");
    assert_eq!(mode(&etc.join("doas.conf")), 0o400);
    assert_eq!(mode(&etc.join("hostname")), 0o644);
    assert_eq!(mode(&etc.join("sysctl.d")), 0o755);
    let linked = fs::metadata(etc.join("protocols")).expect("exists");
    let store = fs::metadata(env.root.store().path(&pkg).join("etc/protocols")).expect("exists");
    assert_eq!(linked.ino(), store.ino());
    assert_eq!(
        fs::read_link(etc.join("localtime")).expect("a link"),
        Path::new("/usr/share/zoneinfo/UTC")
    );
    assert!(tree.write(&generation).is_err(), "nothing is overwritten");
}

#[test]
fn identity_values_that_would_corrupt_their_files_are_rejected() {
    let env = Env::new();
    type Change = fn(&mut EtcConfig);
    let cases: [(&str, Change); 9] = [
        ("hostname", |c| c.hostname = "forge\n10.0.0.1 bank".into()),
        ("hostname", |c| c.hostname = "-forge".into()),
        ("hostname", |c| c.hostname = String::new()),
        ("timezone", |c| c.timezone = "../../../etc/shadow".into()),
        ("timezone", |c| c.timezone = "/etc/shadow".into()),
        ("locale", |c| c.locale = "C.UTF-8\nLC_ALL=x".into()),
        ("keymap", |c| c.keymap = Some("us\nFONT=x".into())),
        ("keymap", |c| c.keymap = Some("..".into())),
        ("console_font", |c| c.console_font = Some("a b".into())),
    ];
    for (field, change) in cases {
        let mut config = base();
        change(&mut config);
        let err = render_config(&env, &config, &[]).expect_err(field);
        match err {
            Error::Config(errors) => assert!(
                errors.iter().any(
                    |e| matches!(e, ConfigError::InvalidValue { context, .. } if context == field)
                ),
                "{field}: {errors:?}"
            ),
            other => panic!("{field}: {other}"),
        }
    }
    let mut config = base();
    config.hostname = "build-01.example.org".into();
    config.timezone = "America/Argentina/Buenos_Aires".into();
    config.locale = "sr_RS.UTF-8@latin".into();
    config.keymap = Some("de-latin1-nodeadkeys".into());
    config.console_font = Some("Lat2-Terminus16".into());
    render_config(&env, &config, &[]).expect("renders");
}

#[test]
fn generated_entries_must_have_plain_paths_below_etc() {
    let env = Env::new();
    let config = base();
    for path in ["../escape", "/abs", "sv//x", "static/x"] {
        let generated = BTreeMap::from([(path.to_owned(), text("x\n".into()))]);
        let err = render(
            &config,
            &declared(&config),
            &env.root.store(),
            &[],
            &generated,
        )
        .expect_err(path);
        assert!(matches!(err, Error::Config(_)), "{path}: {err}");
    }
}
