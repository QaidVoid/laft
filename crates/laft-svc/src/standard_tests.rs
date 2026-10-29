use crate::test_support::{config, valid};
use crate::{Getty, Service, ServiceConfig, TURNSTILED, getty_services};

#[test]
fn every_tty_gets_a_getty_after_the_session_tracker() {
    let set = valid(&config(&[]));
    assert!(set.specs().contains_key("getty-tty1"));
    assert!(set.specs().contains_key("getty-tty2"));
    let gettys = getty_services(&ServiceConfig {
        getty: Getty {
            ttys: vec!["tty1".into(), "ttyS0".into(), "ttyUSB0".into()],
        },
        ..config(&[])
    });
    assert_eq!(
        gettys.keys().collect::<Vec<_>>(),
        ["getty-tty1", "getty-ttys0", "getty-ttyusb0"]
    );
    assert_eq!(
        gettys["getty-ttys0"].exec,
        ["/usr/bin/agetty", "--noclear", "ttyS0", "38400", "linux"]
    );
    assert_eq!(gettys["getty-ttys0"].after, [TURNSTILED]);
    for name in gettys.keys() {
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{name} follows the service name rule"
        );
    }
    let none = ServiceConfig {
        getty: Getty { ttys: Vec::new() },
        ..config(&[])
    };
    assert!(getty_services(&none).is_empty());
}

#[test]
fn gettys_start_without_a_disabled_or_missing_session_tracker() {
    let disabled = config(&[(
        TURNSTILED,
        Service {
            enable: false,
            ..Service::longrun(&["/usr/bin/turnstiled"])
        },
    )]);
    let mut missing = config(&[]);
    missing.services.remove(TURNSTILED);
    for config in [disabled, missing] {
        let set = valid(&config);
        assert!(!set.specs().contains_key(TURNSTILED));
        assert!(
            !set.specs()["getty-tty1"]
                .after
                .contains(&TURNSTILED.to_owned())
        );
    }
}
