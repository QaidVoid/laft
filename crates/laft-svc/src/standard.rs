//! The names of the services that Laft itself relies on, and the gettys
//! generated from the configuration.
//!
//! The standard services are declared once, in the base part of the system
//! configuration. Only the names that this crate gives a meaning are here.

use std::collections::BTreeMap;

use crate::model::{OnChange, Service, ServiceConfig, strings};

/// The oneshot that ends the early boot graph. Every service outside the
/// graph depends on it implicitly.
pub const EARLY: &str = "early";

/// The oneshot that runs before every other service and reconciles `/etc`
/// and the accounts with the booted generation.
pub const ACTIVATE: &str = "laft-activate";

/// The oneshot that marks the boot as good.
pub const BOOT_COMPLETE: &str = "boot-complete";

/// The login session tracker that every getty is ordered after when it is
/// enabled.
pub const TURNSTILED: &str = "turnstiled";

/// The files and directories below a generation's `etc` that standard early
/// oneshots read, by service name, for the oneshots that can safely run
/// again on a live system. A directory stands for every file directly in
/// it. Their content is not part of a spec, so reconciliation compares
/// them between generations with
/// [`etc_changed`](crate::reconcile::etc_changed) to tell whether such a
/// service changed. `mount-fs` and `swap` also read `fstab`, but are left
/// out, because running `mount-fs` again first unmounts filesystems that
/// may be in use. A changed `fstab` takes effect at the next boot.
pub const ETC_INPUTS: &[(&str, &[&str])] = &[
    ("console", &["vconsole.conf"]),
    ("hostname", &["hostname"]),
    ("kmod", &["modprobe.d", "modules-load.d"]),
    ("sysctl", &["sysctl.d/laft.conf"]),
];

/// The prefix of the services generated from [`Getty::ttys`](crate::Getty::ttys).
pub const GETTY_PREFIX: &str = "getty-";

/// The name of the getty service for `tty`: the terminal name in lowercase
/// after [`GETTY_PREFIX`], so that `ttyS0` gets `getty-ttys0` and the name
/// follows the rule for service names. Distinct valid terminal names stay
/// distinct, because only the `S` and `USB` prefixes carry uppercase.
pub fn getty_name(tty: &str) -> String {
    format!("{GETTY_PREFIX}{}", tty.to_ascii_lowercase())
}

/// The services that give every terminal of `config.getty` a login prompt,
/// by name. Each is ordered after [`TURNSTILED`] only when `config`
/// enables it, so that a system without the session tracker still gets
/// its gettys.
pub fn getty_services(config: &ServiceConfig) -> BTreeMap<String, Service> {
    let tracked = config.services.get(TURNSTILED).is_some_and(|s| s.enable);
    let after = if tracked {
        strings(&[TURNSTILED])
    } else {
        Vec::new()
    };
    config
        .getty
        .ttys
        .iter()
        .map(|tty| {
            let service = Service {
                description: format!("Login prompt on {tty}"),
                after: after.clone(),
                on_change: Some(OnChange::Restart),
                ..Service::longrun(&["/usr/bin/agetty", "--noclear", tty, "38400", "linux"])
            };
            (getty_name(tty), service)
        })
        .collect()
}

#[cfg(test)]
#[path = "standard_tests.rs"]
mod tests;
