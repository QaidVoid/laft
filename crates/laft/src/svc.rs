//! `laft svc`: the state of services under runit, runtime start and stop,
//! logs, and the launcher entry points of the rendered runit scripts.

use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use laft_core::Timestamp;
use laft_svc::{ExecMode, Launcher, SETUP_FAILED, ServiceKind, Spec, SpecLog, SpecReady};
use serde_json::json;

use crate::cli::SvcCommand;
use crate::env::Env;
use crate::failure::{Failure, exit};
use crate::ui::{self, table};

/// The scan directory of runit.
const SERVICE_DIR: &str = "/run/runit/service";

/// How a service is doing, from the supervisor's records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct State {
    /// `up`, `starting`, `down`, `failed` or `done`.
    pub(crate) state: &'static str,
    /// Whether the service is ready.
    pub(crate) ready: bool,
    /// The process id, while it runs.
    pub(crate) pid: Option<u32>,
    /// How long it has been in its state, while it runs.
    pub(crate) uptime: Option<Duration>,
}

/// The `supervise/status` record of runsv: when the state last changed,
/// the pid, and whether the service runs and should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Supervised {
    /// Seconds since the epoch when the state last changed.
    pub(crate) since: u64,
    /// The pid, 0 when it does not run.
    pub(crate) pid: u32,
    /// Whether the supervisor wants it up.
    pub(crate) want_up: bool,
}

/// Parses the 20 byte `supervise/status` file of runsv.
pub(crate) fn parse_status(bytes: &[u8]) -> Option<Supervised> {
    if bytes.len() < 20 {
        return None;
    }
    let tai = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
    let since = tai.checked_sub(4_611_686_018_427_387_914)?;
    let pid = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    Some(Supervised {
        since,
        pid,
        want_up: bytes[17] != b'd',
    })
}

/// The directory of generation specs the command reads: the active
/// generation's.
fn spec_dir(env: &Env) -> Result<PathBuf, Failure> {
    if env.live() {
        return Ok(env.path("/run/laft/system/services/spec"));
    }
    let active = env
        .active()?
        .ok_or_else(|| Failure::general("no generation is active"))?;
    Ok(env
        .root
        .path()
        .join(format!("laft/gens/{active}/services/spec")))
}

/// The specs of `names`, or of every service.
fn specs(env: &Env, names: &[String]) -> Result<Vec<Spec>, Failure> {
    let dir = spec_dir(env)?;
    let mut wanted: Vec<String> = names.to_vec();
    if wanted.is_empty() {
        for item in
            fs::read_dir(&dir).map_err(|e| Failure::general(format!("{}: {e}", dir.display())))?
        {
            let name = item?.file_name().to_string_lossy().into_owned();
            if let Some(name) = name.strip_suffix(".json") {
                wanted.push(name.to_owned());
            }
        }
        wanted.sort();
    }
    let mut seen = std::collections::BTreeSet::new();
    wanted.retain(|name| seen.insert(name.clone()));
    wanted
        .iter()
        .map(|name| {
            let path = dir.join(format!("{name}.json"));
            if name.contains('/') || name.starts_with('.') || !path.exists() {
                return Err(Failure::new(
                    exit::USAGE,
                    format!("no service {name} is enabled"),
                ));
            }
            Spec::read(&path).map_err(|e| Failure::general(format!("service {name}: {e}")))
        })
        .collect()
}

/// Whether the boot recorded `name` as failed.
fn boot_failed(env: &Env, name: &str) -> bool {
    fs::read_to_string(env.path("/run/laft/boot-failures"))
        .is_ok_and(|t| t.lines().any(|l| l == format!("{name} failed")))
}

