//! The runit backend: the files a generation needs to boot and supervise
//! its services with runit.
//!
//! Every service gets its spec file. Services outside the early boot graph
//! get a service directory whose `run`, `finish` and `log/run` scripts only
//! call the launcher, so their content depends on nothing but the service
//! name, log policy and stop signal.
//!
//! runsv stops a service with `SIGTERM`. A service with another stop signal
//! gets the control hook `control/t`, which sends that signal instead. runsv
//! runs the hook for every request that would send `SIGTERM`, including
//! `sv down`, `sv force-stop` and `sv exit`, and skips its own signal when
//! the hook succeeds. A `control/d` hook is not rendered, because runsv
//! runs it after `control/t` for the same request, which would signal the
//! service twice. The early boot graph runs in stage 1 as a plain list
//! of launcher calls in dependency order, because runit supervises nothing
//! before stage 2.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use laft_core::Blake3Hash;

use crate::error::{Error, corrupt, io};
use crate::spec::{Spec, SpecLog};
use crate::standard::ACTIVATE;
use crate::{ServiceKind, ServiceSet, Signal, graph};

/// The launcher, as rendered scripts call it.
const LAUNCHER: &str = "/usr/bin/laft svc";

/// The mode of rendered scripts.
const SCRIPT: u32 = 0o555;

/// The mode of other rendered files.
const DATA: u32 = 0o444;

/// The shortest wait in `sv -w` at shutdown, in seconds. A supervised
/// service with a longer stop timeout makes the wait that long.
const DEFAULT_SHUTDOWN_WAIT: u64 = 10;

/// runsvdir's status line: it shows the latest output of services without a
/// logger in its process title, overwriting these dots.
const READPROCTITLE: &str = "log: ...........................................................................................................................................";

/// A rendered file or symlink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A regular file.
    File {
        /// Its permission bits.
        mode: u32,
        /// Its content.
        bytes: Vec<u8>,
    },
    /// A symbolic link.
    Symlink(String),
}

/// What the runit backend renders for one generation, by path relative to
/// the generation directory.
///
/// Paths below `services/` are the spec files and the runit tree. Paths
/// below `etc/` are the entry points that runit and interactive shells read
/// from fixed locations in `/etc`. The same service set and generation
/// number always render the same bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    entries: BTreeMap<String, Entry>,
    units: BTreeMap<String, Unit>,
}

/// The rendered state of one service, which reconciliation compares between
/// generations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The service's spec.
    pub spec: Spec,
    /// The spec hash, [`Spec::hash`]: the hash of the spec's canonical
    /// JSON, which is the content of its spec file.
    pub spec_hash: Blake3Hash,
    /// The BLAKE3 hash of the backend files rendered for the service. For a
    /// service of the early boot graph, that is its line in stage 1.
    pub unit_hash: Blake3Hash,
}

