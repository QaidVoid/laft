//! The service launcher: everything that happens between the supervisor
//! deciding to start a service and the service's own program running.
//!
//! The rendered runit scripts call the `laft` binary's internal `svc`
//! subcommands, which map one to one onto the methods of [`Launcher`]. Each
//! reads the service's spec from the active generation, so the scripts
//! never change when only the spec does.
//!
//! Some steps need a separate process running launcher code: the readiness
//! watcher and poller, and the detached half of an early longrun. The
//! launcher starts them by executing the running program again, as
//!
//! - `svc ready --backend runit --parent <pid> <name>`, which the binary
//!   routes to [`Launcher::ready`], and
//! - `svc exec --backend runit --detached <name>`, routed to
//!   [`Launcher::exec`] with [`ExecMode::Detached`].

mod accounts;
mod process;

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::io::Errno;
use rustix::pipe::PipeFlags;
use rustix::process::{Pid, PidfdFlags, Signal};

use self::accounts::{Accounts, Credentials};
use self::process::{
    apply_limits, command, drop_privileges, ensure_dir, place_fd, restore_limits, spawn_as,
    write_atomic,
};
use crate::error::{Error, corrupt, io};
use crate::spec::{Spec, SpecLog, SpecReady};
use crate::standard::{ACTIVATE, EARLY};
use crate::{Restart, ServiceKind};

/// The program that executes the launcher again for its helper processes.
const SELF_EXE: &str = "/proc/self/exe";

/// The exit status of a non-early oneshot whose `up` command failed, which
/// tells `finish` not to restart it. The `run` script of an emulated
/// oneshot never exits with the status of `up` itself, so this cannot
/// collide with it.
const ONESHOT_FAILED: i32 = 100;

/// The exit status of every launcher failure, which makes the supervisor
/// start the service again later unless its restart policy is
/// [`Restart::Never`].
///
/// The status is reserved: a supervised program that exits with it is
/// restarted as if the launcher had failed.
pub const SETUP_FAILED: i32 = 111;

/// How long to wait between two looks at a ready marker or a process.
const POLL: Duration = Duration::from_millis(50);

/// What a detached start writes to its status pipe when it will not
/// execute the service.
const FAILED: &[u8] = b"!";

/// How `laft svc exec` was invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// From a service directory's `run` script, under supervision.
    Supervised,
    /// From stage 1 with `--background`: start the service detached, wait
    /// until it is ready or its readiness times out, and return.
    Background,
    /// The detached process that [`ExecMode::Background`] starts. It leaves
    /// the session of stage 1 and skips the dependency gate, because stage 1
    /// already ran the dependencies.
    Detached,
}

/// The launcher, working on the system below a root directory.
///
/// The root is `/` on a running system and a temporary directory in tests.
/// It prefixes every file the launcher reads or manages: specs, ready
/// markers, account databases, the boot failure record and the declared
/// directories. Commands, working directories and the log directory handed
/// to the logger are used as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    root: PathBuf,
}

impl Default for Launcher {
    /// The launcher of the running system, with root `/`.
    fn default() -> Self {
        Self::new("/")
    }
}

impl Launcher {
    /// The launcher for the system below `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Starts a longrun, or emulates a oneshot outside the early boot graph
    /// as a supervised process.
    ///
    /// For a longrun it checks that every dependency is ready, resolves the
    /// accounts, creates the declared directories, applies the limits and
    /// the umask, sets up readiness, drops privileges, builds the
    /// environment and executes the command. On success it does not return.
    ///
    /// A oneshot runs `up`, marks itself ready and then waits until it is
    /// stopped, which [`Self::finish`] completes by running `down`. When
    /// `up` fails, it returns status 100, which tells [`Self::finish`] not
    /// to restart it.
    ///
    /// # Errors
    ///
    /// Fails when the spec is missing or invalid, a dependency is not
    /// ready, or any setup step fails. The service did not start, and the
    /// caller exits with [`SETUP_FAILED`].
    pub fn exec(&self, name: &str, mode: ExecMode) -> Result<i32, Error> {
        if mode != ExecMode::Detached {
            return self.exec_spec(name, mode);
        }
        // Standard input of the detached process is the status pipe of
        // `start_detached`. It moves to a close-on-exec descriptor, so it
        // closes exactly when the service is executed, and a byte on it
        // reports that this never happened.
        let status = match rustix::io::fcntl_dupfd_cloexec(rustix::stdio::stdin(), 3) {
            Ok(status) => File::from(status),
            Err(e) => {
                let _ = rustix::io::write(rustix::stdio::stdin(), FAILED);
                return Err(setup_error(name, "keep the status pipe", e));
            }
        };
        let result = File::open("/dev/null")
            .and_then(|null| Ok(rustix::stdio::dup2_stdin(&null)?))
            .map_err(|e| Error::Setup {
                name: name.to_owned(),
                step: "replace standard input",
                source: e,
            })
            .and_then(|()| self.exec_spec(name, mode));
        let _ = (&status).write_all(FAILED);
        result
    }

