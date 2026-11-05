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
fn a_missing_program_is_an_error() {
    let own = rustix::process::getuid().as_raw();
    let mut command = Command::new("/nonexistent/program");
    let error = run_as(&mut command, own, own).expect_err("the program is missing");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
}
