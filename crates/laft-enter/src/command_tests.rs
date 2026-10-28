use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;

use rustix::fs::Mode;
use rustix::io::Errno;

use super::{
    NOT_EXECUTABLE, NOT_FOUND, candidates, command_line, directories, environment, exec,
    exit_status,
};

fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect()
}

fn paths(list: &[&str]) -> Vec<PathBuf> {
    list.iter().map(PathBuf::from).collect()
}

#[test]
fn the_callers_environment_is_kept_with_the_view_added() {
    let captured = env(&[("PATH", "/usr/bin"), ("LD_PRELOAD", "/tmp/x.so")]);
    assert_eq!(
        environment(captured, "dev"),
        env(&[
            ("PATH", "/usr/bin"),
            ("LD_PRELOAD", "/tmp/x.so"),
            ("LAFT_VIEW", "dev"),
        ])
    );
}

#[test]
fn a_laft_view_from_the_caller_is_replaced() {
    let captured = env(&[("LAFT_VIEW", "system"), ("HOME", "/home/a")]);
    assert_eq!(
        environment(captured, "dev"),
        env(&[("HOME", "/home/a"), ("LAFT_VIEW", "dev")])
    );
}

#[test]
fn ambiguous_entries_are_dropped_and_the_first_duplicate_wins() {
    let captured = env(&[("", "x"), ("A=B", "c"), ("A", "1"), ("A", "2")]);
    assert_eq!(
        environment(captured, "dev"),
        env(&[("A", "1"), ("LAFT_VIEW", "dev")])
    );
}

#[test]
fn an_explicit_command_is_run_as_given() {
    let command = vec![OsString::from("make"), OsString::from("all")];
    assert_eq!(
        command_line(command.clone(), &env(&[("SHELL", "/bin/zsh")])),
        command
    );
}

#[test]
fn the_default_command_is_an_absolute_shell() {
    assert_eq!(
        command_line(vec![], &env(&[("SHELL", "/bin/zsh")])),
        [OsString::from("/bin/zsh")]
    );
    for shell in [&[("SHELL", "zsh")][..], &[("SHELL", "")], &[]] {
        assert_eq!(
            command_line(vec![], &env(shell)),
            [OsString::from("/bin/sh")]
        );
    }
}

#[test]
fn a_program_with_a_slash_is_not_searched() {
    let env = env(&[("PATH", "/usr/bin")]);
    assert_eq!(candidates("./run".as_ref(), &env), paths(&["./run"]));
    assert_eq!(candidates("/bin/sh".as_ref(), &env), paths(&["/bin/sh"]));
}

#[test]
fn a_bare_program_is_searched_in_path_order() {
    let env = env(&[("PATH", "/a::/b:")]);
    assert_eq!(candidates("ls".as_ref(), &env), paths(&["/a/ls", "/b/ls"]));
}

#[test]
fn without_path_only_usr_bin_is_searched() {
    assert_eq!(candidates("ls".as_ref(), &[]), paths(&["/usr/bin/ls"]));
}

#[test]
fn an_empty_program_names_nothing() {
    assert!(candidates("".as_ref(), &[]).is_empty());
    assert_eq!(exit_status(&[]), NOT_FOUND);
}

#[test]
fn missing_files_mean_not_found() {
    assert_eq!(exit_status(&[Errno::NOENT, Errno::NOTDIR]), NOT_FOUND);
}

#[test]
fn a_file_that_is_not_permitted_means_not_executable() {
    assert_eq!(exit_status(&[Errno::ACCESS, Errno::NOENT]), NOT_EXECUTABLE);
    assert_eq!(exit_status(&[Errno::NOENT, Errno::ACCESS]), NOT_EXECUTABLE);
}

#[test]
fn a_file_that_is_not_a_program_means_not_executable() {
    assert_eq!(exit_status(&[Errno::NOEXEC]), NOT_EXECUTABLE);
    assert_eq!(exit_status(&[Errno::NOENT, Errno::NOEXEC]), NOT_EXECUTABLE);
}

#[test]
fn the_working_directory_falls_back_to_home_then_root() {
    let home = env(&[("HOME", "/home/a")]);
    assert_eq!(
        directories(Some("/src".into()), &home),
        paths(&["/src", "/home/a", "/"])
    );
    assert_eq!(directories(None, &home), paths(&["/home/a", "/"]));
    assert_eq!(directories(None, &[]), paths(&["/"]));
}

const CREATE: &str = "LAFT_ENTER_TEST_CREATE";
const EXEC: &str = "LAFT_ENTER_TEST_EXEC";

/// Runs a child copy of this test that sets the hardened umask and then
/// executes, through [`exec`], a second copy that creates a file.
#[test]
fn the_command_runs_with_the_callers_umask() {
    let me = std::env::current_exe().unwrap();
    let args = [
        "--exact",
        "command::tests::the_command_runs_with_the_callers_umask",
        "--test-threads=1",
    ];
    if let Some(file) = std::env::var_os(CREATE) {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o666)
            .open(file)
            .unwrap();
        return;
    }
    if let Some(file) = std::env::var_os(EXEC) {
        rustix::process::umask(Mode::from_raw_mode(0o077));
        let command = std::iter::once(me.into_os_string())
            .chain(args.map(OsString::from))
            .collect();
        let failure = exec(
            command,
            vec![(CREATE.into(), file)],
            None,
            Mode::from_raw_mode(0o022),
        );
        panic!("cannot execute the test again: {failure}");
    }
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("created");
    let output = Command::new(&me)
        .args(args)
        .env(EXEC, &file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let mode = fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(format!("{:o}", mode & 0o777), "644");
}