    fn exec_spec(&self, name: &str, mode: ExecMode) -> Result<i32, Error> {
        let spec = self.load(name)?;
        if mode == ExecMode::Background {
            return self.exec_background(&spec);
        }
        if spec.kind == ServiceKind::Oneshot && !spec.early {
            return self.emulate_oneshot(&spec);
        }
        require(&spec, ServiceKind::Longrun)?;
        if mode == ExecMode::Detached {
            rustix::process::setsid().map_err(|e| Error::Setup {
                name: spec.name.clone(),
                step: "start a session",
                source: e.into(),
            })?;
        } else {
            self.gate(&spec)?;
        }
        let argv = self.argv(&spec, &spec.exec)?;
        let accounts = Accounts::read(&self.root)?;
        let credentials = accounts.credentials(&spec)?;
        self.prepare_dirs(&spec, &accounts, &credentials)?;
        apply_limits(&spec)?;
        rustix::process::umask(rustix::fs::Mode::from_raw_mode(spec.umask));
        let _notify = self.readiness(&spec, mode)?;
        drop_privileges(&spec.name, &credentials)?;
        let source = command(&spec, &credentials, argv).exec();
        Err(Error::Exec {
            name: spec.name.clone(),
            program: argv[0].clone(),
            source,
        })
    }

    /// Runs a oneshot's `up` command and returns its exit status, or 128
    /// plus the signal that killed it.
    ///
    /// A oneshot of the early boot graph never fails instead: a failure is
    /// appended to the boot failure record and announced on the console,
    /// a dependency that failed or was skipped makes it skip itself, and
    /// it always returns 0. Only a failed activation does not make its
    /// dependents skip. Either kind marks itself ready when `up` succeeds.
    ///
    /// The oneshot [`EARLY`], which ends the early boot graph and which
    /// every normal service depends on, is the exception: it never skips
    /// and always becomes ready, so that a failure in the early boot graph
    /// is recorded but never keeps the normal services from starting.
    ///
    /// # Errors
    ///
    /// Fails when the spec is missing or invalid, a dependency of a normal
    /// oneshot is not ready, or a setup step fails.
    pub fn up(&self, name: &str) -> Result<i32, Error> {
        let spec = self.load(name)?;
        require(&spec, ServiceKind::Oneshot)?;
        if !spec.early {
            self.gate(&spec)?;
            let code = self.run(&spec, &spec.up)?;
            if code == 0 {
                self.mark_ready(name)?;
            }
            return Ok(code);
        }
        let sync_point = name == EARLY;
        if !sync_point && self.failed_dependency(&spec)?.is_some() {
            self.record_failure(name, "skipped")?;
            return Ok(0);
        }
        match self.run(&spec, &spec.up) {
            Ok(0) => self.mark_ready(name)?,
            outcome => {
                if let Err(e) = outcome {
                    eprintln!("laft svc: {e}");
                }
                self.record_failure(name, "failed")?;
                self.announce_failure(name);
                if sync_point {
                    self.mark_ready(name)?;
                }
            }
        }
        Ok(0)
    }

    /// Runs a oneshot's `down` command, if it has one, removes its ready
    /// marker and returns the command's exit status.
    ///
    /// # Errors
    ///
    /// Fails when the spec is missing or invalid, or a setup step fails.
    pub fn down(&self, name: &str) -> Result<i32, Error> {
        let spec = self.load(name)?;
        require(&spec, ServiceKind::Oneshot)?;
        let code = if spec.down.is_empty() {
            0
        } else {
            self.run(&spec, &spec.down)?
        };
        self.clear_ready(name)?;
        Ok(code)
    }

