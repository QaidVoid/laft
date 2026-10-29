use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};

use laft_test::TempDir;

use super::{ExecMode, Launcher, parse_exiting, parse_start_time, reaped_by, restarts, start_time};
use crate::launcher::accounts::Credentials;
use crate::launcher::process::{drop_privileges, spawn_as};
use crate::spec::{Spec, SpecDir};
use crate::validate::resolve;
use crate::{Error, Restart, Service, ServiceKind};

/// Held by every fixture, because the launcher changes the process umask
/// while it starts a command, and tests run on parallel threads.
static UMASK: Mutex<()> = Mutex::new(());

/// A temporary root with account databases in which `root` is uid 0 and
/// `me` is the user running the tests.
struct Fixture {
    dir: TempDir,
    launcher: Launcher,
    _umask: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let umask = UMASK.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = laft_test::tempdir();
        let (uid, gid) = (
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw(),
        );
        fs::create_dir_all(dir.path().join("etc")).expect("mkdir");
        fs::write(
            dir.path().join("etc/passwd"),
            format!("root:x:0:0:root:/root:/bin/sh\nme:x:{uid}:{gid}::/home/me:/bin/sh\n"),
        )
        .expect("write");
        fs::write(
            dir.path().join("etc/group"),
            format!("root:x:0:\nme:x:{gid}:\n"),
        )
        .expect("write");
        let launcher = Launcher::new(dir.path());
        Self {
            dir,
            launcher,
            _umask: umask,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    fn install(&self, spec: &Spec) {
        let path = self.path(&format!("run/laft/system/services/spec/{}.json", spec.name));
        fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
        fs::write(path, spec.to_bytes().expect("encodes")).expect("write");
    }

    fn ready(&self, name: &str) -> bool {
        self.path(&format!("run/runit/ready/{name}")).exists()
    }

    fn mark(&self, name: &str) {
        self.launcher.mark_ready(name).expect("marks");
    }

    /// Creates the supervisor's control pipe of `name`, as a plain file,
    /// in the service directory under the scan directory.
    fn supervise_control(&self, name: &str) -> PathBuf {
        let control = self.path(&format!("run/runit/service/{name}/supervise/control"));
        fs::create_dir_all(control.parent().expect("has a parent")).expect("mkdir");
        fs::write(&control, "").expect("write");
        control
    }

    fn failures(&self) -> String {
        fs::read_to_string(self.path("run/laft/boot-failures")).unwrap_or_default()
    }

    /// A oneshot whose `up` runs `script` with `sh`.
    fn oneshot(&self, name: &str, script: &str, after: &[&str], early: bool) -> Spec {
        let mut spec = resolve(name, &Service::oneshot(&["/bin/sh", "-c", script]));
        spec.after = after.iter().map(|&s| s.to_owned()).collect();
        spec.early = early;
        self.install(&spec);
        spec
    }
}

fn sh(script: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), script.into()]
}

#[test]
fn missing_spec_is_reported() {
    let fixture = Fixture::new();
    let error = fixture.launcher.up("ghost").unwrap_err();
    assert_eq!(
        error.to_string(),
        "no spec for service ghost in the active generation"
    );
}

#[test]
fn commands_refuse_the_wrong_kind() {
    let fixture = Fixture::new();
    fixture.install(&resolve("daemon", &Service::longrun(&["/bin/true"])));
    assert!(matches!(
        fixture.launcher.up("daemon"),
        Err(Error::WrongKind { .. })
    ));
}

#[test]
fn dependency_gate_waits_for_ready_markers() {
    let fixture = Fixture::new();
    let flag = fixture.path("ran");
    fixture.oneshot(
        "job",
        &format!("touch {}", flag.display()),
        &["dbus", "early"],
        false,
    );
    fixture.mark("early");
    let error = fixture.launcher.up("job").unwrap_err();
    assert_eq!(error.to_string(), "job waiting for dbus");
    assert!(!flag.exists());
    fixture.mark("dbus");
    assert_eq!(fixture.launcher.up("job").expect("runs"), 0);
    assert!(flag.exists());
    assert!(fixture.ready("job"));
}

#[test]
fn oneshot_returns_its_exit_status_and_stays_unready_on_failure() {
    let fixture = Fixture::new();
    fixture.oneshot("job", "exit 3", &[], false);
    assert_eq!(fixture.launcher.up("job").expect("runs"), 3);
    assert!(!fixture.ready("job"));
    fixture.oneshot("killed", "kill -s KILL $$", &[], false);
    assert_eq!(fixture.launcher.up("killed").expect("runs"), 128 + 9);
}