/// The state of service `spec`.
fn state_of(env: &Env, spec: &Spec) -> State {
    let ready = env.path("/run/runit/ready").join(&spec.name).exists();
    let supervise = env.path(SERVICE_DIR).join(&spec.name).join("supervise");
    let status = fs::read(supervise.join("status"))
        .ok()
        .and_then(|b| parse_status(&b));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let failed = boot_failed(env, &spec.name);
    match status {
        Some(s) if s.pid != 0 => State {
            state: if ready && spec.kind == ServiceKind::Oneshot {
                "done"
            } else if ready || matches!(spec.ready, SpecReady::Immediate) {
                "up"
            } else {
                "starting"
            },
            ready: ready || matches!(spec.ready, SpecReady::Immediate),
            pid: Some(s.pid),
            uptime: Some(Duration::from_secs(now.saturating_sub(s.since))),
        },
        Some(s) => State {
            state: if s.want_up || failed {
                "failed"
            } else {
                "down"
            },
            ready: false,
            pid: None,
            uptime: None,
        },
        None if spec.early && ready => State {
            state: if spec.kind == ServiceKind::Oneshot {
                "done"
            } else {
                "up"
            },
            ready: true,
            pid: None,
            uptime: None,
        },
        None => State {
            state: if failed { "failed" } else { "down" },
            ready: false,
            pid: None,
            uptime: None,
        },
    }
}

/// `laft svc status`.
fn status(env: &Env, names: &[String]) -> Result<u8, Failure> {
    let specs = specs(env, names)?;
    let mut rows = Vec::new();
    let mut docs = Vec::new();
    for spec in &specs {
        let s = state_of(env, spec);
        docs.push(
            json!({"name": spec.name, "state": s.state, "ready": s.ready,
            "pid": s.pid, "uptime_secs": s.uptime.map(|u| u.as_secs())}),
        );
        let hint = if s.state == "failed" {
            format!("see: laft svc log {}", spec.name)
        } else {
            String::new()
        };
        rows.push(vec![
            spec.name.clone(),
            s.state.to_owned(),
            if s.ready { "yes".into() } else { "no".into() },
            s.pid.map_or_else(|| "-".into(), |p| p.to_string()),
            s.uptime.map_or_else(|| "-".into(), ui::duration),
            hint,
        ]);
    }
    if env.ui.json {
        env.ui.doc(json!({"services": docs}));
    } else {
        env.ui
            .out(table(&["SERVICE", "STATE", "READY", "PID", "UPTIME", ""], &rows).trim_end());
    }
    Ok(0)
}

/// `laft svc show`.
fn show(env: &Env, name: &str) -> Result<u8, Failure> {
    let spec = specs(env, &[name.to_owned()])?.remove(0);
    if env.ui.json {
        env.ui
            .doc(serde_json::to_value(&spec).map_err(Failure::general)?);
        return Ok(0);
    }
    let mut out = String::new();
    let _ = writeln!(out, "service     {}", spec.name);
    let _ = writeln!(
        out,
        "kind        {}{}",
        spec.kind.as_str(),
        if spec.early { ", early" } else { "" }
    );
    if !spec.exec.is_empty() {
        let _ = writeln!(out, "exec        {}", spec.exec.join(" "));
    }
    if !spec.up.is_empty() {
        let _ = writeln!(out, "up          {}", spec.up.join(" "));
    }
    if !spec.down.is_empty() {
        let _ = writeln!(out, "down        {}", spec.down.join(" "));
    }
    let group = spec.group.clone().unwrap_or_default();
    let _ = writeln!(
        out,
        "user        {}{}",
        spec.user,
        if group.is_empty() {
            String::new()
        } else {
            format!(":{group}")
        }
    );
    if !spec.after.is_empty() {
        let _ = writeln!(out, "after       {}", spec.after.join(", "));
    }
    let ready = match &spec.ready {
        SpecReady::Immediate => "immediately".to_owned(),
        SpecReady::Notify { .. } => "when it notifies".to_owned(),
        SpecReady::Check { cmd, .. } => format!("when `{}` succeeds", cmd.join(" ")),
    };
    let _ = writeln!(out, "ready       {ready}");
    let _ = writeln!(out, "restart     {:?}", spec.restart);
    let log = match &spec.log {
        SpecLog::Logged { dir, .. } => dir.clone(),
        SpecLog::Discard => "discarded".into(),
        SpecLog::Console => "console".into(),
    };
    let _ = writeln!(out, "log         {log}");
    let _ = writeln!(out, "generation  {}", spec_dir(env)?.display());
    env.ui.out(out.trim_end());
    Ok(0)
}