    /// Handles the end of a supervised service, called with the two
    /// arguments runit passes to `finish`: the exit code of the `run`
    /// script, or `-1` when it did not exit normally, and the low byte of
    /// its wait status, which is the signal that killed it in that case.
    ///
    /// A oneshot that was up runs its `down` command. The ready marker is
    /// removed. When the restart policy says no, the supervisor is told to
    /// keep the service down through the control pipe of the service's
    /// directory in the scan directory, `/run/runit/service/<name>`, which
    /// is also the working directory runit gives `finish`. Otherwise the
    /// restart delay passes before this returns.
    ///
    /// # Errors
    ///
    /// Fails when the spec is missing or invalid, or the marker or the
    /// supervisor's control pipe cannot be written.
    pub fn finish(&self, name: &str, exit_code: i32, exit_signal: i32) -> Result<i32, Error> {
        let spec = self.load(name)?;
        if exit_code == -1 {
            eprintln!("laft svc: {name}: killed by signal {exit_signal}");
        }
        let was_up = self.ready_marker(name).exists();
        if spec.kind == ServiceKind::Oneshot && was_up && !spec.down.is_empty() {
            match self.run(&spec, &spec.down) {
                Ok(0) => {}
                Ok(code) => eprintln!("laft svc: {name}: down exited with status {code}"),
                Err(e) => eprintln!("laft svc: {e}"),
            }
        }
        self.clear_ready(name)?;
        if !restarts(&spec, exit_code) {
            let control = self.service_dir(name).join("supervise/control");
            OpenOptions::new()
                .write(true)
                .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
                .open(&control)
                .and_then(|mut c| c.write_all(b"d"))
                .map_err(io(&control))?;
            return Ok(0);
        }
        std::thread::sleep(Duration::from_millis(spec.restart_delay_ms));
        Ok(0)
    }

    /// Starts the logger of a logged service: creates `/var/log/<name>`
    /// owned by the static `log` account with mode `0o750`, writes the
    /// rotation settings to its `config` file, drops to `log` and executes
    /// `svlogd`. On success it does not return.
    ///
    /// # Errors
    ///
    /// Fails when the spec is missing or invalid, the service is not
    /// logged, or a setup step fails.
    pub fn logger(&self, name: &str) -> Result<i32, Error> {
        let spec = self.load(name)?;
        let SpecLog::Logged { dir, size, count } = &spec.log else {
            return Err(Error::NotLogged(spec.name.clone()));
        };
        let unknown = |kind| Error::UnknownAccount {
            name: spec.name.clone(),
            kind,
            account: "log".into(),
        };
        let uid = laft_core::system_uid("log").ok_or_else(|| unknown("user"))?;
        let gid = laft_core::system_gid("log").ok_or_else(|| unknown("group"))?;
        self.prepare_log(name, *size, *count, uid, gid)?;
        let credentials = Credentials {
            user: "log".into(),
            uid,
            gid,
            groups: vec![gid],
            home: "/var/empty".into(),
            shell: "/usr/bin/nologin".into(),
            declares_groups: false,
        };
        drop_privileges(&spec.name, &credentials)?;
        let source = Command::new("/usr/bin/svlogd")
            .args(["-tt", dir])
            .env_clear()
            .env("PATH", "/usr/bin")
            .exec();
        Err(Error::Exec {
            name: spec.name.clone(),
            program: "/usr/bin/svlogd".into(),
            source,
        })
    }