/// Renders `set` for runit, for generation `generation`.
///
/// ```
/// use laft_svc::{ServiceConfig, ServiceSet, runit, standard_services};
///
/// let config = ServiceConfig { services: standard_services(), ..ServiceConfig::default() };
/// let set = ServiceSet::new(&config).expect("the services are valid");
/// let rendered = runit::render(&set, 42)?;
/// assert!(rendered.entries().contains_key("services/runit/sv/getty-tty1/run"));
/// assert!(rendered.entries().contains_key("services/spec/udevd.json"));
/// # Ok::<(), laft_svc::Error>(())
/// ```
///
/// # Errors
///
/// Fails only when a spec cannot be encoded as canonical JSON, which a
/// validated set never causes.
pub fn render(set: &ServiceSet, generation: u64) -> Result<Rendered, Error> {
    let specs = set.specs();
    let mut entries = BTreeMap::new();
    let mut units = BTreeMap::new();
    let mut file = |path: String, mode: u32, bytes: Vec<u8>| {
        entries.insert(path, Entry::File { mode, bytes });
    };
    let early = early_order(specs);
    let early_lines: BTreeMap<&str, String> = early
        .iter()
        .map(|s| (s.name.as_str(), stage1_line(s)))
        .collect();
    let mut spec_hashes = BTreeMap::new();
    for (name, spec) in specs {
        spec_hashes.insert(name, spec.hash()?);
        file(format!("services/spec/{name}.json"), DATA, spec.to_bytes()?);
    }
    for (name, spec) in specs {
        let unit_hash = match early_lines.get(name.as_str()) {
            Some(line) => Blake3Hash::of(line.as_bytes()),
            None => {
                let unit = service_dir(spec);
                for (path, mode, bytes) in &unit {
                    file(
                        format!("services/runit/sv/{name}/{path}"),
                        *mode,
                        bytes.clone(),
                    );
                }
                unit_hash(&unit)
            }
        };
        units.insert(
            name.clone(),
            Unit {
                spec: spec.clone(),
                spec_hash: spec_hashes[name],
                unit_hash,
            },
        );
    }
    file(
        "services/runit/stage/1".into(),
        SCRIPT,
        stage1(generation, &early, &early_lines),
    );
    file("services/runit/stage/2".into(), SCRIPT, stage2());
    file(
        "services/runit/stage/3".into(),
        SCRIPT,
        stage3(specs, &early),
    );
    file(
        "services/runit/stage/ctrlaltdel".into(),
        SCRIPT,
        b"#!/bin/sh\nexec /run/laft/booted/usr/bin/runit-init 6\n".to_vec(),
    );
    let longruns: String = early
        .iter()
        .filter(|s| s.kind == ServiceKind::Longrun)
        .map(|s| format!("{}\n", s.name))
        .collect();
    file(
        "services/runit/early-longruns".into(),
        DATA,
        longruns.into_bytes(),
    );
    file(
        "etc/profile.d/laft-runit.sh".into(),
        DATA,
        b"export SVDIR=/run/runit/service\n".to_vec(),
    );
    for stage in ["1", "2", "3", "ctrlaltdel"] {
        entries.insert(
            format!("etc/runit/{stage}"),
            Entry::Symlink(format!("/run/laft/booted/services/runit/stage/{stage}")),
        );
    }
    for written_by_runit in ["stopit", "reboot"] {
        entries.insert(
            format!("etc/runit/{written_by_runit}"),
            Entry::Symlink(format!("/run/runit/{written_by_runit}")),
        );
    }
    Ok(Rendered { entries, units })
}

impl Rendered {
    /// Every rendered file and symlink, by path relative to the generation
    /// directory.
    pub fn entries(&self) -> &BTreeMap<String, Entry> {
        &self.entries
    }

    /// The rendered state of every service, by name.
    pub fn units(&self) -> &BTreeMap<String, Unit> {
        &self.units
    }

    /// Writes every entry below `generation_dir`, a generation tree under
    /// construction, with [`Entry::write`].
    ///
    /// # Errors
    ///
    /// Fails when an entry exists already or cannot be written.
    pub fn write(&self, generation_dir: &Path) -> Result<(), Error> {
        for (relative, entry) in &self.entries {
            entry.write(&generation_dir.join(relative))?;
        }
        Ok(())
    }
}

impl Entry {
    /// Creates the entry at `path`, a new path inside a generation tree
    /// under construction, and its missing parent directories with mode
    /// `0o755`. A file gets exactly its mode, whatever the umask.
    ///
    /// The file is not synced, because a generation is published with one
    /// rename after its whole tree is synced.
    ///
    /// # Errors
    ///
    /// Fails when something exists at `path` already or cannot be written.
    pub fn write(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            create_dirs(parent)?;
        }
        match self {
            Self::File { mode, bytes } => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
                    .map_err(io(path))?;
                file.write_all(bytes).map_err(io(path))?;
                file.set_permissions(fs::Permissions::from_mode(*mode))
                    .map_err(io(path))
            }
            Self::Symlink(target) => symlink(target, path).map_err(io(path)),
        }
    }
}

