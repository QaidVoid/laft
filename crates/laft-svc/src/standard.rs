//! The services every system declares, and the early boot graph among them.

use std::collections::BTreeMap;

use crate::model::{Getty, Log, OnChange, Ready, Service, strings};

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

/// The commands that implement early boot steps which no single standard
/// tool performs, run as `laft svc builtin <step> up|down`.
fn builtin(step: &str, action: &str) -> Vec<String> {
    strings(&["/usr/bin/laft", "svc", "builtin", step, action])
}

fn early_oneshot(up: Vec<String>, after: &[&str], on_change: OnChange) -> Service {
    Service {
        kind: crate::ServiceKind::Oneshot,
        up,
        after: strings(after),
        log: Log::Console,
        on_change: Some(on_change),
        early: true,
        ..Service::default()
    }
}

/// The early oneshots, without `early` itself, which depends on all of them.
fn early_oneshots() -> Vec<(&'static str, Service)> {
    vec![
        (
            ACTIVATE,
            Service {
                description: "Reconcile /etc and accounts with the booted generation".into(),
                ..early_oneshot(
                    strings(&["/usr/bin/laft", "activate", "--boot"]),
                    &[],
                    OnChange::Ignore,
                )
            },
        ),
        (
            "kernel-fs",
            Service {
                description: "Mount the kernel's pseudo filesystems".into(),
                ..early_oneshot(builtin("kernel-fs", "up"), &[ACTIVATE], OnChange::Ignore)
            },
        ),
        (
            "kmod",
            Service {
                description: "Load the configured kernel modules".into(),
                ..early_oneshot(builtin("kmod", "up"), &["kernel-fs"], OnChange::Rerun)
            },
        ),
        (
            "udev-trigger",
            Service {
                description: "Replay kernel device events".into(),
                ..early_oneshot(builtin("udev-trigger", "up"), &["kmod"], OnChange::Ignore)
            },
        ),
        (
            "clock",
            Service {
                description: "Set the system clock from the hardware clock".into(),
                down: strings(&["/usr/bin/hwclock", "--systohc", "--utc"]),
                ..early_oneshot(
                    strings(&["/usr/bin/hwclock", "--hctosys", "--utc"]),
                    &["kernel-fs"],
                    OnChange::Ignore,
                )
            },
        ),
        (
            "hostname",
            Service {
                description: "Set the host name".into(),
                ..early_oneshot(builtin("hostname", "up"), &["kernel-fs"], OnChange::Rerun)
            },
        ),
        (
            "mount-fs",
            Service {
                description: "Check and mount the configured filesystems".into(),
                down: builtin("mount-fs", "down"),
                ..early_oneshot(
                    builtin("mount-fs", "up"),
                    &["udev-trigger"],
                    OnChange::Rerun,
                )
            },
        ),
        (
            "swap",
            Service {
                description: "Enable swap".into(),
                down: strings(&["/usr/bin/swapoff", "-a"]),
                ..early_oneshot(
                    strings(&["/usr/bin/swapon", "-a"]),
                    &["mount-fs"],
                    OnChange::Rerun,
                )
            },
        ),
        (
            "random-seed",
            Service {
                description: "Carry the random seed across boots".into(),
                down: builtin("random-seed", "down"),
                ..early_oneshot(
                    builtin("random-seed", "up"),
                    &["mount-fs"],
                    OnChange::Ignore,
                )
            },
        ),
        (
            "sysctl",
            Service {
                description: "Apply kernel parameters".into(),
                ..early_oneshot(
                    strings(&["/usr/bin/sysctl", "-q", "-p", "/etc/sysctl.d/laft.conf"]),
                    &["kmod", "mount-fs"],
                    OnChange::Rerun,
                )
            },
        ),
        (
            "console",
            Service {
                description: "Load the console keymap".into(),
                ..early_oneshot(builtin("console", "up"), &["udev-trigger"], OnChange::Rerun)
            },
        ),
    ]
}

/// The services the base part declares on every system, by name.
///
/// They are the early boot graph, which brings the machine from PID 1 to the
/// point where normal services may run and ends in the oneshot [`EARLY`],
/// plus the session tracker [`TURNSTILED`] and [`BOOT_COMPLETE`]. The gettys
/// are not among them, because they are generated from [`Getty`] by
/// [`getty_services`].
///
/// ```
/// let services = laft_svc::standard_services();
/// assert!(services[laft_svc::ACTIVATE].after.is_empty());
/// assert!(services[laft_svc::EARLY].after.contains(&"sysctl".to_owned()));
/// ```
pub fn standard_services() -> BTreeMap<String, Service> {
    let oneshots = early_oneshots();
    let mut early_after: Vec<&str> = oneshots.iter().map(|(name, _)| *name).collect();
    early_after.sort_unstable();
    let mut services: BTreeMap<String, Service> = oneshots
        .into_iter()
        .map(|(name, service)| (name.to_owned(), service))
        .collect();
    services.insert(
        EARLY.to_owned(),
        Service {
            description: "End of the early boot graph".into(),
            ..early_oneshot(strings(&["/usr/bin/true"]), &early_after, OnChange::Ignore)
        },
    );
    services.insert(
        "udevd".to_owned(),
        Service {
            description: "Device manager".into(),
            ready: Ready::Check {
                cmd: strings(&["/usr/bin/udevadm", "control", "--ping"]),
                interval: 1,
                timeout: 60,
            },
            after: strings(&["kernel-fs"]),
            log: Log::Console,
            on_change: Some(OnChange::Restart),
            early: true,
            ..Service::longrun(&["/usr/bin/udevd"])
        },
    );
    services.insert(
        TURNSTILED.to_owned(),
        Service {
            description: "Login session tracker".into(),
            on_change: Some(OnChange::Restart),
            ..Service::longrun(&["/usr/bin/turnstiled"])
        },
    );
    services.insert(
        BOOT_COMPLETE.to_owned(),
        Service {
            description: "Mark the boot as good".into(),
            on_change: Some(OnChange::Ignore),
            ..Service::oneshot(&["/usr/bin/laft", "boot", "mark-good"])
        },
    );
    services
}

/// The name of the getty service for `tty`.
pub fn getty_name(tty: &str) -> String {
    format!("{GETTY_PREFIX}{tty}")
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