    /// Stops the early longruns that stage 1 started without supervision,
    /// as listed in the booted generation: each gets its stop signal, and
    /// `SIGKILL` when it outlives its stop timeout.
    ///
    /// A process is signaled only when its pid file names a process that
    /// still has the start time recorded with it, so a pid that a crashed
    /// service left behind and the kernel gave to another process is never
    /// signaled. Every pid file is removed.
    ///
    /// # Errors
    ///
    /// Fails when the list or a pid file cannot be read.
    pub fn stop_early(&self) -> Result<i32, Error> {
        let list = self
            .root
            .join("run/laft/booted/services/runit/early-longruns");
        let names = match fs::read_to_string(&list) {
            Ok(names) => names,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(io(&list)(e)),
        };
        for name in names.lines().filter(|n| !n.is_empty()) {
            let pid_file = self.early_pid_file(name);
            let text = match fs::read_to_string(&pid_file) {
                Ok(text) => text,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io(&pid_file)(e)),
            };
            let (signal, timeout) = match self.load(name) {
                Ok(spec) => (spec.stop_signal, spec.stop_timeout_ms),
                Err(_) => (Signal::TERM.as_raw(), 10_000),
            };
            match same_process(&text) {
                Some(pidfd) => stop_process(&pidfd, signal, timeout),
                None => eprintln!("laft svc: {name}: the recorded process is gone"),
            }
            let _ = fs::remove_file(&pid_file);
        }
        Ok(0)
    }

    /// Runs the readiness helper of a service, in the process the launcher
    /// starts for it before executing the service. `parent` is the process
    /// that becomes the service. The helper ends when that process does.
    ///
    /// For `notify`, standard input is the read end of the service's
    /// readiness pipe, and the service is ready when a newline arrives. For
    /// `check`, the check command runs as the service user at every
    /// interval until it succeeds or the timeout passes. Either way, the
    /// ready marker is created when the service is ready.
    ///
    /// # Errors
    ///
    /// Fails when the spec is missing or invalid, or the marker cannot be
    /// created.
    pub fn ready(&self, name: &str, parent: u32) -> Result<i32, Error> {
        let setup = |e: Errno| Error::Setup {
            name: name.to_owned(),
            step: "follow the service",
            source: e.into(),
        };
        rustix::process::set_parent_process_death_signal(Some(Signal::TERM)).map_err(setup)?;
        let parent_alive = rustix::process::getppid()
            .is_some_and(|p| u32::try_from(p.as_raw_nonzero().get()) == Ok(parent));
        if !parent_alive {
            return Ok(0);
        }
        let spec = self.load(name)?;
        match &spec.ready {
            SpecReady::Immediate => {}
            SpecReady::Notify { .. } => {
                self.watch_notify(name, io::stdin().lock())?;
            }
            SpecReady::Check {
                cmd,
                interval_ms,
                timeout_ms,
            } => {
                let accounts = Accounts::read(&self.root)?;
                let credentials = accounts.credentials(&spec)?;
                self.poll_check(&spec, &credentials, cmd, *interval_ms, *timeout_ms)?;
            }
        }
        Ok(0)
    }

    fn exec_background(&self, spec: &Spec) -> Result<i32, Error> {
        require(spec, ServiceKind::Longrun)?;
        if self.failed_dependency(spec)?.is_some() {
            self.record_failure(&spec.name, "skipped")?;
            return Ok(0);
        }
        let mut detached = Command::new(SELF_EXE);
        detached
            .args([
                "svc",
                "exec",
                "--backend",
                "runit",
                "--detached",
                &spec.name,
            ])
            .env_clear()
            .env("PATH", "/usr/bin");
        self.start_detached(spec, detached)
    }

    /// Starts `detached`, the process that becomes the early longrun
    /// `spec`, records its pid and start time, and waits until the service
    /// is ready, exits or its ready timeout passes. Anything but readiness
    /// is recorded and announced as a failure, and a pid file whose
    /// process exited is removed. A process still running after the
    /// timeout keeps its pid file, so that it is stopped at shutdown.
    ///
    /// Standard input of `detached` is the write end of a status pipe. The
    /// pipe closes without data when the service is executed, and receives
    /// a byte when that fails. An immediately ready service is marked
    /// ready here once the pipe closed and the process still runs.
    fn start_detached(&self, spec: &Spec, mut detached: Command) -> Result<i32, Error> {
        let name = &spec.name;
        let (reader, writer) = rustix::pipe::pipe_with(PipeFlags::CLOEXEC)
            .map_err(|e| setup_error(name, "create the status pipe", e))?;
        let spawned = detached.stdin(Stdio::from(writer)).spawn();
        drop(detached);
        let mut child = match spawned {
            Ok(child) => child,
            Err(source) => {
                eprintln!("laft svc: service {name}: cannot start: {source}");
                self.record_failure(name, "failed")?;
                self.announce_failure(name);
                return Ok(0);
            }
        };
        let wait_error = |source| Error::Setup {
            name: name.clone(),
            step: "wait for the service",
            source,
        };
        let pid_file = self.early_pid_file(name);
        create_parent(&pid_file)?;
        let started = start_time(child.id()).ok_or_else(|| Error::Setup {
            name: name.clone(),
            step: "read the start time of the service",
            source: io::ErrorKind::NotFound.into(),
        })?;
        fs::write(&pid_file, format!("{} {started}\n", child.id())).map_err(io(&pid_file))?;
        let deadline = Instant::now() + Duration::from_millis(spec.ready_timeout_ms());
        let mut status = Some(reader);
        let mut exited = false;
        while Instant::now() < deadline {
            let executed = match &status {
                Some(reader) => match exec_status(reader) {
                    ExecStatus::Pending => false,
                    ExecStatus::Executed if exiting(child.id()) => {
                        // The pipe also closes when the process exits
                        // without executing the service, and that happens
                        // before the process can be waited for. An exit
                        // can stall, for example on a hung mount, so the
                        // wait ends at the deadline.
                        exited = reaped_by(&mut child, deadline).map_err(wait_error)?;
                        break;
                    }
                    ExecStatus::Executed => {
                        status = None;
                        true
                    }
                    ExecStatus::Failed => break,
                },
                None => {
                    std::thread::sleep(POLL);
                    true
                }
            };
            if child.try_wait().map_err(wait_error)?.is_some() {
                exited = true;
                break;
            }
            if executed && spec.ready == SpecReady::Immediate {
                self.mark_ready(name)?;
            }
            if self.ready_marker(name).exists() {
                return Ok(0);
            }
        }
        if exited {
            let _ = fs::remove_file(&pid_file);
        }
        self.record_failure(name, "failed")?;
        self.announce_failure(name);
        Ok(0)
    }

    fn emulate_oneshot(&self, spec: &Spec) -> Result<i32, Error> {
        self.gate(spec)?;
        if self.run(spec, &spec.up)? != 0 {
            return Ok(ONESHOT_FAILED);
        }
        self.mark_ready(&spec.name)?;
        // Stopping the service kills this process with the supervisor's
        // signal, and `finish` then runs `down`.
        loop {
            std::thread::park();
        }
    }

    /// Runs `argv` for the service in a child process with the service's
    /// accounts, directories, limits, umask, environment and working
    /// directory, and returns its exit status, or 128 plus the signal that
    /// killed it. The limits and the umask are set for the start only, and
    /// the launcher's own are restored right after it.
    fn run(&self, spec: &Spec, argv: &[String]) -> Result<i32, Error> {
        let argv = self.argv(spec, argv)?;
        let accounts = Accounts::read(&self.root)?;
        let credentials = accounts.credentials(spec)?;
        self.prepare_dirs(spec, &accounts, &credentials)?;
        let limits = apply_limits(spec)?;
        let umask = rustix::process::umask(rustix::fs::Mode::from_raw_mode(spec.umask));
        let child = spawn_as(&spec.name, &credentials, command(spec, &credentials, argv));
        rustix::process::umask(umask);
        let restored = restore_limits(&spec.name, limits);
        let status = child?.wait().map_err(|source| Error::Exec {
            name: spec.name.clone(),
            program: argv[0].clone(),
            source,
        })?;
        restored?;
        Ok(exit_code(status))
    }

    fn prepare_dirs(
        &self,
        spec: &Spec,
        accounts: &Accounts,
        credentials: &Credentials,
    ) -> Result<(), Error> {
        for (base, dirs) in [("/var/lib", &spec.state_dirs), ("/run", &spec.runtime_dirs)] {
            for dir in dirs {
                let relative = dir
                    .path
                    .strip_prefix(base)
                    .and_then(|p| p.strip_prefix('/'))
                    .ok_or_else(|| {
                        corrupt(
                            &self.spec_path(&spec.name),
                            format!("{} is not below {base}", dir.path),
                        )
                    })?;
                let (uid, gid) = accounts.dir_owner(spec, credentials, dir)?;
                ensure_dir(&self.root, base, relative, uid, gid, dir.mode)?;
            }
        }
        Ok(())
    }

    fn prepare_log(
        &self,
        name: &str,
        size: u64,
        count: u32,
        uid: u32,
        gid: u32,
    ) -> Result<(), Error> {
        ensure_dir(&self.root, "/var/log", name, uid, gid, 0o750)?;
        let config = self.root.join("var/log").join(name).join("config");
        write_atomic(&config, format!("s{size}\nn{count}\n").as_bytes(), 0o644)
    }

    /// Sets up readiness before the privilege drop, and returns the write
    /// end of a notification pipe, which must stay open until `execve`.
    ///
    /// An immediately ready service is marked ready here, right before
    /// `execve`, unless it runs detached. Then the process that started it
    /// marks it once the `execve` succeeded.
    fn readiness(&self, spec: &Spec, mode: ExecMode) -> Result<Option<OwnedFd>, Error> {
        let name = &spec.name;
        let helper = |stdin: Stdio| {
            Command::new(SELF_EXE)
                .args(["svc", "ready", "--backend", "runit", "--parent"])
                .arg(std::process::id().to_string())
                .arg(name)
                .stdin(stdin)
                .env_clear()
                .env("PATH", "/usr/bin")
                .spawn()
                .map_err(|source| Error::Exec {
                    name: name.clone(),
                    program: SELF_EXE.into(),
                    source,
                })
        };
        match spec.ready {
            SpecReady::Immediate => {
                if mode != ExecMode::Detached {
                    self.mark_ready(name)?;
                }
                Ok(None)
            }
            SpecReady::Notify { fd } => {
                let (reader, writer) =
                    rustix::pipe::pipe_with(PipeFlags::CLOEXEC).map_err(|e| Error::Setup {
                        name: name.clone(),
                        step: "create the readiness pipe",
                        source: e.into(),
                    })?;
                helper(Stdio::from(reader))?;
                place_fd(name, &writer, fd).map(Some)
            }
            SpecReady::Check { .. } => {
                helper(Stdio::null())?;
                Ok(None)
            }
        }
    }

    /// Creates the ready marker once a newline arrives on `reader`. Returns
    /// whether it did, which it does not when the pipe closes first.
    fn watch_notify(&self, name: &str, reader: impl Read) -> Result<bool, Error> {
        let mut line = Vec::new();
        let read = BufReader::new(reader).read_until(b'\n', &mut line);
        if read.is_ok() && line.last() == Some(&b'\n') {
            self.mark_ready(name)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Runs the check command until it succeeds, then creates the ready
    /// marker. Returns whether it did before the timeout. An attempt that
    /// is still running at the timeout is killed and counts as failed.
    fn poll_check(
        &self,
        spec: &Spec,
        credentials: &Credentials,
        cmd: &[String],
        interval_ms: u64,
        timeout_ms: u64,
    ) -> Result<bool, Error> {
        let argv = self.argv(spec, cmd)?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let mut check = command(spec, credentials, argv);
            check.stdin(Stdio::null()).stdout(Stdio::null());
            let passed = spawn_as(&spec.name, credentials, check)
                .is_ok_and(|child| wait_until(child, deadline).is_some_and(|s| s.success()));
            if passed {
                self.mark_ready(&spec.name)?;
                return Ok(true);
            }
            let interval = Duration::from_millis(interval_ms);
            if Instant::now() + interval > deadline {
                return Ok(false);
            }
            std::thread::sleep(interval);
        }
    }

    /// Fails with the first dependency that has no ready marker.
    fn gate(&self, spec: &Spec) -> Result<(), Error> {
        match spec.after.iter().find(|d| !self.ready_marker(d).exists()) {
            Some(dep) => Err(Error::Waiting {
                name: spec.name.clone(),
                dep: dep.clone(),
            }),
            None => Ok(()),
        }
    }

    /// The first dependency recorded as failed or skipped in this boot. A
    /// failed activation does not count, because a stale `/etc` is better
    /// than a machine without its filesystems.
    fn failed_dependency(&self, spec: &Spec) -> Result<Option<String>, Error> {
        let path = self.boot_failures();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(&path)(e)),
        };
        let failed: Vec<&str> = text
            .lines()
            .filter_map(|l| {
                l.strip_suffix(" failed")
                    .or_else(|| l.strip_suffix(" skipped"))
            })
            .collect();
        Ok(spec
            .after
            .iter()
            .find(|d| d.as_str() != ACTIVATE && failed.contains(&d.as_str()))
            .cloned())
    }

    fn record_failure(&self, name: &str, outcome: &str) -> Result<(), Error> {
        let path = self.boot_failures();
        create_parent(&path)?;
        OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o644)
            .open(&path)
            .and_then(|mut f| f.write_all(format!("{name} {outcome}\n").as_bytes()))
            .map_err(io(&path))
    }

    /// Tells the console about a failed early service. The console may be
    /// missing, and then nobody can be told.
    fn announce_failure(&self, name: &str) {
        let console = self.root.join("dev/console");
        if let Ok(mut console) = OpenOptions::new().write(true).open(console) {
            let _ = writeln!(console, "laft: early service {name} failed");
        }
    }

    fn load(&self, name: &str) -> Result<Spec, Error> {
        let path = self.spec_path(name);
        let spec = match fs::read(&path) {
            Ok(bytes) => Spec::parse(&bytes, &path)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::NoSpec(name.to_owned()));
            }
            Err(e) => return Err(io(&path)(e)),
        };
        if spec.name != name {
            return Err(corrupt(&path, "the spec names a different service"));
        }
        Ok(spec)
    }

    fn mark_ready(&self, name: &str) -> Result<(), Error> {
        let marker = self.ready_marker(name);
        create_parent(&marker)?;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&marker)
            .map(drop)
            .map_err(io(&marker))
    }

    fn clear_ready(&self, name: &str) -> Result<(), Error> {
        let marker = self.ready_marker(name);
        match fs::remove_file(&marker) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io(&marker)(e)),
            _ => Ok(()),
        }
    }

    /// `argv`, which must name a program.
    fn argv<'a>(&self, spec: &Spec, argv: &'a [String]) -> Result<&'a [String], Error> {
        if argv.is_empty() {
            return Err(corrupt(&self.spec_path(&spec.name), "a command is empty"));
        }
        Ok(argv)
    }

    fn spec_path(&self, name: &str) -> PathBuf {
        self.root
            .join("run/laft/system/services/spec")
            .join(format!("{name}.json"))
    }

    /// The service's directory in the supervisor's scan directory.
    fn service_dir(&self, name: &str) -> PathBuf {
        self.root.join("run/runit/service").join(name)
    }

    fn ready_marker(&self, name: &str) -> PathBuf {
        self.root.join("run/runit/ready").join(name)
    }

    fn early_pid_file(&self, name: &str) -> PathBuf {
        self.root
            .join("run/runit/early")
            .join(format!("{name}.pid"))
    }

    fn boot_failures(&self) -> PathBuf {
        self.root.join("run/laft/boot-failures")
    }
}

