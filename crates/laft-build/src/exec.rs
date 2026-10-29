//! Running build phases and tools, in a sandbox or, in bootstrap mode,
//! directly on the host.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use laft_sandbox::{Network, Sandbox};

use crate::error::{BuildError, io};

/// How long a phase may run unless the caller chooses otherwise: six hours.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(6 * 3600);

/// Where commands run.
#[derive(Debug, Clone)]
enum Place {
    /// In a [`Sandbox`] with this view as `/usr`.
    Sandbox(PathBuf),
    /// Directly on the host, with the tools of `<root>/usr/bin`.
    Direct(PathBuf),
}

/// Runs the commands of one build: its phase scripts and the tools that
/// post-processing needs.
///
/// A sandboxed executor runs everything in a [`Sandbox`] whose `/usr` is
/// the build view, whose `/build` is the build directory and whose `/out`
/// is the output directory, so every build sees the same absolute paths. A
/// phase has no network. Only [`Executor::fetch`] runs with the host's
/// network, for the steps that download.
///
/// A direct executor is for bootstrap mode, which builds inside an
/// existing system before any build view exists. It runs commands as
/// ordinary host processes with `<root>/usr/bin` as `PATH`, after the
/// directories of [`Executor::search_first`], and the build directory as
/// the working directory. Nothing isolates them, so a build
/// made this way is not hermetic. Every `/build` and `/out` path in the
/// environment and the scripts is rewritten to the real directories, which
/// keeps recipes working unchanged.
///
/// Every command's standard output and standard error go to
/// `<logs>/<phase>.log`, which grows when a phase name repeats. Standard
/// input is empty. A command that runs longer than the timeout is killed.
#[derive(Debug, Clone)]
pub struct Executor {
    place: Place,
    build: PathBuf,
    out: PathBuf,
    logs: PathBuf,
    timeout: Duration,
    search: Vec<PathBuf>,
}

impl Executor {
    /// An executor that runs every command in a sandbox with `view` as
    /// `/usr`, `build` as `/build` and `out` as `/out`, logging to `logs`.
    pub fn sandboxed(
        view: impl Into<PathBuf>,
        build: impl Into<PathBuf>,
        out: impl Into<PathBuf>,
        logs: impl Into<PathBuf>,
    ) -> Self {
        Self {
            place: Place::Sandbox(view.into()),
            build: build.into(),
            out: out.into(),
            logs: logs.into(),
            timeout: DEFAULT_TIMEOUT,
            search: Vec::new(),
        }
    }

    /// An executor for bootstrap mode that runs every command directly on
    /// the host with the tools of `root/usr/bin`, in `build`, with `out` as
    /// the output directory, logging to `logs`.
    pub fn direct(
        root: impl Into<PathBuf>,
        build: impl Into<PathBuf>,
        out: impl Into<PathBuf>,
        logs: impl Into<PathBuf>,
    ) -> Self {
        Self {
            place: Place::Direct(root.into()),
            build: build.into(),
            out: out.into(),
            logs: logs.into(),
            timeout: DEFAULT_TIMEOUT,
            search: Vec::new(),
        }
    }

    /// Sets the longest time one command may run.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Makes a direct executor look for programs in `dirs`, in order,
    /// before `<root>/usr/bin`, for tools that a bootstrap installs outside
    /// the root's `usr`, such as a cross toolchain. A sandboxed executor
    /// ignores them, because its `PATH` belongs to the build view.
    pub fn search_first(mut self, dirs: Vec<PathBuf>) -> Self {
        self.search = dirs;
        self
    }

    /// The `PATH` of a direct executor with the tools of `root`, which
    /// fails for a directory whose name contains `:`.
    fn direct_path(&self, root: &Path) -> Result<OsString, BuildError> {
        let dirs = self.search.iter().cloned().chain([root.join("usr/bin")]);
        std::env::join_paths(dirs).map_err(|e| {
            let bad = self
                .search
                .iter()
                .find(|d| d.to_string_lossy().contains(':'));
            BuildError::Io {
                path: bad.cloned().unwrap_or_default(),
                error: io::Error::new(io::ErrorKind::InvalidInput, e),
            }
        })
    }