/// Reads the rendered state of every service from the `services` directory
/// of a generation, so that it can be compared with another generation's.
///
/// # Errors
///
/// Fails when a spec file or a service's backend files cannot be read, or
/// do not belong together.
pub fn load_units(services_dir: &Path) -> Result<BTreeMap<String, Unit>, Error> {
    let spec_dir = services_dir.join("spec");
    let stage1_path = services_dir.join("runit/stage/1");
    let stage1 = fs::read_to_string(&stage1_path).map_err(io(&stage1_path))?;
    let early_lines: BTreeMap<&str, &str> = stage1
        .lines()
        .filter(|l| l.starts_with(LAUNCHER))
        .filter_map(|l| Some((l.rsplit(' ').next()?, l)))
        .collect();
    let mut units = BTreeMap::new();
    for item in fs::read_dir(&spec_dir).map_err(io(&spec_dir))? {
        let path = item.map_err(io(&spec_dir))?.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        let bytes = fs::read(&path).map_err(io(&path))?;
        let spec = Spec::parse(&bytes, &path)?;
        if spec.name != name {
            return Err(corrupt(&path, "the spec names a different service"));
        }
        let unit_hash = if spec.early {
            let line = early_lines
                .get(name)
                .ok_or_else(|| corrupt(&stage1_path, format!("no line starts {name}")))?;
            Blake3Hash::of(line.as_bytes())
        } else {
            let dir = services_dir.join("runit/sv").join(name);
            let mut files = Vec::new();
            read_unit(&dir, &dir, &mut files)?;
            files.sort();
            unit_hash(&files)
        };
        units.insert(
            name.to_owned(),
            Unit {
                spec_hash: spec.hash()?,
                spec,
                unit_hash,
            },
        );
    }
    Ok(units)
}

/// A file of a service directory: its path inside the directory, its mode
/// and its content.
type UnitFile = (String, u32, Vec<u8>);

fn read_unit(base: &Path, dir: &Path, files: &mut Vec<UnitFile>) -> Result<(), Error> {
    for item in fs::read_dir(dir).map_err(io(dir))? {
        let path = item.map_err(io(dir))?.path();
        let metadata = fs::symlink_metadata(&path).map_err(io(&path))?;
        if metadata.is_dir() {
            read_unit(base, &path, files)?;
        } else {
            let relative = path
                .strip_prefix(base)
                .map_err(|_| corrupt(&path, "outside its service directory"))?
                .to_string_lossy()
                .into_owned();
            let bytes = fs::read(&path).map_err(io(&path))?;
            files.push((relative, metadata.mode() & 0o7777, bytes));
        }
    }
    Ok(())
}