/// Whether the supervisor restarts a service that exited with `exit_code`,
/// which is `-1` when it was killed by a signal. The policy `never` wins
/// over every reserved status.
fn restarts(spec: &Spec, exit_code: i32) -> bool {
    match (spec.restart, exit_code) {
        (Restart::Never, _) => false,
        (_, SETUP_FAILED) => true,
        (_, ONESHOT_FAILED) if spec.kind == ServiceKind::Oneshot => false,
        (Restart::Always, _) => true,
        (Restart::OnFailure, code) => code != 0,
    }
}

fn require(spec: &Spec, kind: ServiceKind) -> Result<(), Error> {
    if spec.kind == kind {
        Ok(())
    } else {
        Err(Error::WrongKind {
            name: spec.name.clone(),
            expected: kind,
        })
    }
}

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(SETUP_FAILED)
}

fn create_parent(path: &Path) -> Result<(), Error> {
    match path.parent() {
        Some(dir) => fs::create_dir_all(dir).map_err(io(dir)),
        None => Ok(()),
    }
}

/// Waits for `child` until `deadline`, then kills it. Returns how it
/// exited, or `None` when it was killed or cannot be waited for.
fn wait_until(mut child: std::process::Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(left.min(POLL));
    }
}

/// Waits for `child` to exit until `deadline` without killing it, and
/// tells whether it exited.
fn reaped_by(child: &mut std::process::Child, deadline: Instant) -> io::Result<bool> {
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        std::thread::sleep(left.min(POLL));
    }
}