/// Runs `sv <command> <service dirs>`.
fn sv(env: &Env, command: &str, specs: &[Spec]) -> Result<(), Failure> {
    let dirs: Vec<PathBuf> = specs
        .iter()
        .map(|s| env.path(SERVICE_DIR).join(&s.name))
        .collect();
    // Its messages go to standard error, which keeps a JSON event stream
    // on standard output intact.
    let status = Command::new(env.path("/usr/bin/sv"))
        .arg(command)
        .args(&dirs)
        .stdout(std::process::Stdio::from(std::io::stderr()))
        .status()
        .map_err(|e| Failure::general(format!("sv: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Failure::general(format!("sv {command} failed")))
    }
}

/// Waits until every service of `specs`, which were all started at
/// `started`, is ready or its own readiness timeout after that start
/// passed, and returns those that did not become ready.
fn wait_ready(env: &Env, specs: &[Spec], started: Instant) -> Vec<String> {
    let ready = env.path("/run/runit/ready");
    specs
        .iter()
        .filter(|spec| {
            let deadline = started + Duration::from_millis(spec.ready_timeout_ms());
            !laft_svc::reconcile::wait_ready(&ready, &spec.name, deadline)
                && !matches!(spec.ready, SpecReady::Immediate)
        })
        .map(|spec| spec.name.clone())
        .collect()
}

/// `laft svc start`, `stop` and `restart`.
fn control(env: &Env, command: &str, names: &[String]) -> Result<u8, Failure> {
    env.require_write(&format!("svc {command}"))?;
    if !env.live() {
        return Err(Failure::general(format!(
            "laft svc {command} needs a running system, and none runs below --root"
        )));
    }
    env.ui.start(&format!("svc {command}"));
    let specs = specs(env, names)?;
    if let Some(early) = specs.iter().find(|s| s.early) {
        return Err(Failure::general(format!(
            "{} belongs to the early boot graph, which no supervisor runs",
            early.name
        )));
    }
    let started = Instant::now();
    match command {
        "stop" => sv(env, "down", &specs)?,
        "start" => sv(env, "up", &specs)?,
        _ => sv(env, "restart", &specs)?,
    }
    let failed = if command == "stop" {
        Vec::new()
    } else {
        wait_ready(env, &specs, started)
    };
    env.ui
        .event(json!({"event": "result", "services": names, "failed": failed}));
    for name in &failed {
        env.ui.warn(format!(
            "{name} did not become ready, see: laft svc log {name}"
        ));
    }
    Ok(if failed.is_empty() { 0 } else { exit::SERVICES })
}

/// The local time of a TAI64N label such as `@4000000068c2a1b80c4d2a1e`.
pub(crate) fn tai64n(label: &str) -> Option<Timestamp> {
    let hex = label.strip_prefix('@')?.get(..16)?;
    let tai = u64::from_str_radix(hex, 16).ok()?;
    Some(Timestamp::from_secs(
        tai.checked_sub(4_611_686_018_427_387_914)?,
    ))
}

/// A log line with its TAI64N label in local time.
fn log_line(line: &str) -> String {
    match line.split_once(' ') {
        Some((label, rest)) if label.starts_with('@') => match tai64n(label) {
            Some(time) => format!("{}  {rest}", ui::local_time(time)),
            None => line.to_owned(),
        },
        _ => line.to_owned(),
    }
}

/// `laft svc log`.
fn log(env: &Env, name: &str, lines: usize, follow: bool) -> Result<u8, Failure> {
    let path = env.path("/var/log").join(name).join("current");
    let text = fs::read_to_string(&path)
        .map_err(|e| Failure::general(format!("{}: {e}", path.display())))?;
    let all: Vec<&str> = text.lines().collect();
    for line in &all[all.len().saturating_sub(lines)..] {
        env.ui.out(log_line(line));
    }
    if follow {
        follow_file(env, &path, text.len() as u64)?;
    }
    Ok(0)
}

/// Prints what is appended to `path` from `offset` on, reopening it when
/// the logger rotates it.
fn follow_file(env: &Env, path: &Path, mut offset: u64) -> Result<(), Failure> {
    use std::os::unix::fs::MetadataExt;
    let mut inode = fs::metadata(path)?.ino();
    let mut pending = String::new();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let Ok(meta) = fs::metadata(path) else {
            continue;
        };
        if meta.ino() != inode || meta.len() < offset {
            inode = meta.ino();
            offset = 0;
        }
        if meta.len() == offset {
            continue;
        }
        let mut file = fs::File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut chunk = String::new();
        offset += file.read_to_string(&mut chunk)? as u64;
        pending.push_str(&chunk);
        while let Some(end) = pending.find('\n') {
            env.ui.out(log_line(&pending[..end]));
            pending.drain(..=end);
        }
    }
}

/// Runs a launcher entry point and returns its exit status. Launcher
/// failures go to standard error and give the status that makes the
/// supervisor try again later.
fn launcher(env: &Env, run: impl FnOnce(&Launcher) -> Result<i32, laft_svc::Error>) -> u8 {
    match run(&Launcher::new(env.root.path())) {
        Ok(code) => u8::try_from(code).unwrap_or(exit::FAILURE),
        Err(e) => {
            eprintln!("laft svc: {e}");
            u8::try_from(SETUP_FAILED).unwrap_or(exit::FAILURE)
        }
    }
}

/// Dispatches a `laft svc` command.
pub(crate) fn run(env: &Env, command: SvcCommand) -> Result<u8, Failure> {
    match command {
        SvcCommand::Status { names } => status(env, &names),
        SvcCommand::Show { name } => show(env, &name),
        SvcCommand::Start { names } => control(env, "start", &names),
        SvcCommand::Stop { names } => control(env, "stop", &names),
        SvcCommand::Restart { names } => control(env, "restart", &names),
        SvcCommand::Log {
            name,
            lines,
            follow,
        } => log(env, &name, lines, follow),
        SvcCommand::Exec {
            background,
            detached,
            name,
            ..
        } => {
            let mode = if detached {
                ExecMode::Detached
            } else if background {
                ExecMode::Background
            } else {
                ExecMode::Supervised
            };
            Ok(launcher(env, |l| l.exec(&name, mode)))
        }
        SvcCommand::Up { name, .. } => Ok(launcher(env, |l| l.up(&name))),
        SvcCommand::Down { name, .. } => Ok(launcher(env, |l| l.down(&name))),
        SvcCommand::Finish {
            name,
            exit_code,
            signal,
            ..
        } => Ok(launcher(env, |l| l.finish(&name, exit_code, signal))),
        SvcCommand::Logger { name, .. } => Ok(launcher(env, |l| l.logger(&name))),
        SvcCommand::StopEarly { .. } => Ok(launcher(env, Launcher::stop_early)),
        SvcCommand::Ready { parent, name, .. } => Ok(launcher(env, |l| l.ready(&name, parent))),
        SvcCommand::Builtin { step, action } => Ok(builtin(env, step, action)),
    }
}

/// Runs a built-in early boot step on the root and prints its warnings.
/// A failure exits with status 1, which fails the oneshot.
fn builtin(env: &Env, step: laft_svc::builtin::Step, action: laft_svc::builtin::Action) -> u8 {
    match laft_svc::builtin::run(env.root.path(), step, action) {
        Ok(warnings) => {
            for warning in warnings {
                eprintln!("laft svc builtin: warning: {warning}");
            }
            0
        }
        Err(e) => {
            eprintln!("laft svc builtin: {e}");
            exit::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "svc_tests.rs"]
mod tests;