#[test]
fn oneshot_runs_with_its_environment_umask_and_working_directory() {
    let fixture = Fixture::new();
    let out = fixture.path("out");
    let work = fixture.path("work");
    fs::create_dir(&work).expect("mkdir");
    let mut spec = fixture.oneshot(
        "job",
        &format!(
            "echo \"$USER $HOME $GREETING $(pwd) $(umask)\" > {}",
            out.display()
        ),
        &[],
        false,
    );
    spec.env.insert("GREETING".into(), "hello".into());
    spec.cwd = work.display().to_string();
    spec.umask = 0o027;
    fixture.install(&spec);
    assert_eq!(fixture.launcher.up("job").expect("runs"), 0);
    let text = fs::read_to_string(out).expect("written");
    assert_eq!(text, format!("root /root hello {} 0027\n", work.display()));
}

#[test]
fn a_oneshot_gets_its_limits_and_the_launcher_keeps_its_own() {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let fixture = Fixture::new();
    let original = getrlimit(Resource::Nofile);
    let Some(hard) = original.maximum.filter(|&hard| hard > 64) else {
        println!("skipping: the open file limit is unlimited or too small");
        return;
    };
    let launcher_limit = Rlimit {
        current: Some(hard - 1),
        maximum: Some(hard),
    };
    setrlimit(Resource::Nofile, launcher_limit).expect("lowers the soft limit");
    let out = fixture.path("out");
    let mut spec = fixture.oneshot("job", &format!("ulimit -n > {}", out.display()), &[], false);
    spec.limits.nofile = Some(hard);
    fixture.install(&spec);
    let code = fixture.launcher.up("job");
    let after = getrlimit(Resource::Nofile);
    setrlimit(Resource::Nofile, original).expect("restores the test's limit");
    assert_eq!(code.expect("runs"), 0);
    assert_eq!(read(&out), format!("{hard}\n"));
    assert_eq!(after, launcher_limit);
}

#[test]
fn early_failure_is_recorded_and_dependents_skip() {
    let fixture = Fixture::new();
    let flag = fixture.path("swap-ran");
    fixture.oneshot("mount-fs", "exit 1", &[], true);
    fixture.oneshot(
        "swap",
        &format!("touch {}", flag.display()),
        &["mount-fs"],
        true,
    );
    fixture.oneshot("clock", "exit 0", &[], true);
    assert_eq!(fixture.launcher.up("mount-fs").expect("never fails"), 0);
    assert_eq!(fixture.launcher.up("swap").expect("never fails"), 0);
    assert_eq!(fixture.launcher.up("clock").expect("never fails"), 0);
    assert_eq!(fixture.failures(), "mount-fs failed\nswap skipped\n");
    assert!(!flag.exists());
    assert!(!fixture.ready("mount-fs") && !fixture.ready("swap"));
    assert!(fixture.ready("clock"));
}

#[test]
fn a_failed_early_oneshot_never_holds_back_normal_services() {
    let fixture = Fixture::new();
    fixture.oneshot("swap", "exit 1", &[], true);
    fixture.oneshot("clock", "exit 0", &[], true);
    fixture.oneshot("early", "exit 0", &["clock", "swap"], true);
    let flag = fixture.path("sshd-ran");
    fixture.oneshot(
        "sshd-keys",
        &format!("touch {}", flag.display()),
        &["early"],
        false,
    );
    for name in ["swap", "clock", "early"] {
        assert_eq!(fixture.launcher.up(name).expect("never fails"), 0);
    }
    assert!(fixture.ready("early"));
    assert_eq!(fixture.failures(), "swap failed\n");
    assert_eq!(fixture.launcher.up("sshd-keys").expect("runs"), 0);
    assert!(flag.exists());
}

#[test]
fn early_becomes_ready_even_when_its_own_command_fails() {
    let fixture = Fixture::new();
    fixture.oneshot("early", "exit 1", &[], true);
    assert_eq!(fixture.launcher.up("early").expect("never fails"), 0);
    assert!(fixture.ready("early"));
    assert_eq!(fixture.failures(), "early failed\n");
}

