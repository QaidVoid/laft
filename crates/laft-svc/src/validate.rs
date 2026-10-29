//! Validation of the declared services, and their resolution into specs.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use laft_core::Name;

use crate::graph;
use crate::model::{Dir, Init, Log, OnChange, Ready, Service, ServiceConfig, ServiceKind};
use crate::spec::{SPEC_VERSION, Spec, SpecDir, SpecLimits, SpecLog, SpecReady};
use crate::standard::{self, ACTIVATE, EARLY};

/// The longest service name, in bytes.
const MAX_NAME: usize = 48;

/// One way in which the declared services are invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The init system has no backend in this version.
    UnsupportedInit(Init),
    /// A service name is malformed or reserved.
    InvalidName {
        /// The name.
        service: String,
        /// Why it is rejected.
        reason: String,
    },
    /// A terminal name in the getty list is malformed or listed twice.
    InvalidTty {
        /// The terminal name.
        tty: String,
        /// Why it is rejected.
        reason: &'static str,
    },
    /// A service lacks the command its kind requires.
    MissingCommand {
        /// The service.
        service: String,
        /// Its kind.
        kind: ServiceKind,
        /// The missing field.
        field: &'static str,
    },
    /// A service sets a command its kind does not use.
    UnexpectedCommand {
        /// The service.
        service: String,
        /// Its kind.
        kind: ServiceKind,
        /// The field that must be empty.
        field: &'static str,
    },
    /// The program of a command is not an absolute path.
    RelativeCommand {
        /// The service.
        service: String,
        /// The field, for example `exec`.
        field: &'static str,
        /// The program.
        program: String,
    },
    /// A path that must be absolute is not.
    RelativePath {
        /// The service.
        service: String,
        /// The field.
        field: &'static str,
        /// The path.
        path: String,
    },
    /// A directory path of `state_dirs` or `runtime_dirs` is malformed.
    InvalidDirPath {
        /// The service.
        service: String,
        /// The path.
        path: String,
    },
    /// A number is outside its allowed range.
    OutOfRange {
        /// The service.
        service: String,
        /// The field, for example `ready.fd`.
        field: &'static str,
        /// The value.
        value: u64,
        /// The smallest allowed value.
        min: u64,
        /// The largest allowed value.
        max: u64,
    },
    /// A setting that applies only to one kind of service.
    WrongKind {
        /// The service.
        service: String,
        /// What is wrong.
        reason: &'static str,
    },
    /// A dependency names no declared service.
    UnknownDependency {
        /// The service.
        service: String,
        /// The missing dependency.
        dep: String,
        /// The declared service with the closest name, if one is close.
        suggestion: Option<String>,
    },
    /// A dependency names a disabled service.
    DisabledDependency {
        /// The service.
        service: String,
        /// The disabled dependency.
        dep: String,
    },
    /// A service of the early boot graph depends on a service outside it.
    EarlyDependency {
        /// The early service.
        service: String,
        /// The dependency outside the early boot graph.
        dep: String,
    },
    /// The dependencies form a cycle, listed in order. The first and last
    /// names are the same, and each depends on the next.
    Cycle(Vec<String>),
    /// An environment variable cannot be passed to a program: its name is
    /// empty or contains `=` or a NUL byte, or its value contains a NUL
    /// byte.
    InvalidEnv {
        /// The service.
        service: String,
        /// The variable name.
        key: String,
        /// Why it is rejected.
        reason: &'static str,
    },
    /// Two services declare the same directory with different owners or
    /// modes, so each start would change what the other set.
    SharedDir {
        /// The first service, by name.
        first: String,
        /// The second service.
        second: String,
        /// The absolute directory path.
        path: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedInit(init) => write!(
                f,
                "init = {} is not implemented in this version of Laft. Only runit is supported",
                init.as_str()
            ),
            Self::InvalidName { service, reason } => {
                write!(f, "service {service:?}: invalid service name, {reason}")
            }
            Self::InvalidTty { tty, reason } => write!(f, "getty: tty {tty:?} {reason}"),
            Self::MissingCommand {
                service,
                kind,
                field,
            } => write!(
                f,
                "service {service}: a {} requires {field:?}",
                kind.as_str()
            ),
            Self::UnexpectedCommand {
                service,
                kind,
                field,
            } => write!(
                f,
                "service {service}: a {} cannot have {field:?}",
                kind.as_str()
            ),
            Self::RelativeCommand {
                service,
                field,
                program,
            } => write!(
                f,
                "service {service}: {field}[0] {program:?} is not an absolute path"
            ),
            Self::RelativePath {
                service,
                field,
                path,
            } => write!(
                f,
                "service {service}: {field} {path:?} is not an absolute path"
            ),
            Self::InvalidDirPath { service, path } => write!(
                f,
                "service {service}: invalid directory path {path:?}, expected relative \
                 components of A-Z, a-z, 0-9, '.', '_', '@', '+' and '-'"
            ),
            Self::OutOfRange {
                service,
                field,
                value,
                min,
                max,
            } => write!(
                f,
                "service {service}: {field} is {value}, expected {min} to {max}"
            ),
            Self::WrongKind { service, reason } => write!(f, "service {service}: {reason}"),
            Self::UnknownDependency {
                service,
                dep,
                suggestion,
            } => {
                write!(f, "service {service}: no service {dep:?}")?;
                if let Some(suggestion) = suggestion {
                    write!(f, " (did you mean {suggestion:?}?)")?;
                }
                Ok(())
            }
            Self::DisabledDependency { service, dep } => write!(
                f,
                "service {service}: after references {dep:?}, which is disabled"
            ),
            Self::EarlyDependency { service, dep } => write!(
                f,
                "service {service}: an early service cannot depend on {dep:?}, which is not early"
            ),
            Self::Cycle(cycle) => write!(
                f,
                "dependency cycle between services\n  {}",
                cycle.join(" -> ")
            ),
            Self::InvalidEnv {
                service,
                key,
                reason,
            } => write!(f, "service {service}: env {key:?} {reason}"),
            Self::SharedDir {
                first,
                second,
                path,
            } => write!(
                f,
                "services {first} and {second} both declare {path} with different owners or modes"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The enabled services of a generation, validated and resolved into specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSet {
    specs: BTreeMap<String, Spec>,
    packages: BTreeSet<Name>,
}

impl ServiceSet {
    /// Validates the declared services and resolves every enabled one,
    /// including the generated gettys, into its spec.
    ///
    /// `config.services` holds every declared service after all
    /// contributions are merged, so it includes the standard services of
    /// the base part, which end the early boot graph in [`EARLY`].
    ///
    /// ```
    /// use laft_svc::{EARLY, Service, ServiceConfig, ServiceSet};
    ///
    /// let mut config = ServiceConfig::default();
    /// config.getty.ttys.clear();
    /// let mut early = Service::oneshot(&["/usr/bin/true"]);
    /// early.early = true;
    /// config.services.insert(EARLY.into(), early);
    /// config.services.insert("web".into(), Service::longrun(&["/usr/bin/web"]));
    /// let set = ServiceSet::new(&config).expect("the services are valid");
    /// assert_eq!(set.specs()["web"].after, ["early"]);
    ///
    /// config.services.insert("bad".into(), Service::longrun(&["web"]));
    /// let errors = ServiceSet::new(&config).unwrap_err();
    /// assert_eq!(
    ///     errors[0].to_string(),
    ///     r#"service bad: exec[0] "web" is not an absolute path"#,
    /// );
    /// ```
    ///
    /// # Errors
    ///
    /// Returns every problem found, in a stable order.
    pub fn new(config: &ServiceConfig) -> Result<Self, Vec<ConfigError>> {
        let mut errors = Vec::new();
        if config.init != Init::Runit {
            errors.push(ConfigError::UnsupportedInit(config.init));
        }
        for name in config.services.keys() {
            if let Some(reason) = name_problem(name) {
                errors.push(ConfigError::InvalidName {
                    service: name.clone(),
                    reason,
                });
            }
        }
        let mut declared = config.services.clone();
        check_ttys(&config.getty.ttys, &mut errors);
        for (name, getty) in standard::getty_services(config) {
            match declared.entry(name) {
                Entry::Occupied(entry) => errors.push(ConfigError::InvalidName {
                    service: entry.key().clone(),
                    reason: "the name belongs to a generated getty".into(),
                }),
                Entry::Vacant(entry) => {
                    entry.insert(getty);
                }
            }
        }
        let enabled: BTreeMap<&str, &Service> = declared
            .iter()
            .filter(|(_, s)| s.enable)
            .map(|(n, s)| (n.as_str(), s))
            .collect();
        let mut specs = BTreeMap::new();
        for (&name, &service) in &enabled {
            check_service(name, service, &mut errors);
            let spec = resolve(name, service);
            check_after(&spec, &declared, &enabled, &mut errors);
            specs.insert(name.to_owned(), spec);
        }
        check_shared_dirs(&specs, &mut errors);
        let graph: BTreeMap<&str, Vec<&str>> = specs
            .iter()
            .map(|(n, s)| (n.as_str(), s.after.iter().map(String::as_str).collect()))
            .collect();
        if let Err(cycle) = graph::order(&graph, |_| 0) {
            errors.push(ConfigError::Cycle(cycle));
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let packages = enabled
            .values()
            .flat_map(|s| s.packages.iter().cloned())
            .collect();
        Ok(Self { specs, packages })
    }

    /// The spec of every enabled service, by name.
    pub fn specs(&self) -> &BTreeMap<String, Spec> {
        &self.specs
    }

    /// The packages that the enabled services add to the system view.
    pub fn packages(&self) -> &BTreeSet<Name> {
        &self.packages
    }
}

fn name_problem(name: &str) -> Option<String> {
    let well_formed = name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !well_formed {
        Some(format!(
            "expected [a-z][a-z0-9-]* of at most {MAX_NAME} characters"
        ))
    } else if name.ends_with("-log") {
        Some("names ending in -log are reserved for loggers".into())
    } else if name.starts_with("laft-") && name != ACTIVATE {
        Some("names starting with laft- are reserved".into())
    } else {
        None
    }
}

fn check_ttys(ttys: &[String], errors: &mut Vec<ConfigError>) {
    let mut seen = BTreeSet::new();
    for tty in ttys {
        let digits = tty
            .strip_prefix("ttyUSB")
            .or_else(|| tty.strip_prefix("ttyS"))
            .or_else(|| tty.strip_prefix("tty"));
        let valid = digits.is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        let reason = if !valid {
            "is not a terminal name like tty1, ttyS0 or ttyUSB0"
        } else if !seen.insert(tty) {
            "is listed twice"
        } else {
            continue;
        };
        errors.push(ConfigError::InvalidTty {
            tty: tty.clone(),
            reason,
        });
    }
}

fn check_service(name: &str, service: &Service, errors: &mut Vec<ConfigError>) {
    let service_name = || name.to_owned();
    type Fields<'a> = &'a [(&'static str, &'a Vec<String>)];
    let (required, forbidden): (Fields<'_>, Fields<'_>) = match service.kind {
        ServiceKind::Longrun => (
            &[("exec", &service.exec)],
            &[("up", &service.up), ("down", &service.down)],
        ),
        ServiceKind::Oneshot => (&[("up", &service.up)], &[("exec", &service.exec)]),
    };
    for &(field, argv) in required {
        if argv.is_empty() {
            errors.push(ConfigError::MissingCommand {
                service: service_name(),
                kind: service.kind,
                field,
            });
        }
    }
    for &(field, argv) in forbidden {
        if !argv.is_empty() {
            errors.push(ConfigError::UnexpectedCommand {
                service: service_name(),
                kind: service.kind,
                field,
            });
        }
    }
    let mut commands = vec![
        ("exec", &service.exec),
        ("up", &service.up),
        ("down", &service.down),
    ];
    if let Ready::Check { cmd, .. } = &service.ready {
        if cmd.is_empty() {
            errors.push(ConfigError::MissingCommand {
                service: service_name(),
                kind: service.kind,
                field: "ready.cmd",
            });
        }
        commands.push(("ready.cmd", cmd));
    }
    for (field, argv) in commands {
        if let Some(program) = argv.first().filter(|p| !p.starts_with('/')) {
            errors.push(ConfigError::RelativeCommand {
                service: service_name(),
                field,
                program: program.clone(),
            });
        }
    }
    if !service.cwd.starts_with('/') {
        errors.push(ConfigError::RelativePath {
            service: service_name(),
            field: "cwd",
            path: service.cwd.clone(),
        });
    }
    for dir in service.state_dirs.iter().chain(&service.runtime_dirs) {
        if !valid_dir_path(&dir.path) {
            errors.push(ConfigError::InvalidDirPath {
                service: service_name(),
                path: dir.path.clone(),
            });
        }
    }
    for (key, value) in &service.env {
        let reason = if key.is_empty() || key.contains(['=', '\0']) {
            "is not a valid variable name"
        } else if value.contains('\0') {
            "has a value with a NUL byte"
        } else {
            continue;
        };
        errors.push(ConfigError::InvalidEnv {
            service: service_name(),
            key: key.clone(),
            reason,
        });
    }
    check_ranges(name, service, errors);
    if service.kind == ServiceKind::Longrun && service.on_change == Some(OnChange::Rerun) {
        errors.push(ConfigError::WrongKind {
            service: service_name(),
            reason: "on_change = rerun applies only to oneshots",
        });
    }
    if service.kind == ServiceKind::Oneshot && service.ready != Ready::Immediate {
        errors.push(ConfigError::WrongKind {
            service: service_name(),
            reason: "a oneshot is ready when up succeeds, so ready must be immediate",
        });
    }
}

fn check_ranges(name: &str, service: &Service, errors: &mut Vec<ConfigError>) {
    const TIB: u64 = 1 << 40;
    let mut ranges: Vec<(&'static str, u64, u64, u64)> = vec![
        ("umask", service.umask.into(), 0, 0o777),
        ("restart_delay", service.restart_delay.into(), 0, 3600),
        ("stop_timeout", service.stop_timeout.into(), 1, 3600),
        ("rotate.size", service.rotate.size, 4096, 1 << 30),
        ("rotate.count", service.rotate.count.into(), 1, 1000),
    ];
    match &service.ready {
        Ready::Immediate => {}
        Ready::Notify { fd } => ranges.push(("ready.fd", (*fd).into(), 3, 1023)),
        Ready::Check {
            interval, timeout, ..
        } => {
            ranges.push(("ready.interval", (*interval).into(), 1, 3600));
            ranges.push(("ready.timeout", (*timeout).into(), 1, 86400));
        }
    }
    let limits = &service.limits;
    for (field, value, min, max) in [
        ("limits.nofile", limits.nofile, 1, 1_073_741_816),
        ("limits.nproc", limits.nproc, 1, 4_194_304),
        ("limits.core", limits.core, 0, TIB),
        ("limits.memlock", limits.memlock, 0, TIB),
    ] {
        if let Some(value) = value {
            ranges.push((field, value, min, max));
        }
    }
    for dir in service.state_dirs.iter().chain(&service.runtime_dirs) {
        ranges.push(("dir.mode", dir.mode.into(), 0, 0o7777));
    }
    for (field, value, min, max) in ranges {
        if !(min..=max).contains(&value) {
            errors.push(ConfigError::OutOfRange {
                service: name.to_owned(),
                field,
                value,
                min,
                max,
            });
        }
    }
}

fn valid_dir_path(path: &str) -> bool {
    path.split('/').all(|component| {
        !component.is_empty()
            && component != "."
            && component != ".."
            && component
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._@+-".contains(&b))
    })
}

fn check_after(
    spec: &Spec,
    declared: &BTreeMap<String, Service>,
    enabled: &BTreeMap<&str, &Service>,
    errors: &mut Vec<ConfigError>,
) {
    for dep in &spec.after {
        let service = spec.name.clone();
        if let Some(target) = enabled.get(dep.as_str()) {
            if spec.early && !target.early {
                errors.push(ConfigError::EarlyDependency {
                    service,
                    dep: dep.clone(),
                });
            }
        } else if declared.contains_key(dep) {
            errors.push(ConfigError::DisabledDependency {
                service,
                dep: dep.clone(),
            });
        } else {
            errors.push(ConfigError::UnknownDependency {
                service,
                dep: dep.clone(),
                suggestion: closest(dep, declared.keys()),
            });
        }
    }
}

/// The candidate within two edits of `name`, preferring fewer edits and
/// then the smaller name.
fn closest<'a>(name: &str, candidates: impl Iterator<Item = &'a String>) -> Option<String> {
    candidates
        .map(|c| (edit_distance(name, c), c))
        .filter(|&(d, _)| d <= 2)
        .min()
        .map(|(_, c)| c.clone())
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let substitute = diagonal + usize::from(ca != cb);
            diagonal = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()]
}

/// What a service makes of a declared directory: the owner, the group or
/// `None` for the owner's primary group, and the mode.
type DirSettings = (String, Option<String>, u32);

fn dir_settings(spec: &Spec, dir: &SpecDir) -> DirSettings {
    match &dir.owner {
        Some(owner) => (owner.clone(), dir.group.clone(), dir.mode),
        None => (
            spec.user.clone(),
            dir.group.clone().or_else(|| spec.group.clone()),
            dir.mode,
        ),
    }
}

fn check_shared_dirs(specs: &BTreeMap<String, Spec>, errors: &mut Vec<ConfigError>) {
    let mut seen: BTreeMap<&str, (&str, DirSettings)> = BTreeMap::new();
    let mut reported = BTreeSet::new();
    for (name, spec) in specs {
        for dir in spec.state_dirs.iter().chain(&spec.runtime_dirs) {
            let settings = dir_settings(spec, dir);
            match seen.get(dir.path.as_str()) {
                Some((first, first_settings))
                    if *first_settings != settings && reported.insert(dir.path.as_str()) =>
                {
                    errors.push(ConfigError::SharedDir {
                        first: (*first).to_owned(),
                        second: name.clone(),
                        path: dir.path.clone(),
                    });
                }
                Some(_) => {}
                None => {
                    seen.insert(&dir.path, (name, settings));
                }
            }
        }
    }
}

/// Resolves a declared service into its spec. Validation reports problems
/// separately, so this never fails.
pub(crate) fn resolve(name: &str, service: &Service) -> Spec {
    let mut after: BTreeSet<String> = service.after.iter().cloned().collect();
    if !service.early {
        after.insert(EARLY.to_owned());
    }
    let dirs = |dirs: &[Dir], base: &str| -> Vec<SpecDir> {
        dirs.iter()
            .map(|d| SpecDir {
                path: format!("{base}/{}", d.path),
                mode: d.mode,
                owner: d.owner.clone(),
                group: d.group.clone(),
            })
            .collect()
    };
    let ready = match &service.ready {
        Ready::Immediate => SpecReady::Immediate,
        Ready::Notify { fd } => SpecReady::Notify { fd: *fd },
        Ready::Check {
            cmd,
            interval,
            timeout,
        } => SpecReady::Check {
            cmd: cmd.clone(),
            interval_ms: u64::from(*interval) * 1000,
            timeout_ms: u64::from(*timeout) * 1000,
        },
    };
    // Early services run before any supervision exists, so nothing could
    // collect their output but the console.
    let log = match (service.early, service.log) {
        (true, _) | (false, Log::Console) => SpecLog::Console,
        (false, Log::Discard) => SpecLog::Discard,
        (false, Log::Logged) => SpecLog::Logged {
            dir: format!("/var/log/{name}"),
            size: service.rotate.size,
            count: service.rotate.count,
        },
    };
    let on_change = service.on_change.unwrap_or(match service.kind {
        ServiceKind::Longrun => OnChange::Restart,
        ServiceKind::Oneshot => OnChange::Rerun,
    });
    Spec {
        spec_version: SPEC_VERSION,
        name: name.to_owned(),
        kind: service.kind,
        early: service.early,
        exec: service.exec.clone(),
        up: service.up.clone(),
        down: service.down.clone(),
        user: service.user.clone(),
        group: service.group.clone(),
        groups: service.groups.iter().cloned().collect(),
        env: service.env.clone(),
        cwd: service.cwd.clone(),
        umask: service.umask,
        after: after.into_iter().collect(),
        ready,
        log,
        state_dirs: dirs(&service.state_dirs, "/var/lib"),
        runtime_dirs: dirs(&service.runtime_dirs, "/run"),
        restart: service.restart,
        restart_delay_ms: u64::from(service.restart_delay) * 1000,
        stop_signal: service.stop_signal.number(),
        stop_timeout_ms: u64::from(service.stop_timeout) * 1000,
        limits: SpecLimits {
            nofile: service.limits.nofile,
            nproc: service.limits.nproc,
            core: service.limits.core,
            memlock: service.limits.memlock,
        },
        on_change,
    }
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