    /// Whether commands run in a sandbox.
    pub fn is_sandboxed(&self) -> bool {
        matches!(self.place, Place::Sandbox(_))
    }

    /// The `usr` tree commands see as `/usr`.
    pub fn usr(&self) -> PathBuf {
        match &self.place {
            Place::Sandbox(view) => view.clone(),
            Place::Direct(root) => root.join("usr"),
        }
    }

    /// The directory that holds the logs.
    pub fn logs(&self) -> &Path {
        &self.logs
    }

    /// The path under which commands see the payload path `rel` of the
    /// output directory.
    pub fn out_path(&self, rel: &str) -> String {
        match self.place {
            Place::Sandbox(_) => format!("/out/{rel}"),
            Place::Direct(_) => format!("{}/{rel}", self.out.display()),
        }
    }

    /// Runs the `bash` script `script` as the phase `phase` with exactly the
    /// environment `env`, without network.
    ///
    /// # Errors
    ///
    /// Fails with `B040` when the script exits unsuccessfully, with `B030`
    /// when it runs out of time, and with [`BuildError::Sandbox`] when the
    /// sandbox cannot be set up.
    pub fn run(
        &self,
        phase: &str,
        script: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<(), BuildError> {
        self.run_with(phase, script, env, Network::Isolated)
    }

    /// Runs `script` like [`Executor::run`], but with the host's network,
    /// for a step that downloads.
    ///
    /// # Errors
    ///
    /// Fails like [`Executor::run`].
    pub fn fetch(
        &self,
        phase: &str,
        script: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<(), BuildError> {
        self.run_with(phase, script, env, Network::Host)
    }

    /// Runs the program `program` of the view's `/usr/bin` with `args` and
    /// only `PATH` in its environment, logging as the phase `phase`.
    ///
    /// # Errors
    ///
    /// Fails like [`Executor::run`].
    pub fn tool(&self, phase: &str, program: &str, args: &[String]) -> Result<(), BuildError> {
        let script = std::iter::once(program.to_owned())
            .chain(args.iter().map(|a| crate::style::quote(a)))
            .collect::<Vec<_>>()
            .join(" ");
        let env = BTreeMap::from([("PATH".to_owned(), "/usr/bin".to_owned())]);
        self.run(phase, &format!("exec {script}"), &env)
    }

    /// Starts an interactive `bash` in the build's environment `env`, with
    /// the caller's terminal, and waits for it to exit. It has no time
    /// limit and no log.
    ///
    /// # Errors
    ///
    /// Fails with [`BuildError::Sandbox`] when the sandbox cannot be set up,
    /// and when `bash` cannot be started.
    pub fn interactive(&self, env: &BTreeMap<String, String>) -> Result<ExitStatus, BuildError> {
        match &self.place {
            Place::Sandbox(view) => {
                let command = Sandbox::new(view, &self.build, &self.out)
                    .command("/usr/bin/bash")
                    .arg("-i");
                Ok(env.iter().fold(command, |c, (k, v)| c.env(k, v)).status()?)
            }
            Place::Direct(root) => {
                let bash = root.join("usr/bin/bash");
                std::process::Command::new(&bash)
                    .arg("-i")
                    .env_clear()
                    .envs(
                        env.iter()
                            .map(|(k, v)| (k, rebase(v, &self.build, &self.out))),
                    )
                    .env("PATH", self.direct_path(root)?)
                    .current_dir(&self.build)
                    .status()
                    .map_err(io(&bash))
            }
        }
    }

    fn run_with(
        &self,
        phase: &str,
        script: &str,
        env: &BTreeMap<String, String>,
        network: Network,
    ) -> Result<(), BuildError> {
        fs::create_dir_all(&self.logs).map_err(io(&self.logs))?;
        let log_path = self.logs.join(format!("{phase}.log"));
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(io(&log_path))?;
        let err = log.try_clone().map_err(io(&log_path))?;
        let child = match &self.place {
            Place::Sandbox(view) => {
                let command = Sandbox::new(view, &self.build, &self.out)
                    .network(network)
                    .command("/usr/bin/bash")
                    .arg("-c")
                    .arg(script)
                    .arg(phase);
                env.iter()
                    .fold(command, |c, (k, v)| c.env(k, v))
                    .stdin(Stdio::null())
                    .stdout(log)
                    .stderr(err)
                    .spawn()?
            }
            Place::Direct(root) => {
                let mut rebased: BTreeMap<String, OsString> = env
                    .iter()
                    .map(|(k, v)| (k.clone(), rebase(v, &self.build, &self.out).into()))
                    .collect();
                rebased.insert("PATH".to_owned(), self.direct_path(root)?);
                let bash = root.join("usr/bin/bash");
                std::process::Command::new(&bash)
                    .arg("-c")
                    .arg(rebase(script, &self.build, &self.out))
                    .arg(phase)
                    .process_group(0)
                    .env_clear()
                    .envs(&rebased)
                    .current_dir(&self.build)
                    .stdin(Stdio::null())
                    .stdout(log)
                    .stderr(err)
                    .spawn()
                    .map_err(io(&bash))?
            }
        };
        let status = wait(child, self.timeout, &log_path)?;
        if status.success() {
            return Ok(());
        }
        Err(BuildError::problem(
            "B040",
            format!(
                "phase {phase} failed with {status}, see {}{}",
                log_path.display(),
                tail(&log_path)
            ),
        ))
    }
}

/// Waits for `child` until `timeout` has passed, then kills it. A direct
/// phase leads its own process group, which is killed with it, so that
/// nothing it started keeps writing to the tree. A sandboxed phase is the
/// first process of its PID namespace, whose other processes the kernel
/// kills when it dies.
fn wait(mut child: Child, timeout: Duration, log: &Path) -> Result<ExitStatus, BuildError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(io(log))? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            // A group with the child's pid as its id exists only when the
            // child leads it, so this never reaches unrelated processes.
            if let Some(pid) = i32::try_from(child.id())
                .ok()
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            let hours = timeout.as_secs_f64() / 3600.0;
            return Err(BuildError::problem(
                "B030",
                format!("build exceeded {hours}h"),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The last lines of the log `path`, indented, for an error message.
fn tail(path: &Path) -> String {
    let text = fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(20);
    lines[start..]
        .iter()
        .map(|l| format!("\n  | {l}"))
        .collect()
}

/// Rewrites every `/build` and `/out` path in `text` to start with `build`
/// and `out` instead. A path counts when it starts the text or follows a
/// space, a tab, a newline, a quote, `=`, `:` or `,`, and ends there or
/// continues with `/` or one of those characters.
pub(crate) fn rebase(text: &str, build: &Path, out: &Path) -> String {
    let build = build.display().to_string();
    let out = out.display().to_string();
    let bytes = text.as_bytes();
    let boundary = |b: u8| matches!(b, b' ' | b'"' | b'\'' | b'=' | b':' | b',' | b'\n' | b'\t');
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let starts = i == 0 || boundary(bytes[i - 1]);
        let replaced = [("/build", &build), ("/out", &out)]
            .into_iter()
            .find(|(prefix, _)| {
                starts
                    && text[i..].starts_with(prefix)
                    && bytes
                        .get(i + prefix.len())
                        .is_none_or(|&b| b == b'/' || boundary(b))
            });
        match replaced {
            Some((prefix, to)) => {
                result.push_str(to);
                i += prefix.len();
            }
            None => {
                let c = text[i..].chars().next().unwrap_or_default();
                result.push(c);
                i += c.len_utf8();
            }
        }
    }
    result
}

#[cfg(test)]
#[path = "exec_tests.rs"]
mod tests;