#[test]
fn early_setup_failure_counts_as_a_failure() {
    let fixture = Fixture::new();
    let mut spec = fixture.oneshot("hostname", "exit 0", &[], true);
    spec.user = "ghost".into();
    fixture.install(&spec);
    assert_eq!(fixture.launcher.up("hostname").expect("never fails"), 0);
    assert_eq!(fixture.failures(), "hostname failed\n");
}

#[test]
fn failed_activation_does_not_skip_its_dependents() {
    let fixture = Fixture::new();
    let flag = fixture.path("mounted");
    fixture.oneshot("laft-activate", "exit 1", &[], true);
    fixture.oneshot(
        "kernel-fs",
        &format!("touch {}", flag.display()),
        &["laft-activate"],
        true,
    );
    fixture.launcher.up("laft-activate").expect("never fails");
    fixture.launcher.up("kernel-fs").expect("never fails");
    assert_eq!(fixture.failures(), "laft-activate failed\n");
    assert!(flag.exists());
}

#[test]
fn state_directories_are_prepared_before_the_command() {
    let fixture = Fixture::new();
    let mut spec = fixture.oneshot("job", "exit 0", &[], false);
    spec.state_dirs = vec![SpecDir {
        path: "/var/lib/job/data".into(),
        mode: 0o750,
        owner: Some("me".into()),
        group: None,
    }];
    fixture.install(&spec);
    assert_eq!(fixture.launcher.up("job").expect("runs"), 0);
    let meta = fs::metadata(fixture.path("var/lib/job/data")).expect("created");
    assert_eq!(meta.mode() & 0o7777, 0o750);
    assert_eq!(meta.uid(), rustix::process::getuid().as_raw());
}

#[test]
fn down_runs_and_clears_the_marker() {
    let fixture = Fixture::new();
    let flag = fixture.path("down-ran");
    let mut spec = fixture.oneshot("job", "exit 0", &[], false);
    spec.down = sh(&format!("touch {}", flag.display()));
    fixture.install(&spec);
    fixture.mark("job");
    assert_eq!(fixture.launcher.down("job").expect("runs"), 0);
    assert!(flag.exists());
    assert!(!fixture.ready("job"));
}

#[test]
fn restart_policy_decides_after_an_exit() {
    let mut spec = resolve("x", &Service::longrun(&["/bin/true"]));
    for (restart, code, expected) in [
        (Restart::Always, 0, true),
        (Restart::Always, -1, true),
        (Restart::OnFailure, 0, false),
        (Restart::OnFailure, 1, true),
        (Restart::OnFailure, -1, true),
        (Restart::Never, 1, false),
        (Restart::Never, 111, false),
        (Restart::Never, -1, false),
        (Restart::OnFailure, 111, true),
        (Restart::Always, 100, true),
    ] {
        spec.restart = restart;
        assert_eq!(restarts(&spec, code), expected, "{restart:?} {code}");
    }
    spec.kind = ServiceKind::Oneshot;
    assert!(!restarts(&spec, 100));
}

#[test]
fn finish_keeps_a_service_down_when_its_policy_says_so() {
    let fixture = Fixture::new();
    let mut spec = resolve("x", &Service::longrun(&["/bin/true"]));
    spec.restart = Restart::OnFailure;
    spec.restart_delay_ms = 0;
    fixture.install(&spec);
    let control = fixture.supervise_control("x");
    fixture.mark("x");
    fixture.launcher.finish("x", 1, 0).expect("finishes");
    assert_eq!(read(&control), "");
    assert!(!fixture.ready("x"));
    fixture.launcher.finish("x", 0, 0).expect("finishes");
    assert_eq!(read(&control), "d");
}

#[test]
fn finish_counts_a_killed_service_as_failed() {
    let fixture = Fixture::new();
    let mut spec = resolve("x", &Service::longrun(&["/bin/true"]));
    spec.restart = Restart::OnFailure;
    spec.restart_delay_ms = 0;
    fixture.install(&spec);
    let control = fixture.supervise_control("x");
    fixture.launcher.finish("x", -1, 9).expect("finishes");
    assert_eq!(read(&control), "");
}

