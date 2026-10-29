//! Every validation rule rejects a configuration that breaks it, and
//! points at the place that breaks it.

mod support;

use support::{codes, config_dir, load, load_body, pointed_at};

const KEY: &str = "RWSO9Zvh1ju0E6MO5PDnzQFYnptNxsLpLdwYSfm5Cs4Jc+14umRuka1M";

/// Loads `body` in a system with a root filesystem and an ESP, and returns
/// the codes of the errors and the text the first one points at.
fn rejected(body: &str) -> (Vec<&'static str>, String) {
    let report = load_body(body).expect_err("the configuration is invalid");
    (codes(&report), pointed_at(&report, 0).to_owned())
}

/// Loads `body` as the whole system block, without the default disks.
fn rejected_disks(body: &str) -> (Vec<&'static str>, String) {
    let dir = config_dir(&[("system.stav", &format!("system {{\n{body}\n}}\n"))]);
    let report = load(dir.path()).expect_err("the configuration is invalid");
    (codes(&report), pointed_at(&report, 0).to_owned())
}

#[track_caller]
fn assert_rejected(found: (Vec<&'static str>, String), code: &str, at: &str) {
    assert_eq!(found.0, [code], "codes");
    assert_eq!(found.1, at, "the place of {code}");
}

#[test]
fn invalid_account_names_are_rejected() {
    assert_rejected(
        rejected(r#"users { user "Bob" { } }"#),
        "C0405",
        r#"user "Bob" { }"#,
    );
}

#[test]
fn a_password_hash_and_a_password_file_together_are_rejected() {
    let body =
        r#"users { user alice { password_hash = "!", password_file = "/etc/laft/secrets/a" } }"#;
    assert_rejected(rejected(body), "C0406", r#""/etc/laft/secrets/a""#);
}

#[test]
fn ids_reserved_for_static_accounts_are_rejected() {
    assert_rejected(rejected("users { user alice { uid = 50 } }"), "C0407", "50");
}

#[test]
fn changing_the_id_of_a_static_account_is_rejected() {
    assert_rejected(rejected("users { user sshd { uid = 77 } }"), "C0408", "77");
}

#[test]
fn two_accounts_with_one_id_are_rejected() {
    let body = "users {\n user alice { uid = 1000 }\n user bob { uid = 1000 }\n}";
    assert_rejected(rejected(body), "C0414", "1000");
}

#[test]
fn a_regular_user_cannot_take_a_system_group_as_primary_group() {
    assert_rejected(
        rejected("users { user audio { } }"),
        "C0415",
        "user audio { }",
    );
}

#[test]
fn file_paths_outside_etc_are_rejected() {
    let body = r#"file "/etc/../x" { text = "" }"#;
    assert_rejected(rejected(body), "C0420", body);
}

#[test]
fn reserved_file_paths_are_rejected() {
    let body = r#"file "/etc/passwd" { text = "" }"#;
    assert_rejected(rejected(body), "C0421", body);
}

#[test]
fn a_file_needs_exactly_one_content() {
    let body = r#"file "/etc/x" { text = "a", link = "b" }"#;
    assert_rejected(rejected(body), "C0422", body);
}

#[test]
fn a_file_cannot_be_the_directory_of_another() {
    let body = "file \"/etc/foo\" { text = \"\" }\nfile \"/etc/foo/bar\" { text = \"\" }";
    assert_rejected(
        rejected(body),
        "C0425",
        r#"file "/etc/foo/bar" { text = "" }"#,
    );
}

#[test]
fn toml_content_must_be_a_table() {
    let body = r#"file "/etc/x.toml" { toml = ["a"] }"#;
    assert_rejected(rejected(body), "C0426", body);
}

#[test]
fn env_keys_must_be_variable_names() {
    let body = r#"file "/etc/x.env" { env = { "1A" = "x" } }"#;
    assert_rejected(rejected(body), "C0427", r#""1A" = "x""#);
}

#[test]
fn malformed_sysctl_keys_are_rejected() {
    assert_rejected(rejected(r#"sysctl { "Vm.X" = 1 }"#), "C0431", "1");
}

#[test]
fn a_module_cannot_be_loaded_and_blacklisted() {
    let body = r#"kernel { modules = ["pcspkr"], blacklist = ["pcspkr"] }"#;
    assert_rejected(rejected(body), "C0433", r#"["pcspkr"]"#);
}

#[test]
fn sysctl_values_must_be_scalars() {
    assert_rejected(rejected(r#"sysctl { "vm.x" = [1] }"#), "C0434", "[1]");
}

#[test]
fn only_runit_is_available() {
    let report = load_body("init = s6").expect_err("s6 is not available");
    assert_eq!(codes(&report), ["C0450"]);
    assert_eq!(pointed_at(&report, 0), "s6");
    let help = report
        .iter()
        .next()
        .and_then(|d| d.diagnostic.help.as_deref());
    assert!(help.is_some_and(|h| h.contains("init = runit")), "{help:?}");
}

#[test]
fn values_that_cannot_be_written_are_rejected_where_they_are_set() {
    let cases = [
        (
            r#"users { user alice { description = "a:b" } }"#,
            r#""a:b""#,
        ),
        (r#"users { user alice { home = "/a:b" } }"#, r#""/a:b""#),
        (
            r#"users { user alice { ssh_keys = ["a\nb"] } }"#,
            r#"["a\nb"]"#,
        ),
        (
            r#"kernel { options = { snd = "a\nb" } }"#,
            r#"snd = "a\nb""#,
        ),
        (r#"kernel { options = { "a b" = "x" } }"#, r#""a b" = "x""#),
        (
            r#"file "/etc/x" { link = "/y", copy = true }"#,
            r#"file "/etc/x" { link = "/y", copy = true }"#,
        ),
    ];
    for (body, at) in cases {
        assert_rejected(rejected(body), "C0490", at);
    }
}

#[test]
fn running_out_of_ids_points_at_the_account() {
    let users: String = (0..1000)
        .map(|i| format!("user s{i} {{ system = true, group = users }}\n"))
        .collect();
    let report = load_body(&format!("users {{\n{users}}}")).expect_err("no ids are left");
    assert!(codes(&report).iter().all(|&c| c == "C0490"));
    for i in 0..report.iter().count() {
        let at = pointed_at(&report, i);
        assert!(at.starts_with("user s"), "{at}");
    }
}

#[test]
fn plain_http_needs_allow_http() {
    let body = format!(r#"repo mine {{ url = "http://x.example", key = "{KEY}" }}"#);
    assert_rejected(rejected(&body), "C0801", r#""http://x.example""#);
    let allowed =
        format!(r#"repo mine {{ url = "http://x.example", key = "{KEY}", allow_http = true }}"#);
    assert!(load_body(&allowed).is_ok());
}

#[test]
fn repository_keys_must_be_minisign_keys() {
    let body = r#"repo mine { url = "https://x.example", key = "nope" }"#;
    assert_rejected(rejected(body), "C0802", r#""nope""#);
}

#[test]
fn repository_names_must_be_package_names() {
    let body = format!(r#"repo "My_Repo" {{ url = "https://x.example", key = "{KEY}" }}"#);
    assert_eq!(rejected(&body).0, ["C0803"]);
}

#[test]
fn unresolved_packages_are_rejected_where_they_are_written() {
    assert_rejected(
        rejected("packages = [htop, missing-tool]"),
        "C0901",
        "missing-tool",
    );
}

#[test]
fn alternatives_choose_bare_package_names() {
    assert_rejected(
        rejected("alternatives.sh = dash:static"),
        "C0902",
        "dash:static",
    );
}

#[test]
fn a_view_cannot_be_named_system() {
    let body = r#"view "system" { }"#;
    assert_rejected(rejected(body), "C0903", body);
}

#[test]
fn view_members_must_be_declared() {
    let body = r#"view dev { users = ["bob"] }"#;
    assert_rejected(rejected(body), "C0904", r#"["bob"]"#);
}

#[test]
fn a_system_needs_a_root_filesystem() {
    let dir = config_dir(&[(
        "system.stav",
        "system {\n  fs \"/boot\" { device = \"UUID=1A2B\", fstype = vfat }\n}\n",
    )]);
    let report = load(dir.path()).expect_err("no root filesystem");
    assert_eq!(codes(&report), ["C1101"]);
    let file = report.iter().next().and_then(|d| d.file).expect("a place");
    let entry = dir.path().join("system.stav");
    assert_eq!(report.file_name(file), entry.to_str());
}

#[test]
fn mountpoints_must_be_normalized() {
    let body = r#"fs "/mnt/../x" { device = "UUID=1", fstype = ext4 }"#;
    assert_rejected(rejected(body), "C1102", body);
}

#[test]
fn directories_laft_manages_cannot_be_mountpoints() {
    let body = r#"fs "/usr" { device = "UUID=1", fstype = ext4 }"#;
    assert_rejected(rejected(body), "C1103", body);
}

#[test]
fn etc_cannot_hold_a_mountpoint() {
    for mountpoint in ["/etc", "/etc/ssl"] {
        let body = format!(r#"fs "{mountpoint}" {{ device = "UUID=1", fstype = ext4 }}"#);
        assert_rejected(rejected(&body), "C1103", &body);
    }
}

#[test]
fn the_root_filesystem_type_must_be_mountable_by_the_initramfs() {
    let body = r#"fs "/" { device = "UUID=1", fstype = other(name = "zfs") }
      fs "/boot" { device = "UUID=2", fstype = vfat }"#;
    assert_rejected(rejected_disks(body), "C1104", r#"other(name = "zfs")"#);
    let vfat = r#"fs "/" { device = "UUID=1", fstype = vfat }
      fs "/boot" { device = "UUID=2", fstype = vfat }"#;
    assert_eq!(rejected_disks(vfat).0, ["C1104"]);
}

#[test]
fn ext2_and_ext3_roots_are_accepted() {
    for fstype in ["ext2", "ext3"] {
        let body = format!(
            r#"system {{
  fs "/" {{ device = "UUID=1", fstype = {fstype} }}
  fs "/boot" {{ device = "UUID=2", fstype = vfat }}
}}
"#
        );
        let dir = config_dir(&[("system.stav", &body)]);
        assert!(load(dir.path()).is_ok(), "{fstype}");
    }
}

#[test]
fn the_root_device_must_be_resolvable_by_the_initramfs() {
    let body = r#"fs "/" { device = "/dev/disk/by-id/x", fstype = ext4 }
      fs "/boot" { device = "UUID=2", fstype = vfat }"#;
    assert_rejected(rejected_disks(body), "C1105", r#""/dev/disk/by-id/x""#);
}

#[test]
fn device_names_the_initramfs_refuses_are_rejected() {
    for device in ["/dev/..", "/dev/mapper/.", "UUID=a b", "UUID=", "/dev/"] {
        let body = format!(
            r#"fs "/" {{ device = "{device}", fstype = ext4 }}
      fs "/boot" {{ device = "UUID=2", fstype = vfat }}"#
        );
        let (codes, at) = rejected_disks(&body);
        assert!(codes.contains(&"C1105"), "{device}: {codes:?}");
        assert_eq!(at, format!("{device:?}"), "{device}");
    }
}

#[test]
fn the_laft_device_must_be_resolvable_by_the_initramfs() {
    let body = r#"fs "/laft" { device = "LABEL=laft", fstype = ext4 }"#;
    assert_rejected(rejected(body), "C1105", r#""LABEL=laft""#);
}

#[test]
fn the_esp_device_must_be_resolvable_by_the_initramfs() {
    let body = r#"fs "/" { device = "UUID=1", fstype = ext4 }
      fs "/efi" { device = "LABEL=ESP", fstype = vfat }
      boot { esp = "/efi" }"#;
    assert_rejected(rejected_disks(body), "C1105", r#""LABEL=ESP""#);
}

#[test]
fn luks_devices_must_be_resolvable_by_the_initramfs() {
    for device in ["/dev/disk/by-id/x", "/dev/mapper/other"] {
        let body = format!(r#"luks cryptroot {{ device = "{device}" }}"#);
        assert_rejected(rejected(&body), "C1105", &format!("{device:?}"));
    }
}

#[test]
fn luks_names_must_be_device_mapper_names() {
    for name in [".", "..", "a/b"] {
        let body = format!(r#"luks "{name}" {{ device = "UUID=3" }}"#);
        assert_rejected(rejected(&body), "C1105", &body);
    }
}

#[test]
fn the_root_filesystem_is_mounted_read_write() {
    let body = r#"fs "/" { device = "UUID=1", fstype = ext4, options = ["ro"] }
      fs "/boot" { device = "UUID=2", fstype = vfat }"#;
    assert_rejected(rejected_disks(body), "C1106", r#"["ro"]"#);
}

#[test]
fn the_esp_needs_a_vfat_filesystem() {
    assert_rejected(rejected(r#"boot { esp = "/efi" }"#), "C1107", r#""/efi""#);
}

#[test]
fn mapped_devices_need_a_luks_block() {
    let body = r#"fs "/" { device = "/dev/mapper/cryptroot", fstype = ext4 }
      fs "/boot" { device = "UUID=2", fstype = vfat }"#;
    assert_rejected(rejected_disks(body), "C1108", r#""/dev/mapper/cryptroot""#);
    let unlocked = format!(
        r#"{body}
      luks cryptroot {{ device = "UUID=3" }}"#
    );
    let dir = config_dir(&[("system.stav", &format!("system {{\n{unlocked}\n}}\n"))]);
    assert!(load(dir.path()).is_ok());
}

#[test]
fn filesystems_other_than_tmpfs_need_a_device() {
    let body = r#"fs "/home" { fstype = ext4 }"#;
    assert_rejected(rejected(body), "C1109", body);
}

#[test]
fn invalid_service_names_are_rejected() {
    let body = r#"service "Foo" { exec = ["/usr/bin/foo"] }"#;
    assert_rejected(rejected(body), "C1201", body);
}

#[test]
fn a_getty_terminal_is_listed_once() {
    assert_rejected(
        rejected(r#"getty { ttys = ["tty1", "tty1"] }"#),
        "C1202",
        r#"["tty1", "tty1"]"#,
    );
}

#[test]
fn a_longrun_needs_a_command() {
    assert_rejected(rejected("service foo { }"), "C1203", "service foo { }");
}

#[test]
fn commands_must_be_absolute() {
    assert_rejected(
        rejected(r#"service foo { exec = ["foo"] }"#),
        "C1204",
        r#"["foo"]"#,
    );
}

#[test]
fn dependencies_must_be_enabled() {
    let body = r#"service foo { exec = ["/usr/bin/foo"], after = [bar] }
      service bar { exec = ["/usr/bin/bar"], enable = false }"#;
    assert_rejected(rejected(body), "C1205", "[bar]");
}

#[test]
fn dependency_cycles_are_rejected() {
    let body = r#"service a { exec = ["/usr/bin/a"], after = [b] }
      service b { exec = ["/usr/bin/b"], after = [a] }"#;
    let report = load_body(body).expect_err("a cycle is invalid");
    assert_eq!(codes(&report), ["C1206"]);
    let d = &report.iter().next().expect("one error").diagnostic;
    assert_eq!(d.message, "dependency cycle between services: a -> b -> a");
    assert_eq!(d.labels.len(), 1);
}

#[test]
fn environment_variables_must_be_passable() {
    let body = r#"service foo { exec = ["/usr/bin/foo"], env = { "A=B" = "x" } }"#;
    assert_rejected(rejected(body), "C1207", r#""A=B" = "x""#);
}

#[test]
fn shared_directories_need_the_same_owner_and_mode() {
    let body = r#"service a { exec = ["/usr/bin/a"], state_dirs = [Dir { path = "shared", mode = 0o700 }] }
      service b { exec = ["/usr/bin/b"], state_dirs = [Dir { path = "shared", mode = 0o750 }] }"#;
    assert_eq!(rejected(body).0, ["C1208"]);
}

#[test]
fn service_packages_are_bare_names() {
    let body = r#"service foo { exec = ["/usr/bin/foo"], packages = [foo:static] }"#;
    assert_rejected(rejected(body), "C1209", "foo:static");
}

#[test]
fn early_outside_the_base_part_is_a_warning() {
    let loaded = load_body(r#"service foo { exec = ["/usr/bin/foo"], early = true }"#)
        .expect("early is allowed");
    assert_eq!(codes(&loaded.warnings), ["C1210"]);
    assert!(!loaded.warnings.has_errors());
    assert_eq!(pointed_at(&loaded.warnings, 0), "true");
}

#[test]
fn privileged_files_live_in_usr() {
    let body = r#"security { privileged "/opt/x" { } }"#;
    assert_rejected(rejected(body), "C1301", r#"privileged "/opt/x" { }"#);
}

#[test]
fn privileged_files_must_grant_something() {
    let body = r#"security { privileged "/usr/bin/foo" { mode = 0o755 } }"#;
    let report = load_body(body).expect_err("the entry grants nothing");
    assert_eq!(codes(&report), ["C1302"]);
    assert_eq!(
        pointed_at(&report, 0),
        r#"privileged "/usr/bin/foo" { mode = 0o755 }"#
    );
    let help = report
        .iter()
        .next()
        .and_then(|d| d.diagnostic.help.as_deref());
    assert!(
        help.is_some_and(|h| h.contains("enable = false")),
        "{help:?}"
    );
}

#[test]
fn capability_text_must_parse() {
    let body = r#"security { privileged "/usr/bin/foo" { caps = "cap_setuid+x" } }"#;
    assert_rejected(rejected(body), "C1303", r#""cap_setuid+x""#);
}
