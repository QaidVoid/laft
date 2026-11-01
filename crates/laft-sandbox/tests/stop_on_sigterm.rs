//! A caught SIGTERM sets the stop flag instead of ending the process.

use std::time::{Duration, Instant};

#[test]
fn a_caught_sigterm_sets_the_stop_flag() {
    assert!(!laft_sandbox::terminated());
    laft_sandbox::catch_termination();
    assert!(!laft_sandbox::terminated());
    // SAFETY: raising a signal to this process runs the handler that
    // `catch_termination` installed, which writes one atomic.
    unsafe {
        assert_eq!(libc::kill(libc::getpid(), libc::SIGTERM), 0);
    }
    let asked = Instant::now();
    while !laft_sandbox::terminated() {
        assert!(
            asked.elapsed() < Duration::from_secs(10),
            "the SIGTERM never set the stop flag"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