#[test]
fn finish_stops_an_emulated_oneshot_with_its_down_command() {
    let fixture = Fixture::new();
    let flag = fixture.path("down-ran");
    let mut spec = fixture.oneshot("boot-complete", "exit 0", &[], false);
    spec.down = sh(&format!("touch {}", flag.display()));
    spec.restart_delay_ms = 0;
    fixture.install(&spec);
    let control = fixture.supervise_control("boot-complete");
    fixture
        .launcher
        .finish("boot-complete", 100, 0)
        .expect("finishes");
    assert_eq!(read(&control), "d", "a failed up is not restarted");
    assert!(!flag.exists(), "down runs only after a successful up");
    fixture.mark("boot-complete");
    fixture
        .launcher
        .finish("boot-complete", -1, 15)
        .expect("finishes");
    assert!(flag.exists());
    assert!(!fixture.ready("boot-complete"));
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("readable")
}

#[test]
fn notify_watcher_marks_ready_only_on_a_newline() {
    let fixture = Fixture::new();
    assert!(
        !fixture
            .launcher
            .watch_notify("dbus", &b"partial"[..])
            .expect("watches")
    );
    assert!(!fixture.ready("dbus"));
    assert!(
        fixture
            .launcher
            .watch_notify("dbus", &b"unix:path=/run/dbus\n"[..])
            .expect("watches")
    );
    assert!(fixture.ready("dbus"));
}

#[test]
fn check_poller_marks_ready_once_the_check_passes() {
    let fixture = Fixture::new();
    let spec = resolve("udevd", &Service::longrun(&["/bin/true"]));
    let credentials = Credentials {
        user: "root".into(),
        uid: 0,
        gid: 0,
        groups: vec![0],
        home: "/root".into(),
        shell: "/bin/sh".into(),
        declares_groups: false,
    };
    let failing = sh("exit 1");
    let passed = fixture
        .launcher
        .poll_check(&spec, &credentials, &failing, 1000, 1000)
        .expect("polls");
    assert!(!passed && !fixture.ready("udevd"));
    let flag = fixture.path("up");
    let second_try = sh(&format!(
        "test -e {0} || {{ touch {0}; exit 1; }}",
        flag.display()
    ));
    let passed = fixture
        .launcher
        .poll_check(&spec, &credentials, &second_try, 1, 5000)
        .expect("polls");
    assert!(passed && fixture.ready("udevd"));
}