/// What the status pipe of a detached start says.
enum ExecStatus {
    /// Nothing yet.
    Pending,
    /// The pipe closed without data: the service was executed.
    Executed,
    /// A byte arrived: the service will not be executed.
    Failed,
}

/// Waits up to [`POLL`] for news on the status pipe `reader`.
fn exec_status(reader: &OwnedFd) -> ExecStatus {
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: POLL.as_nanos().try_into().unwrap_or(0),
    };
    let mut fds = [PollFd::new(reader, PollFlags::IN)];
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(n) if n > 0 => {}
        _ => return ExecStatus::Pending,
    }
    let mut byte = [0];
    match rustix::io::read(reader, &mut byte) {
        Ok(0) => ExecStatus::Executed,
        Ok(_) => ExecStatus::Failed,
        Err(Errno::INTR | Errno::AGAIN) => ExecStatus::Pending,
        Err(_) => ExecStatus::Failed,
    }
}

fn setup_error(name: &str, step: &'static str, errno: Errno) -> Error {
    Error::Setup {
        name: name.to_owned(),
        step,
        source: errno.into(),
    }
}

/// The start time of process `pid` in clock ticks after boot, from
/// `/proc/<pid>/stat`, or `None` when the process does not exist.
fn start_time(pid: u32) -> Option<u64> {
    parse_start_time(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// Whether process `pid` is exiting or gone, from `/proc/<pid>/stat`.
fn exiting(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| parse_exiting(&stat))
}

/// Whether a `/proc/<pid>/stat` line describes a process that is exiting:
/// a zombie, a dead process or one with `PF_EXITING` set. The kernel sets
/// that flag before it closes the files of an exiting process, so a pipe
/// that closed because the process exits always finds it set. A line that
/// cannot be read counts as exiting.
fn parse_exiting(stat: &str) -> bool {
    /// The task flag of a process that started to exit.
    const PF_EXITING: u32 = 0x4;
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return true;
    };
    let mut fields = rest.split_whitespace();
    let state = fields.next();
    let flags = fields.nth(5).and_then(|f| f.parse::<u32>().ok());
    match (state, flags) {
        (Some("Z" | "X" | "x"), _) | (_, None) => true,
        (_, Some(flags)) => flags & PF_EXITING != 0,
    }
}

