use crate::test_support::{config, valid};
use crate::{Getty, getty_services};

#[test]
fn every_tty_gets_a_getty_after_the_session_tracker() {
    let set = valid(&config(&[]));
    assert!(set.specs().contains_key("getty-tty1"));
    assert!(set.specs().contains_key("getty-tty2"));
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