#[test]
fn a_hanging_check_is_killed_at_the_timeout() {
    let fixture = Fixture::new();
    let spec = resolve("udevd", &Service::longrun(&["/bin/true"]));
    let credentials = Credentials {
        user: "root".into(),
        uid: 0,
        gid: 0,
        groups: vec![0],
        home: "/root".into(),
        shell: "/bin/sh".into(),
        declares_groups: false,
    };
    let started = std::time::Instant::now();
    let passed = fixture
        .launcher
        .poll_check(&spec, &credentials, &sh("sleep 30"), 1000, 300)
        .expect("polls");
    assert!(!passed && !fixture.ready("udevd"));
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn logger_directory_and_rotation_settings_are_written() {
    let fixture = Fixture::new();
    let (uid, gid) = (
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw(),
    );
    fixture
        .launcher
        .prepare_log("myapp", 5_000_000, 5, uid, gid)
        .expect("prepares");
    let dir = fixture.path("var/log/myapp");
    assert_eq!(fs::metadata(&dir).expect("exists").mode() & 0o7777, 0o750);
    assert_eq!(read(&dir.join("config")), "s5000000\nn5\n");
}

#[test]
fn logger_refuses_a_service_that_is_not_logged() {
    let fixture = Fixture::new();
    let spec = resolve(
        "quiet",
        &Service {
            log: crate::Log::Discard,
            ..Service::longrun(&["/bin/true"])
        },
    );
    fixture.install(&spec);
    let error = fixture.launcher.logger("quiet").unwrap_err();
    assert_eq!(error.to_string(), "service quiet is not logged");
}

#[test]
fn background_start_skips_after_a_failed_dependency() {
    let fixture = Fixture::new();
    let mut spec = resolve("udevd", &Service::longrun(&["/bin/true"]));
    spec.early = true;
    spec.after = vec!["kernel-fs".into()];
    fixture.install(&spec);
    fixture
        .launcher
        .record_failure("kernel-fs", "failed")
        .expect("records");
    let code = fixture
        .launcher
        .exec("udevd", ExecMode::Background)
        .expect("never fails");
    assert_eq!(code, 0);
    assert_eq!(fixture.failures(), "kernel-fs failed\nudevd skipped\n");
}

/// An early longrun `udevd` listed for `stop-early`, stopped with `TERM`.
fn early_udevd(fixture: &Fixture) {
    let mut spec = resolve("udevd", &Service::longrun(&["/bin/true"]));
    spec.early = true;
    spec.stop_timeout_ms = 5000;
    fixture.install(&spec);
    let list = fixture.path("run/laft/booted/services/runit/early-longruns");
    fs::create_dir_all(list.parent().expect("has a parent")).expect("mkdir");
    fs::write(&list, "udevd\nmissing\n").expect("write");
}

fn write_pid_file(fixture: &Fixture, text: &str) -> PathBuf {
    let pid_file = fixture.path("run/runit/early/udevd.pid");
    fs::create_dir_all(pid_file.parent().expect("has a parent")).expect("mkdir");
    fs::write(&pid_file, text).expect("write");
    pid_file
}

#[test]
fn stop_early_signals_each_listed_process() {
    let fixture = Fixture::new();
    early_udevd(&fixture);
    let mut child = Command::new("/usr/bin/sleep")
        .arg("100")
        .spawn()
        .expect("spawns");
    let started = start_time(child.id()).expect("the child has a start time");
    let pid_file = write_pid_file(&fixture, &format!("{} {started}\n", child.id()));
    let status = std::thread::scope(|scope| {
        let reaper = scope.spawn(|| child.wait().expect("waits"));
        assert_eq!(fixture.launcher.stop_early().expect("stops"), 0);
        reaper.join().expect("the reaper finishes")
    });
    assert_eq!(status.signal(), Some(15));
    assert!(!pid_file.exists());
}

#[test]
fn stop_early_leaves_a_process_that_reused_the_pid_alone() {
    let fixture = Fixture::new();
    early_udevd(&fixture);
    let mut child = Command::new("/usr/bin/sleep")
        .arg("100")
        .spawn()
        .expect("spawns");
    let started = start_time(child.id()).expect("the child has a start time");
    for text in [
        format!("{} {}\n", child.id(), started + 1),
        format!("{}\n", child.id()),
    ] {
        let pid_file = write_pid_file(&fixture, &text);
        assert_eq!(fixture.launcher.stop_early().expect("stops"), 0);
        assert!(!pid_file.exists(), "a stale pid file is removed");
        assert!(child.try_wait().expect("waits").is_none(), "{text:?}");
    }
    child.kill().expect("kills");
    child.wait().expect("waits");
}

#[test]
fn start_time_is_read_after_the_command_name() {
    let stat = "42 (a) b) (c) S 1 42 42 0 -1 4194560 100 0 0 0 0 0 0 0 20 0 1 0 98765 1000 10";
    assert_eq!(parse_start_time(stat), Some(98765));
    assert_eq!(parse_start_time("42 (x) S 1"), None);
    assert_eq!(parse_start_time("garbage"), None);
}

#[test]
fn a_background_start_that_crashes_leaves_no_pid_file() {
    // The status pipe closes before the exiting process can be waited
    // for, so a single start rarely hits the window between the two.
    for _ in 0..200 {
        let fixture = Fixture::new();
        let mut spec = resolve("udevd", &Service::longrun(&["/bin/true"]));
        spec.early = true;
        fixture.install(&spec);
        let mut crash = Command::new("/bin/sh");
        crash.args(["-c", "exit 1"]);
        let code = fixture
            .launcher
            .start_detached(&spec, crash)
            .expect("never fails");
        assert_eq!(code, 0);
        assert_eq!(fixture.failures(), "udevd failed\n");
        assert!(!fixture.ready("udevd"));
        assert!(!fixture.path("run/runit/early/udevd.pid").exists());
    }
}

#[test]
fn exiting_processes_are_told_from_running_ones() {
    let stat = |state: &str, flags: u32| {
        format!("42 (a (b) c) {state} 1 42 42 0 -1 {flags} 0 0 0 0 0 0 0 0 20 0 1 0 1234 0 0")
    };
    assert!(!parse_exiting(&stat("S", 0x0040_0100)));
    assert!(!parse_exiting(&stat("R", 0)));
    assert!(parse_exiting(&stat("R", 0x0040_0104)));
    assert!(parse_exiting(&stat("Z", 0)));
    assert!(parse_exiting(&stat("X", 0)));
    assert!(parse_exiting("garbage"));
}

#[test]
fn waiting_for_an_exiting_process_ends_at_the_deadline() {
    use std::time::{Duration, Instant};
    let mut stuck = Command::new("sleep").arg("5").spawn().unwrap();
    let started = Instant::now();
    let deadline = started + Duration::from_millis(200);
    assert!(!reaped_by(&mut stuck, deadline).unwrap());
    assert!(started.elapsed() < Duration::from_secs(2));
    stuck.kill().unwrap();
    stuck.wait().unwrap();

    let mut quick = Command::new("true").spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    assert!(reaped_by(&mut quick, deadline).unwrap());
}

#[test]
fn an_immediate_background_start_is_ready_once_the_service_runs() {
    let fixture = Fixture::new();
    let mut spec = resolve("udevd", &Service::longrun(&["/bin/true"]));
    spec.early = true;
    fixture.install(&spec);
    let mut daemon = Command::new("/bin/sh");
    daemon.args(["-c", "exec sleep 100 0</dev/null"]);
    let started = std::time::Instant::now();
    fixture
        .launcher
        .start_detached(&spec, daemon)
        .expect("never fails");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(fixture.failures(), "");
    assert!(fixture.ready("udevd"));
    early_udevd(&fixture);
    fixture.launcher.stop_early().expect("stops");
}

#[test]
fn an_immediate_background_start_whose_exec_fails_is_not_ready() {
    let fixture = Fixture::new();
    let mut spec = resolve("udevd", &Service::longrun(&["/bin/true"]));
    spec.early = true;
    fixture.install(&spec);
    let mut failing = Command::new("/bin/sh");
    failing.args(["-c", "printf '!' >&0; exec sleep 100 0</dev/null"]);
    let started = std::time::Instant::now();
    fixture
        .launcher
        .start_detached(&spec, failing)
        .expect("never fails");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(fixture.failures(), "udevd failed\n");
    assert!(!fixture.ready("udevd"));
    early_udevd(&fixture);
    fixture.launcher.stop_early().expect("stops");
}

#[test]
fn a_ready_background_start_records_its_pid_and_start_time() {
    let fixture = Fixture::new();
    let mut spec = resolve("udevd", &Service::longrun(&["/bin/true"]));
    spec.early = true;
    fixture.install(&spec);
    let marker = fixture.path("run/runit/ready/udevd");
    let mut daemon = Command::new("/bin/sh");
    let script = format!(
        "mkdir -p {} && touch {}; exec sleep 100",
        marker.parent().expect("has a parent").display(),
        marker.display()
    );
    daemon.args(["-c", &script]);
    fixture
        .launcher
        .start_detached(&spec, daemon)
        .expect("never fails");
    assert_eq!(fixture.failures(), "");
    let text = read(&fixture.path("run/runit/early/udevd.pid"));
    let (pid, started) = text.trim().split_once(' ').expect("pid and start time");
    let pid: u32 = pid.parse().expect("a pid");
    assert_eq!(start_time(pid), started.parse().ok());
    early_udevd(&fixture);
    assert_eq!(fixture.launcher.stop_early().expect("stops"), 0);
}

#[test]
fn privileges_are_dropped_for_the_command() {
    if !rustix::process::geteuid().is_root() {
        println!("skipping: dropping privileges needs root");
        return;
    }
    let credentials = Credentials {
        user: "nobody".into(),
        uid: 65534,
        gid: 65534,
        groups: vec![65534],
        home: "/var/empty".into(),
        shell: "/usr/bin/nologin".into(),
        declares_groups: false,
    };
    let mut id = Command::new("/usr/bin/id");
    id.arg("-u").stdout(std::process::Stdio::piped());
    let output = spawn_as("nobody", &credentials, id)
        .expect("spawns")
        .wait_with_output()
        .expect("waits");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "65534\n");
    assert!(
        rustix::process::geteuid().is_root(),
        "the launcher stays root"
    );
    std::thread::spawn(move || {
        drop_privileges("nobody", &credentials).expect("drops");
        assert_eq!(rustix::process::geteuid().as_raw(), 65534);
    })
    .join()
    .expect("the thread finishes");
}