/// Hashes the files of a service directory, sorted by path.
fn unit_hash(files: &[UnitFile]) -> Blake3Hash {
    let mut hasher = blake3::Hasher::new();
    for (path, mode, bytes) in files {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&mode.to_le_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    Blake3Hash::from_bytes(*hasher.finalize().as_bytes())
}

/// The files of the service directory of a service outside the early boot
/// graph, sorted by path.
fn service_dir(spec: &Spec) -> Vec<UnitFile> {
    let name = &spec.name;
    let redirect = match spec.log {
        SpecLog::Logged { .. } => " 2>&1",
        SpecLog::Discard => " >/dev/null 2>&1",
        SpecLog::Console => "",
    };
    let script = |line: String| format!("#!/bin/sh\nexec {line}\n").into_bytes();
    let mut files = vec![
        (
            "finish".to_owned(),
            SCRIPT,
            script(format!(
                "{LAUNCHER} finish --backend runit {name} \"$1\" \"$2\""
            )),
        ),
        (
            "run".to_owned(),
            SCRIPT,
            script(format!("{LAUNCHER} exec --backend runit {name}{redirect}")),
        ),
    ];
    if let Some(signal) = Signal::from_number(spec.stop_signal).filter(|s| *s != Signal::Term) {
        // runsv runs control hooks in the service directory, where
        // `supervise/pid` holds the pid of the running service. Only shell
        // builtins are used, so the hook works whatever is installed.
        let hook = format!(
            "#!/bin/sh\nread -r pid < supervise/pid && kill -s {} \"$pid\"\n",
            signal.name()
        );
        files.push(("control/t".to_owned(), SCRIPT, hook.into_bytes()));
    }
    if let SpecLog::Logged { .. } = spec.log {
        files.push((
            "log/run".to_owned(),
            SCRIPT,
            script(format!("{LAUNCHER} logger --backend runit {name}")),
        ));
    }
    files.sort();
    files
}

/// The services of the early boot graph in the order stage 1 runs them:
/// dependencies first, then the activation, then longruns, so that they
/// start as soon as their dependencies are up, then by name.
fn early_order(specs: &BTreeMap<String, Spec>) -> Vec<&Spec> {
    let graph: BTreeMap<&str, Vec<&str>> = specs
        .values()
        .filter(|s| s.early)
        .map(|s| {
            (
                s.name.as_str(),
                s.after.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let rank = |name: &str| match specs[name].kind {
        _ if name == ACTIVATE => 0,
        ServiceKind::Longrun => 1,
        ServiceKind::Oneshot => 2,
    };
    graph::order(&graph, rank)
        .expect("validated services have no dependency cycle")
        .into_iter()
        .map(|name| &specs[name])
        .collect()
}

fn stage1_line(spec: &Spec) -> String {
    let name = &spec.name;
    match spec.kind {
        ServiceKind::Longrun => format!("{LAUNCHER} exec --backend runit --background {name}"),
        ServiceKind::Oneshot => format!("{LAUNCHER} up --backend runit {name}"),
    }
}

fn stage1(generation: u64, early: &[&Spec], lines: &BTreeMap<&str, String>) -> Vec<u8> {
    let mut out = format!(
        "#!/bin/sh\n\
         # Rendered by laft for generation {generation}. Early boot graph, in dependency order.\n\
         PATH=/usr/bin\n\
         export PATH\n\
         mkdir -p /run/runit/ready /run/runit/service /run/runit/early\n\n"
    );
    for spec in early {
        out.push_str(&lines[spec.name.as_str()]);
        out.push('\n');
    }
    out.push_str("exit 0\n");
    out.into_bytes()
}

fn stage2() -> Vec<u8> {
    format!(
        "#!/bin/sh\n\
         PATH=/usr/bin\n\
         export PATH\n\
         cp -a /run/laft/system/services/runit/sv/. /run/runit/service/\n\
         exec env - PATH=/usr/bin runsvdir -P /run/runit/service '{READPROCTITLE}'\n"
    )
    .into_bytes()
}

fn stage3(specs: &BTreeMap<String, Spec>, early: &[&Spec]) -> Vec<u8> {
    let wait = specs
        .values()
        .filter(|s| !s.early)
        .map(|s| s.stop_timeout_ms.div_ceil(1000))
        .fold(DEFAULT_SHUTDOWN_WAIT, u64::max);
    let mut out = format!(
        "#!/bin/sh\n\
         PATH=/usr/bin\n\
         export PATH\n\
         sv -w {wait} force-stop /run/runit/service/*\n\
         sv exit /run/runit/service/*\n\
         # Stop early longruns started in stage 1.\n\
         {LAUNCHER} stop-early --backend runit\n\
         # Early oneshots with a down command, in reverse order.\n"
    );
    for spec in early.iter().rev() {
        if spec.kind == ServiceKind::Oneshot && !spec.down.is_empty() {
            out.push_str(&format!("{LAUNCHER} down --backend runit {}\n", spec.name));
        }
    }
    out.push_str(
        "kill -s TERM -1\n\
         sleep 2\n\
         kill -s KILL -1\n\
         sync\n\
         mount -o remount,ro /\n",
    );
    out.into_bytes()
}

/// Creates `dir` and its missing parents with mode `0o755`.
fn create_dirs(dir: &Path) -> Result<(), Error> {
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut current = dir;
    while !current.exists() {
        missing.push(current.to_owned());
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    for path in missing.iter().rev() {
        fs::create_dir(path).map_err(io(path))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(io(path))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "runit_tests.rs"]
mod tests;
