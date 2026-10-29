use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use laft_test::TempDir;
use rustix::io::Errno;

use crate::error::Step;
use crate::test_support::sandbox_refused;
use crate::{Command, Error, MAX_OPEN_FILES, Network, Sandbox};

/// A sandbox whose view is the host's `/usr`, bound read-only, so that real
/// programs run inside. Its `/build` and `/out` are temporary directories.
struct Fixture {
    dir: TempDir,
    sandbox: Sandbox,
}

impl Fixture {
    fn new() -> Self {
        let dir = laft_test::tempdir();
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
    let error = match result {
        Ok(value) => return Some(value),
        Err(error) => error,
    };
    if let Some(reason) = sandbox_refused(&error) {
        println!("skipping: {reason}");
        return None;
    }
    if error.step() == Step::Exec && errno(&error) == Some(Errno::NOENT) {
        println!("skipping: the host's /usr cannot run /usr/bin/sh as a view: {error}");
        return None;
    }
    panic!("{error}")
}

/// The expected error of a command that must fail, or `None` after printing
/// why this environment cannot run sandboxes.
fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> Option<Error> {
    let error = result.unwrap_err();
    if let Some(reason) = sandbox_refused(&error) {
        println!("skipping: {reason}");
        return None;
    }
    Some(error)
}

fn errno(error: &Error) -> Option<Errno> {
    error.io_error().and_then(Errno::from_io_error)
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
        "echo /*; echo /dev/*; echo $PWD; for p in '{host}' '{manifest}' /home /etc/hosts /var; do test -e \"$p\" && echo \"visible $p\"; done; true"
    );
    let Some(output) = fixture.run(&script) else {
        return;
    };
    assert_eq!(
        stdout(&output),
        "/bin /build /dev /etc /lib /lib64 /out /proc /sbin /tmp /usr\n\
         /dev/fd /dev/full /dev/null /dev/ptmx /dev/pts /dev/random /dev/stderr /dev/stdin /dev/stdout /dev/tty /dev/urandom /dev/zero\n\
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
fn isolated_network_has_only_loopback_and_reaches_nothing_else() {
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
        errors.matches("connect: Connection refused").count(),
        1,
        "the loopback interface is up: {errors}"
    );
    assert_eq!(
        errors.matches("connect: Network is unreachable").count(),
        1,
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
    for program in ["/usr/bin/laft-no-such-program", "laft-no-such-program"] {
        let result = fixture.sandbox.command(program).status();
        let Some(error) = failure(result) else {
            return;
        };
        assert_eq!(error.step(), Step::Exec, "{program}: {error}");
        assert_eq!(errno(&error), Some(Errno::NOENT), "{program}");
    }
}

#[test]
fn program_without_a_slash_is_found_in_the_view() {
    let fixture = Fixture::new();
    let Ok(host) = std::fs::canonicalize("/usr/bin/sh") else {
        println!("skipping: the host has no /usr/bin/sh");
        return;
    };
    let command = fixture
        .sandbox
        .command("sh")
        .arg("-c")
        .arg("readlink /proc/$$/exe");
    let Some(output) = supported(command.output()) else {
        return;
    };
    assert_eq!(stdout(&output).trim_end(), host.to_str().unwrap());
}

#[test]
fn output_gives_the_command_a_null_stdin() {
    let fixture = Fixture::new();
    let Some(output) = supported(fixture.shell("stat -L -c %t:%T /proc/self/fd/0").output()) else {
        return;
    };
    assert_eq!(
        stdout(&output),
        "1:3
",
        "{output:?}"
    );
}

#[test]
fn spawn_mirrors_a_command_killed_by_a_signal() {
    let fixture = Fixture::new();
    let command = fixture
        .shell("echo started; exec sleep 1000")
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let Some(mut child) = supported(command.spawn()) else {
        return;
    };
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line, "started\n");
    let waiter = child.id().to_string();
    let init = children_of(&waiter);
    assert_eq!(init.len(), 1, "{init:?}");
    // PID 1 of a namespace takes only SIGKILL and SIGSTOP from outside unless
    // it handles the signal.
    let pid = rustix::process::Pid::from_raw(init[0]).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
}

/// The processes whose parent is `pid`, read from `/proc`.
fn children_of(pid: &str) -> Vec<i32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let child = name.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{name}/stat")).ok()?;
            let parent = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?;
            (parent == pid).then_some(child)
        })
        .collect()
}

