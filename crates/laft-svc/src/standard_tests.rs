use std::collections::BTreeSet;

use crate::test_support::{config, valid};
use crate::{ACTIVATE, EARLY, Getty, ServiceKind, getty_services, standard_services};

#[test]
fn standard_services_are_valid_without_warnings() {
    let set = valid(&config(&[]));
    assert!(set.warnings().is_empty());
    assert!(set.specs().contains_key("getty-tty1"));
    assert!(set.specs().contains_key("getty-tty2"));
}

#[test]
fn early_waits_for_every_other_early_oneshot() {
    let services = standard_services();
    let oneshots: BTreeSet<&str> = services
        .iter()
        .filter(|(n, s)| s.early && s.kind == ServiceKind::Oneshot && *n != EARLY)
        .map(|(n, _)| n.as_str())
        .collect();
    let after: BTreeSet<&str> = services[EARLY].after.iter().map(String::as_str).collect();
    assert_eq!(after, oneshots);
}

#[test]
fn every_early_service_runs_after_activation() {
    let services = standard_services();
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
fn every_tty_gets_a_getty_after_the_session_tracker() {
    let gettys = getty_services(&Getty {
        ttys: vec!["tty1".into(), "ttyS0".into(), "ttyUSB0".into()],
    });
    assert_eq!(
        gettys.keys().collect::<Vec<_>>(),
        ["getty-tty1", "getty-ttys0", "getty-ttyusb0"]
    );
    assert_eq!(
        gettys["getty-ttys0"].exec,
        ["/usr/bin/agetty", "--noclear", "ttyS0", "38400", "linux"]
    );
    assert_eq!(gettys["getty-ttys0"].after, ["turnstiled"]);
    for name in gettys.keys() {
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{name} follows the service name rule"
        );
    }
    assert!(getty_services(&Getty { ttys: Vec::new() }).is_empty());
}
