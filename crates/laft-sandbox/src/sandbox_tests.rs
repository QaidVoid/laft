use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rustix::io::Errno;
use tempfile::TempDir;

use crate::error::Step;
use crate::test_support::namespaces_refused;
use crate::{Command, Error, Network, Sandbox};

/// A sandbox whose view is the host's `/usr`, bound read-only, so that real
/// programs run inside. Its `/build` and `/out` are temporary directories.
struct Fixture {
    dir: TempDir,
    sandbox: Sandbox,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("build")).unwrap();
        std::fs::create_dir(dir.path().join("out")).unwrap();
        let sandbox = Sandbox::new("/usr", dir.path().join("build"), dir.path().join("out"));
        Self { dir, sandbox }
    }

    fn build(&self) -> PathBuf {
        self.dir.path().join("build")
    }

    fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    fn shell(&self, script: &str) -> Command {
        self.sandbox.command("/usr/bin/sh").arg("-c").arg(script)
    }

    /// Runs `script` and returns its output, or `None` after printing why
    /// the test is skipped.
    fn run(&self, script: &str) -> Option<Output> {
        supported(self.shell(script).stdin(Stdio::null()).output())
    }
}

/// The value, or `None` after printing why this environment cannot run
/// sandboxes.
fn supported<T>(result: Result<T, Error>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) if namespaces_refused(&error) => {
            println!("skipping: user namespaces are unavailable: {error}");
            None
        }
        Err(error)
            if error.step() == Step::Exec
                && Errno::from_io_error(error.io_error()) == Some(Errno::NOENT) =>
        {
            println!("skipping: the host's /usr cannot run /usr/bin/sh as a view: {error}");
            None
        }
        Err(error) => panic!("{error}"),
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// The value of the `/proc/<pid>/status` field `name`.
fn status_field<'a>(status: &'a str, name: &str) -> &'a str {
    status
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .unwrap_or_else(|| panic!("no {name} in {status}"))
        .trim()
}

/// The interface names listed in a `/proc/net/dev` table.
fn interfaces(table: &str) -> BTreeSet<String> {
    table
        .lines()
        .skip(2)
        .filter_map(|line| Some(line.split_once(':')?.0.trim().to_owned()))
        .collect()
}

#[test]
fn output_and_exit_status_come_through() {
    let fixture = Fixture::new();
    let Some(output) = fixture.run("echo out; echo err >&2; exit 7") else {
        return;
    };
    assert_eq!(stdout(&output), "out\n");
    assert_eq!(output.stderr, b"err\n");
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn output_streams_while_the_command_runs() {
    let fixture = Fixture::new();
    let command = fixture
        .shell("echo ready; read line; echo \"got $line\"")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let Some(mut child) = supported(command.spawn()) else {
        return;
    };
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    assert_eq!(lines.next().unwrap().unwrap(), "ready");
    child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    assert_eq!(lines.next().unwrap().unwrap(), "got go");
    assert!(child.wait().unwrap().success());
}

#[test]
fn killing_the_child_ends_every_sandbox_process() {
    let fixture = Fixture::new();
    let command = fixture
        .shell("sleep 1000 & echo started; wait")
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let Some(mut child) = supported(command.spawn()) else {
        return;
    };
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line, "started\n");
    child.kill().unwrap();
    child.wait().unwrap();
    // The background `sleep` holds the pipe too, so end of file shows that
    // it died with the namespace.
    let (done, finished) = mpsc::channel();
    thread::spawn(move || done.send(stdout.read_to_end(&mut Vec::new()).is_ok()));
    assert_eq!(finished.recv_timeout(Duration::from_secs(20)), Ok(true));
}

#[test]
fn host_paths_are_invisible() {
    let fixture = Fixture::new();
    let host = fixture.dir.path().display();
    let manifest = env!("CARGO_MANIFEST_DIR");
    let script = format!(
        "echo /*; echo /dev/*; echo $PWD; for p in '{host}' '{manifest}' /home /etc /var; do test -e \"$p\" && echo \"visible $p\"; done; true"
    );
    let Some(output) = fixture.run(&script) else {
        return;
    };
    assert_eq!(
        stdout(&output),
        "/bin /build /dev /lib /lib64 /out /proc /sbin /tmp /usr\n\
         /dev/fd /dev/full /dev/null /dev/random /dev/stderr /dev/stdin /dev/stdout /dev/tty /dev/urandom /dev/zero\n\
         /build\n"
    );
}

#[test]
fn view_and_root_are_read_only() {
    let fixture = Fixture::new();
    let Some(output) = fixture.run(
        "mkdir /usr/laft-probe 2>&1; mkdir /laft-probe 2>&1; mkdir /dev/laft-probe 2>&1; cat /proc/self/mountinfo",
    ) else {
        return;
    };
    let text = stdout(&output);
    assert_eq!(text.matches("Read-only file system").count(), 3, "{text}");
    assert!(!Path::new("/usr/laft-probe").exists());
    let usr = text
        .lines()
        .find(|line| line.split(' ').nth(4) == Some("/usr"))
        .unwrap_or_else(|| panic!("no /usr mount in {text}"));
    let options: BTreeSet<&str> = usr.split(' ').nth(5).unwrap().split(',').collect();
    for option in ["ro", "nosuid", "nodev"] {
        assert!(options.contains(option), "{usr}");
    }
}