#[test]
fn a_root_service_gets_its_declared_groups() {
    let credentials = Credentials {
        user: "root".into(),
        uid: 0,
        gid: 65534,
        groups: vec![65534],
        home: "/root".into(),
        shell: "/bin/sh".into(),
        declares_groups: true,
    };
    let result = std::thread::spawn(move || {
        drop_privileges("job", &credentials).map(|()| {
            (
                rustix::process::geteuid().as_raw(),
                rustix::process::getegid().as_raw(),
            )
        })
    })
    .join()
    .expect("the thread finishes");
    if rustix::process::geteuid().is_root() {
        assert_eq!(result.expect("drops"), (0, 65534));
    } else {
        let error = result.expect_err("changing groups needs root");
        assert!(error.to_string().contains("set the groups"), "{error}");
    }
}

/// The root of the fixture that [`exec_in_a_child_process`] works in, set
/// only in the environment of the child test process.
const EXEC_ROOT: &str = "LAFT_SVC_TEST_EXEC_ROOT";

/// Runs `Launcher::exec` for the longrun `daemon`, which replaces this
/// process on success. A failure exits with the launcher's status.
#[test]
#[ignore = "run by launcher_exec_starts_a_longrun in a child process"]
fn exec_in_a_child_process() {
    let Some(root) = std::env::var_os(EXEC_ROOT) else {
        return;
    };
    let error = Launcher::new(root)
        .exec("daemon", ExecMode::Supervised)
        .expect_err("exec only returns on failure");
    eprintln!("laft svc: {error}");
    std::process::exit(crate::SETUP_FAILED);
}

