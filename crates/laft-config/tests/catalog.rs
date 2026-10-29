//! The std catalog: `std/services` and `std/desktop`.

mod support;

use support::{DISKS, codes, config_dir, load};

/// Loads a system whose body is `body`, with both catalog modules imported.
fn with_catalog(body: &str) -> Result<laft_config::Loaded, laft_config::Report> {
    let text = format!(
        "import \"std/desktop\" as desktop\nimport \"std/services\" as services\n\
         system {{\n{DISKS}\n{body}\n}}\n"
    );
    let dir = config_dir(&[("system.stav", &text)]);
    load(dir.path())
}

#[test]
fn every_service_part_declares_an_enabled_service_with_its_packages() {
    let parts = [
        ("sshd", "openssh"),
        ("dbus", "dbus"),
        ("seatd", "seatd"),
        ("elogind", "elogind"),
        ("dhcpcd", "dhcpcd"),
        ("iwd", "iwd"),
        ("chronyd", "chrony"),
        ("nftables", "nftables"),
    ];
    for (part, package) in parts {
        let loaded = with_catalog(&format!("include services.{part}()"))
            .unwrap_or_else(|e| panic!("{part}:\n{e}"));
        let service = &loaded.config.services.services[part];
        assert!(service.enable, "{part}");
        let packages: Vec<&str> = service.packages.iter().map(|p| p.as_str()).collect();
        assert_eq!(packages, [package], "{part}");
    }
}

#[test]
fn sshd_listens_on_the_given_port_and_refuses_passwords() {
    let config = with_catalog("include services.sshd(port = 2222)")
        .expect("valid")
        .config;
    assert_eq!(
        config.services.services["sshd"]
            .exec
            .last()
            .map(String::as_str),
        Some("2222")
    );
    let sshd_config = config.etc.files["/etc/ssh/sshd_config"]
        .text
        .clone()
        .expect("text");
    assert!(sshd_config.contains("PasswordAuthentication no\n"));
    assert!(sshd_config.contains("/etc/ssh/authorized_keys.d/%u"));
}

#[test]
fn the_wayland_desktop_sets_up_packages_seat_groups_and_environment() {
    let body = "users { user alice { uid = 1000 } }\n\
                include desktop.wayland(user = alice, fonts = [noto-fonts-cjk])";
    let config = with_catalog(body).expect("valid").config;
    let packages: Vec<String> = config.packages.iter().map(|r| r.pkg.to_string()).collect();
    for package in [
        "niri",
        "foot",
        "mesa",
        "pipewire",
        "wireplumber",
        "noto-fonts-cjk",
    ] {
        assert!(
            packages.iter().any(|p| p == package),
            "{package} in {packages:?}"
        );
    }
    assert!(config.services.services["seatd"].enable);
    for group in ["video", "audio", "input"] {
        let members = &config.etc.users.groups[group].members;
        assert!(members.contains("alice"), "{group}");
    }
    let profile = config.etc.files["/etc/profile.d/wayland.sh"].text.clone();
    assert!(profile.is_some_and(|t| t.contains("export MOZ_ENABLE_WAYLAND=1\n")));
}

#[test]
fn the_compositor_selects_its_package() {
    let body = "users { user alice { } }\ninclude desktop.wayland(user = alice, compositor = sway)";
    let config = with_catalog(body).expect("valid").config;
    let packages: Vec<String> = config.packages.iter().map(|r| r.pkg.to_string()).collect();
    assert!(packages.contains(&"sway".to_owned()));
    assert!(!packages.contains(&"niri".to_owned()));
}

#[test]
fn a_plain_file_in_the_configuration_wins_over_the_desktop_default() {
    let body = "users { user alice { } }\ninclude desktop.wayland(user = alice)\n\
                file \"/etc/profile.d/wayland.sh\" { text = \"x\" }";
    let config = with_catalog(body).expect("valid").config;
    assert_eq!(
        config.etc.files["/etc/profile.d/wayland.sh"]
            .text
            .as_deref(),
        Some("x")
    );
}

#[test]
fn the_desktop_user_must_be_declared() {
    let report = with_catalog("include desktop.wayland(user = alice)").expect_err("no alice");
    assert!(codes(&report).iter().all(|&c| c == "E0802"), "{report}");
}

#[test]
fn parts_that_include_the_system_bus_declare_it_once() {
    let config =
        with_catalog("include services.elogind()\ninclude services.iwd()\ninclude services.dbus()")
            .expect("valid")
            .config;
    let dbus = &config.services.services["dbus"];
    assert_eq!(dbus.runtime_dirs.len(), 1, "{:?}", dbus.runtime_dirs);
    let packages: Vec<&str> = dbus.packages.iter().map(|p| p.as_str()).collect();
    assert_eq!(packages, ["dbus"]);
    for service in ["elogind", "iwd"] {
        let after = &config.services.services[service].after;
        assert_eq!(
            after.iter().map(String::as_str).collect::<Vec<_>>(),
            ["dbus"]
        );
    }
}