#[test]
fn build_out_and_tmp_are_writable() {
    let fixture = Fixture::new();
    let Some(output) = fixture
        .run("echo built > /out/result && echo work > scratch && echo temp > /tmp/t && cat /tmp/t")
    else {
        return;
    };
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "temp\n");
    assert_eq!(
        std::fs::read_to_string(fixture.out().join("result")).unwrap(),
        "built\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.build().join("scratch")).unwrap(),
        "work\n"
    );
}

#[test]
fn proc_shows_only_the_sandbox_processes() {
    let fixture = Fixture::new();
    let Some(output) = fixture.run("echo $$; echo /proc/[0-9]*") else {
        return;
    };
    assert_eq!(stdout(&output), "1\n/proc/1\n");
}

#[test]
fn isolated_network_has_only_loopback_and_reaches_nothing() {
    let fixture = Fixture::new();
    let Some(output) = fixture.run("cat /proc/net/dev") else {
        return;
    };
    assert_eq!(
        interfaces(&stdout(&output)),
        BTreeSet::from(["lo".to_owned()])
    );

    if !Path::new("/usr/bin/bash").exists() {
        println!("skipping the connection attempt: the host has no /usr/bin/bash");
        return;
    }
    let output = fixture
        .sandbox
        .command("/usr/bin/bash")
        .arg("-c")
        .arg("echo > /dev/tcp/127.0.0.1/80; echo > /dev/tcp/192.0.2.1/80")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let errors = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        errors.matches("connect: Network is unreachable").count(),
        2,
        "{errors}"
    );
}

#[test]
fn host_network_keeps_the_host_interfaces() {
    let fixture = Fixture::new();
    let sandbox = fixture.sandbox.clone().network(Network::Host);
    let command = sandbox
        .command("/usr/bin/cat")
        .arg("/proc/net/dev")
        .stdin(Stdio::null());
    let Some(output) = supported(command.output()) else {
        return;
    };
    let host = std::fs::read_to_string("/proc/self/net/dev").unwrap();
    assert_eq!(interfaces(&stdout(&output)), interfaces(&host));
}

#[test]
fn command_runs_as_root_without_capabilities_and_with_no_new_privs() {
    let fixture = Fixture::new();
    let Some(output) = fixture.run("cat /proc/self/status; cat /proc/sys/kernel/hostname") else {
        return;
    };
    let status = stdout(&output);
    for field in ["CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb"] {
        assert_eq!(status_field(&status, field), "0000000000000000", "{field}");
    }
    assert_eq!(status_field(&status, "NoNewPrivs"), "1");
    assert_eq!(status_field(&status, "Uid"), "0\t0\t0\t0");
    assert_eq!(status_field(&status, "Gid"), "0\t0\t0\t0");
    assert!(status.ends_with("laft-build\n"), "{status}");
}

#[test]
fn environment_is_clean() {
    let fixture = Fixture::new();
    let command = fixture
        .shell("env | sort")
        .env("LAFT_TEST", "1")
        .stdin(Stdio::null());
    let Some(output) = supported(command.output()) else {
        return;
    };
    let names: BTreeSet<&str> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    // A shell adds `PWD` and may add `SHLVL` and `_` itself.
    let expected = BTreeSet::from(["LAFT_TEST", "PATH", "PWD"]);
    assert!(names.is_superset(&expected), "{names:?}");
    assert!(
        names
            .difference(&expected)
            .all(|name| ["SHLVL", "_", "OLDPWD"].contains(name)),
        "{names:?}"
    );
}

#[test]
fn missing_directories_are_reported_before_forking() {
    let fixture = Fixture::new();
    let missing = fixture.dir.path().join("missing");
    let error = Sandbox::new(&missing, fixture.build(), fixture.out())
        .command("/usr/bin/true")
        .status()
        .unwrap_err();
    assert_eq!(error.step(), Step::Resolve);
    assert!(error.to_string().contains("missing"), "{error}");
}

#[test]
fn missing_program_is_reported_as_an_exec_failure() {
    let fixture = Fixture::new();
    let result = fixture
        .sandbox
        .command("/usr/bin/laft-no-such-program")
        .status();
    let error = match result {
        Err(error) if namespaces_refused(&error) => {
            println!("skipping: user namespaces are unavailable: {error}");
            return;
        }
        other => other.unwrap_err(),
    };
    assert_eq!(error.step(), Step::Exec);
    assert_eq!(Errno::from_io_error(error.io_error()), Some(Errno::NOENT));
}

#[test]
fn setup_failure_names_the_step() {
    let fixture = Fixture::new();
    std::fs::remove_dir(fixture.out()).unwrap();
    std::fs::write(fixture.out(), "a file, not a directory").unwrap();
    let result = fixture.sandbox.command("/usr/bin/true").status();
    let error = match result {
        Err(error) if namespaces_refused(&error) => {
            println!("skipping: user namespaces are unavailable: {error}");
            return;
        }
        other => other.unwrap_err(),
    };
    assert_eq!(error.step(), Step::Attach, "{error}");
    assert!(
        error.to_string().starts_with("attaching a mount: "),
        "{error}"
    );
}