/// Runs [`exec_in_a_child_process`] for the fixture below `root`.
fn exec_daemon(root: &Path) -> std::process::Output {
    Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            "--exact",
            "launcher::tests::exec_in_a_child_process",
            "--ignored",
            "--nocapture",
        ])
        .env(EXEC_ROOT, root)
        .output()
        .expect("runs")
}

#[test]
fn launcher_exec_starts_a_longrun() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let as_root = rustix::process::geteuid().is_root();
    let user = if as_root {
        fs::write(
            fixture.path("etc/passwd"),
            "root:x:0:0:root:/root:/bin/sh\nnobody:x:65534:65534::/var/empty:/bin/sh\n",
        )
        .expect("write");
        fs::write(fixture.path("etc/group"), "root:x:0:\nnobody:x:65534:\n").expect("write");
        "nobody"
    } else {
        println!("skipping the privilege drop: it needs root");
        "root"
    };
    let work = fixture.path("work");
    fs::create_dir(&work).expect("mkdir");
    for dir in [fixture.dir.path(), work.as_path()] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o777)).expect("chmod");
    }
    let out = work.join("out");
    let script = format!(
        "fd3=closed; [ -w /proc/self/fd/3 ] && fd3=open; \
         echo \"$(id -u) $(ulimit -n) $(umask) $(pwd) $HOME $fd3\" > {}",
        out.display()
    );
    let mut spec = resolve("daemon", &Service::longrun(&["/bin/sh", "-c", &script]));
    spec.user = user.into();
    spec.after = vec!["dbus".into()];
    spec.cwd = work.display().to_string();
    spec.umask = 0o027;
    spec.limits.nofile = Some(256);
    spec.ready = crate::SpecReady::Notify { fd: 3 };
    spec.state_dirs = vec![SpecDir {
        path: "/var/lib/daemon".into(),
        mode: 0o750,
        owner: (!as_root).then(|| "me".into()),
        group: None,
    }];
    fixture.install(&spec);

    let gated = exec_daemon(fixture.dir.path());
    assert_eq!(gated.status.code(), Some(crate::SETUP_FAILED));
    assert!(
        String::from_utf8_lossy(&gated.stderr).contains("daemon waiting for dbus"),
        "{}",
        String::from_utf8_lossy(&gated.stderr)
    );
    assert!(!out.exists() && !fixture.path("var/lib/daemon").exists());

    fixture.mark("dbus");
    let started = exec_daemon(fixture.dir.path());
    assert_eq!(
        started.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let (uid, home) = if as_root {
        (65534, "/var/empty")
    } else {
        (rustix::process::getuid().as_raw(), "/root")
    };
    assert_eq!(
        read(&out),
        format!("{uid} 256 0027 {} {home} open\n", work.display())
    );
    let state = fs::metadata(fixture.path("var/lib/daemon")).expect("created");
    assert_eq!(state.mode() & 0o7777, 0o750);
    assert_eq!(
        state.uid(),
        if as_root {
            uid
        } else {
            rustix::process::getuid().as_raw()
        }
    );
}
