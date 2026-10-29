//! The names of the services that Laft itself relies on, and the gettys
//! generated from the configuration.
//!
//! The standard services are declared once, in the base part of the system
//! configuration. Only the names that this crate gives a meaning are here.

use std::collections::BTreeMap;

use crate::model::{Getty, OnChange, Service, strings};

/// The oneshot that ends the early boot graph. Every service outside the
/// graph depends on it implicitly.
pub const EARLY: &str = "early";

/// The oneshot that runs before every other service and reconciles `/etc`
/// and the accounts with the booted generation.
pub const ACTIVATE: &str = "laft-activate";

/// The oneshot that marks the boot as good.
pub const BOOT_COMPLETE: &str = "boot-complete";

/// The login session tracker that every getty is ordered after.
pub const TURNSTILED: &str = "turnstiled";

/// The prefix of the services generated from [`Getty::ttys`].
pub const GETTY_PREFIX: &str = "getty-";

/// The name of the getty service for `tty`: the terminal name in lowercase
/// after [`GETTY_PREFIX`], so that `ttyS0` gets `getty-ttys0` and the name
/// follows the rule for service names. Distinct valid terminal names stay
/// distinct, because only the `S` and `USB` prefixes carry uppercase.
pub fn getty_name(tty: &str) -> String {
    format!("{GETTY_PREFIX}{}", tty.to_ascii_lowercase())
}

/// The services that give every terminal of `getty` a login prompt, by name.
pub fn getty_services(getty: &Getty) -> BTreeMap<String, Service> {
    getty
        .ttys
        .iter()
        .map(|tty| {
            let service = Service {
                description: format!("Login prompt on {tty}"),
                after: strings(&[TURNSTILED]),
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