/// The start time field of a `/proc/<pid>/stat` line. The command name in
/// parentheses may contain spaces and parentheses itself, so fields are
/// counted after its last `)`, where the third field begins.
fn parse_start_time(stat: &str) -> Option<u64> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// A descriptor for the process a pid file names, when that process still
/// has the recorded start time. The descriptor is taken before the start
/// time is compared, so it cannot refer to a process that took the pid
/// later.
fn same_process(pid_file: &str) -> Option<OwnedFd> {
    let (pid, started) = pid_file.trim().split_once(' ')?;
    let (pid, started): (u32, u64) = (pid.parse().ok()?, started.parse().ok()?);
    let raw = Pid::from_raw(i32::try_from(pid).ok()?)?;
    let pidfd = rustix::process::pidfd_open(raw, PidfdFlags::empty()).ok()?;
    (start_time(pid) == Some(started)).then_some(pidfd)
}

/// Sends `signal` to the process of `pidfd`, waits up to `timeout_ms` for
/// it to exit, then kills it. A process that is gone already needs
/// nothing.
fn stop_process(pidfd: &OwnedFd, signal: i32, timeout_ms: u64) {
    let signal = Signal::from_named_raw(signal).unwrap_or(Signal::TERM);
    if rustix::process::pidfd_send_signal(pidfd, signal).is_err() {
        return;
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let timeout = Timespec {
            tv_sec: left.as_secs().try_into().unwrap_or(i64::MAX),
            tv_nsec: left.subsec_nanos().into(),
        };
        let mut fds = [PollFd::new(pidfd, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(n) if n > 0 => return,
            Err(Errno::INTR) => continue,
            _ => break,
        }
    }
    let _ = rustix::process::pidfd_send_signal(pidfd, Signal::KILL);
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
