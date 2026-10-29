//! The two complete example configurations evaluate, validate and convert.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use laft_config::{PkgError, SystemConfig};
use laft_core::PkgRef;

/// The packages of the test index: everything the examples, the base part
/// and the std catalog name.
const INDEX: &[&str] = &[
    "base-minimal",
    "brightnessctl",
    "chrony",
    "clang",
    "cmake",
    "dash",
    "dbus",
    "dhcpcd",
    "fd",
    "firefox",
    "flatpak",
    "foot",
    "fuzzel",
    "git",
    "htop",
    "iwd",
    "jujutsu",
    "linux-lts",
    "llvm",
    "mako",
    "mesa",
    "neovim",
    "niri",
    "ninja",
    "nodejs",
    "noto-fonts",
    "noto-fonts-cjk",
    "noto-fonts-emoji",
    "openssh",
    "opendoas",
    "openssl1.1",
    "pipewire",
    "prometheus-node-exporter",
    "python",
    "ripgrep",
    "seatd",
    "ttf-jetbrains-mono",
    "vulkan-icd-loader",
    "wireplumber",
    "xdg-desktop-portal",
    "xdg-desktop-portal-wlr",
    "xwayland-satellite",
    "zsh",
];

fn in_index(pkg: &PkgRef) -> Result<(), PkgError> {
    if INDEX.contains(&pkg.name().as_str()) {
        Ok(())
    } else {
        Err(PkgError {
            message: format!("no package `{}` in the index", pkg.name()),
            help: None,
        })
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Loads an example and returns the debug form of what it adds to or
/// changes in an empty system, with the fixture directory written as
/// `/etc/laft`.
fn added_to_base(name: &str) -> String {
    let dir = fixture(name);
    let loaded = laft_config::load(&dir, &in_index).unwrap_or_else(|e| panic!("{name}:\n{e}"));
    assert!(loaded.warnings.is_empty(), "{}", loaded.warnings);
    let base = support::load_body("")
        .expect("an empty system is valid")
        .config;
    let mut config = loaded.config;
    without_base(&mut config, &base);
    format!("{config:#?}").replace(&dir.display().to_string(), "/etc/laft")
}

/// Removes every entry that `base` has with the same value.
fn without_base(config: &mut SystemConfig, base: &SystemConfig) {
    let users = &base.etc.users;
    config
        .etc
        .users
        .accounts
        .retain(|name, user| users.accounts.get(name) != Some(user));
    config
        .etc
        .users
        .groups
        .retain(|name, group| users.groups.get(name) != Some(group));
    config
        .etc
        .sysctl
        .retain(|key, value| base.etc.sysctl.get(key) != Some(value));
    config
        .services
        .services
        .retain(|name, service| base.services.services.get(name) != Some(service));
    config
        .etc
        .files
        .retain(|path, file| base.etc.files.get(path) != Some(file));
    config
        .boot
        .fs
        .retain(|path, fs| base.boot.fs.get(path) != Some(fs));
    config
        .repos
        .retain(|name, repo| base.repos.get(name) != Some(repo));
    let privileged = &base.security.privileged;
    config
        .security
        .privileged
        .retain(|path, entry| privileged.get(path) != Some(entry));
}

#[test]
fn the_minimal_server_example_converts() {
    insta::assert_snapshot!(added_to_base("server"));
}

#[test]
fn the_wayland_laptop_example_converts() {
    insta::assert_snapshot!(added_to_base("laptop"));
}

#[test]
fn package_requests_point_at_their_source() {
    let dir = fixture("server");
    let config = laft_config::load(&dir, &in_index).expect("valid").config;
    let origins: BTreeSet<(String, String)> = config
        .packages
        .iter()
        .map(|r| {
            let origin = r.origin.replace(&dir.display().to_string(), "/etc/laft");
            (r.pkg.to_string(), origin)
        })
        .collect();
    assert!(
        origins.contains(&("ripgrep:static".into(), "/etc/laft/system.stav:9:31".into())),
        "{origins:?}"
    );
}

#[test]
fn a_package_missing_from_the_index_is_reported_where_it_is_written() {
    let dir = fixture("server");
    let report = laft_config::load(&dir, &|pkg| match pkg.name().as_str() {
        "htop" => Err(PkgError {
            message: "no package `htop` in the index".into(),
            help: Some("did you mean `btop`?".into()),
        }),
        _ => Ok(()),
    })
    .unwrap_err();
    assert_eq!(support::codes(&report), ["C0901"]);
    assert_eq!(support::pointed_at(&report, 0), "htop");
    let help = report.iter().next().and_then(|d| d.diagnostic.help.clone());
    assert_eq!(help.as_deref(), Some("did you mean `btop`?"));
}
