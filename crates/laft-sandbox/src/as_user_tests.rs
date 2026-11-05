use super::*;

use std::process::Command;

#[test]
fn a_program_runs_with_the_ids_it_is_given() {
    let own = rustix::process::getuid().as_raw();
    let mut command = Command::new("/usr/bin/id");
    command.arg("-u");
    let status =
        run_as(&mut command, own, rustix::process::getgid().as_raw()).expect("the program runs");
    assert!(status.success(), "{status}");
}

#[test]
fn a_failing_program_reports_its_status() {
    let own = rustix::process::getuid().as_raw();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "exit 3"]);
    let status = run_as(&mut command, own, own).expect("the program runs");
    assert_eq!(status.code(), Some(3), "{status}");
}

#[test]
fn another_user_requires_root() {
    let own = rustix::process::getuid().as_raw();
    if own == 0 {
        return;
    }
    let mut command = Command::new("/bin/true");
    let error = run_as(&mut command, own + 1, rustix::process::getgid().as_raw())
        .expect_err("another user needs root");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
}

#[test]
fn zero_ids_are_rejected() {
    let mut command = Command::new("/bin/true");
    let error = run_as(&mut command, 0, 1).expect_err("uid zero is rejected");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{error}");
    let error = run_as(&mut command, 1, 0).expect_err("gid zero is rejected");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{error}");
}

#[test]
fn the_child_ignores_setuid_bits() {
    let own = rustix::process::getuid().as_raw();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "grep -q 'NoNewPrivs:.*1' /proc/self/status"]);
    let status =
        run_as(&mut command, own, rustix::process::getgid().as_raw()).expect("the program runs");
    assert!(status.success(), "{status}");
}

#[test]
fn a_missing_program_is_an_error() {
    let own = rustix::process::getuid().as_raw();
    let mut command = Command::new("/nonexistent/program");
    let error = run_as(&mut command, own, own).expect_err("the program is missing");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
}