#[test]
fn core_dumps_are_off_and_open_files_are_limited() {
    let fixture = Fixture::new();
    let Some(output) = fixture.run("cat /proc/self/limits") else {
        return;
    };
    let limits = stdout(&output);
    let limit = |name: &str| -> Vec<String> {
        let line = limits
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("no {name} in {limits}"));
        line.split_whitespace().take(2).map(str::to_owned).collect()
    };
    assert_eq!(limit("Max core file size"), ["0", "0"]);
    let hard = rustix::process::getrlimit(rustix::process::Resource::Nofile).maximum;
    let open_files = hard.map_or(MAX_OPEN_FILES, |hard| hard.min(MAX_OPEN_FILES));
    assert_eq!(
        limit("Max open files"),
        [open_files.to_string(), open_files.to_string()]
    );
}

#[test]
fn host_network_sees_only_the_host_name_resolution_files() {
    let fixture = Fixture::new();
    let mut expected = String::new();
    let mut present = Vec::new();
    for name in ["hosts", "nsswitch.conf", "resolv.conf"] {
        if let Ok(contents) = std::fs::read_to_string(Path::new("/etc").join(name)) {
            expected.push_str(&format!("== {name}\n{contents}"));
            present.push(name);
        }
    }
    let mut listed: Vec<&str> = present.iter().copied().chain(["group", "passwd"]).collect();
    listed.sort_unstable();
    for name in listed {
        expected.push_str(&format!("{name}\n"));
    }
    let host = fixture.sandbox.clone().network(Network::Host);
    let script = "for f in hosts nsswitch.conf resolv.conf; do test -e /etc/$f && { echo \"== $f\"; cat /etc/$f; }; done; ls -A /etc; touch /etc/laft-probe; cat /proc/self/mountinfo >&2";
    let command = host.command("/usr/bin/sh").arg("-c").arg(script);
    let Some(output) = supported(command.output()) else {
        return;
    };
    assert_eq!(stdout(&output), expected);
    let errors = String::from_utf8(output.stderr).unwrap();
    assert!(errors.contains("Read-only file system"), "{errors}");
    for name in present {
        let target = format!("/etc/{name}");
        let mount = errors
            .lines()
            .find(|line| line.split(' ').nth(4) == Some(target.as_str()))
            .unwrap_or_else(|| panic!("no {target} mount in {errors}"));
        let options: BTreeSet<&str> = mount.split(' ').nth(5).unwrap().split(',').collect();
        assert!(
            options.is_superset(&BTreeSet::from(["ro", "nosuid", "nodev"])),
            "{mount}"
        );
    }

    let Some(output) = fixture
        .run("for f in hosts nsswitch.conf resolv.conf; do test -e /etc/$f && echo $f; done; true")
    else {
        return;
    };
    assert_eq!(stdout(&output), "");
}

#[test]
fn setup_failure_names_the_step() {
    let fixture = Fixture::new();
    std::fs::remove_dir(fixture.out()).unwrap();
    std::fs::write(fixture.out(), "a file, not a directory").unwrap();
    let result = fixture.sandbox.command("/usr/bin/true").status();
    let Some(error) = failure(result) else {
        return;
    };
    assert_eq!(error.step(), Step::Attach, "{error}");
    assert!(
        error.to_string().starts_with("attaching a mount: "),
        "{error}"
    );
}

#[test]
fn a_command_opens_pseudo_terminals_of_its_own() {
    let fixture = Fixture::new();
    let Some(output) = fixture.run("exec 3<>/dev/ptmx && test -c /dev/pts/0 && echo opened") else {
        return;
    };
    assert_eq!(stdout(&output), "opened\n", "{output:?}");
}

#[test]
fn the_user_and_group_of_a_command_have_names() {
    let fixture = Fixture::new();
    let Some(output) =
        fixture.run("whoami; id -gn; touch /etc/laft-probe 2>/dev/null || echo read-only")
    else {
        return;
    };
    assert_eq!(stdout(&output), "root\nroot\nread-only\n", "{output:?}");
}
