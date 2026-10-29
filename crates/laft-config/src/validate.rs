//! The checks that follow evaluation, and the assembly of the result.

use laft_activate::{BootConfig, ConfigError as EtcError, EtcConfig, Setting, check_config};
use laft_core::{PkgRef, system_gid, system_uid};
use laft_svc::{ConfigError as ServiceError, ServiceConfig, ServiceSet, Warning};
use stav::{Diagnostic, FileDiagnostic, Value};

use crate::convert::{self, Cx, entries, error_at, helped};
use crate::{PkgError, SystemConfig};

/// Validates and converts the evaluated configuration of `cx`, resolving
/// every package reference with `resolve`. Every problem is added to
/// `found`. Returns the configuration when every part of it converted.
pub(crate) fn validate(
    cx: &Cx<'_>,
    resolve: &dyn Fn(&PkgRef) -> Result<(), PkgError>,
    found: &mut Vec<FileDiagnostic>,
) -> Option<SystemConfig> {
    packages(cx, resolve, found);
    init(cx, found);
    let etc = convert::etc(cx, found);
    let security = cx.get(&["system", "security"], found);
    if let (Some(etc), Some(security)) = (&etc, &security) {
        for error in check_config(etc, security) {
            found.extend(etc_error(cx, &error));
        }
    }
    let services = convert::services(cx, found);
    if let Some(services) = &services {
        check_services(cx, services, found);
    }
    let boot = convert::boot(cx, found);
    if let Some(boot) = &boot {
        check_boot(cx, boot, found);
    }
    let views = convert::views(cx, found);
    if let Some(etc) = &etc {
        check_view_members(cx, etc, found);
    }
    let config = SystemConfig {
        extra_locales: cx.get(&["system", "extra_locales"], found)?,
        session: cx.get(&["system", "session"], found)?,
        swap: cx.get(&["system", "swap"], found)?,
        packages: convert::requests(cx, &["system", "packages"]),
        alternatives: convert::alternatives(cx, &["system", "alternatives"], found),
        views,
        gc: convert::gc(cx, found)?,
        repos: convert::repos(cx, found),
        etc: etc?,
        boot: boot?,
        services: services?,
        security: security?,
    };
    Some(config)
}

/// Returns the path of the user or group `name`, by `kind`.
fn account<'a>(kind: &str, name: &'a str) -> [&'a str; 4] {
    let map = if kind == "user" { "accounts" } else { "groups" };
    ["system", "users", map, name]
}

/// Returns `d` with a secondary location at `path`, when it has a source.
fn labeled(cx: &Cx<'_>, mut d: FileDiagnostic, path: &[&str], message: &str) -> FileDiagnostic {
    if let Some(span) = cx.span(path) {
        d.diagnostic = d.diagnostic.with_label(Some(span.file), span.span, message);
    }
    d
}

/// Resolves every package reference (`C0901`).
fn packages(
    cx: &Cx<'_>,
    resolve: &dyn Fn(&PkgRef) -> Result<(), PkgError>,
    found: &mut Vec<FileDiagnostic>,
) {
    for (text, span) in cx.symbols() {
        let Ok(pkg) = text.parse::<PkgRef>() else {
            continue;
        };
        if let Err(e) = resolve(&pkg) {
            let mut d = error_at("C0901", Some(span), e.message);
            d.diagnostic.help = e.help;
            found.push(d);
        }
    }
}

/// Rejects the init backends this version does not implement (`C0450`).
fn init(cx: &Cx<'_>, found: &mut Vec<FileDiagnostic>) {
    let path = ["system", "init"];
    if let Some(Value::Variant { name, .. }) = cx.value(&path)
        && name != "runit"
    {
        let mut d = cx.error(
            "C0450",
            &path,
            format!("init backend `{name}` is not available in this version of Laft"),
        );
        d.diagnostic = d.diagnostic.with_help(
            "remove the line or write `init = runit`. Services are declared once and will work \
             unchanged on s6 and dinit when those backends exist.",
        );
        found.push(d);
    }
}

/// Returns the diagnostic of an error that `/etc` rendering, account
/// declaration or privileged files report, or `None` when another
/// diagnostic already covers it.
fn etc_error(cx: &Cx<'_>, error: &EtcError) -> Option<FileDiagnostic> {
    let declared_group = |name: &str| cx.value(&["system", "users", "groups", name]).is_some();
    let id = |kind: &str| if kind == "user" { "uid" } else { "gid" };
    let code = error.code().unwrap_or("C0490");
    let message = error.to_string();
    let d = match error {
        EtcError::InvalidName { kind, name } => {
            if *kind == "group" && !declared_group(name) {
                return None;
            }
            cx.error(code, &account(kind, name), message)
        }
        EtcError::PasswordSources(name) => {
            let d = cx.error(
                code,
                &["system", "users", "accounts", name, "password_file"],
                message,
            );
            labeled(
                cx,
                d,
                &["system", "users", "accounts", name, "password_hash"],
                "password_hash is set here",
            )
        }
        EtcError::ReservedId { kind, name, .. } | EtcError::RegistryId { kind, name, .. } => {
            let mut path = account(kind, name).to_vec();
            path.push(id(kind));
            cx.error(code, &path, message)
        }
        EtcError::DuplicateId { kind, names, .. } => {
            let mut second = account(kind, &names[1]).to_vec();
            second.push(id(kind));
            let mut first = account(kind, &names[0]).to_vec();
            first.push(id(kind));
            let d = cx.error(code, &second, message);
            labeled(cx, d, &first, "the same id is given here")
        }
        EtcError::SystemPrimaryGroup(name) => {
            let d = cx.error(
                code,
                &["system", "users", "accounts", name],
                format!("user {name:?} would take the system group {name:?} as its primary group"),
            );
            helped(d, "set group explicitly")
        }
        EtcError::InvalidPath(path)
        | EtcError::ReservedPath(path)
        | EtcError::Content(path)
        | EtcError::Structure { path, .. } => cx.error(code, &["system", "files", path], message),
        EtcError::EnvKey { path, key } => {
            cx.error(code, &["system", "files", path, "env", key], message)
        }
        EtcError::PathClash { path, file: parent } => {
            let (at, other) = if cx.value(&["system", "files", path]).is_some() {
                (path, parent)
            } else {
                (parent, path)
            };
            let d = cx.error(code, &["system", "files", at], message);
            labeled(
                cx,
                d,
                &["system", "files", other],
                "the other entry is declared here",
            )
        }
        EtcError::SysctlKey(key) | EtcError::SysctlValue(key) => {
            cx.error(code, &["system", "sysctl", key], message)
        }
        EtcError::BlacklistedModule(_) => {
            let d = cx.error(code, &["system", "kernel", "blacklist"], message);
            labeled(
                cx,
                d,
                &["system", "kernel", "modules"],
                "the module is loaded here",
            )
        }
        EtcError::PrivilegedPath(path) => {
            cx.error(code, &["system", "security", "privileged", path], message)
        }
        EtcError::Unprivileged(path) => helped(
            cx.error(code, &["system", "security", "privileged", path], message),
            "set `enable = false` to drop the entry, or give the file the setuid or setgid bit or capabilities",
        ),
        EtcError::InvalidCaps { path, .. } => cx.error(
            code,
            &["system", "security", "privileged", path, "caps"],
            message,
        ),
        EtcError::InvalidValue { setting, .. } => {
            let path = match setting {
                Setting::System(field) => vec!["system", field],
                Setting::User { name, field } => vec!["system", "users", "accounts", name, field],
                Setting::KernelOptions(module) => vec!["system", "kernel", "options", module],
                Setting::KernelModule(module) => {
                    let options = ["system", "kernel", "options", module];
                    if cx.value(&options).is_some() {
                        options.to_vec()
                    } else {
                        vec!["system", "kernel"]
                    }
                }
                Setting::File(path) => vec!["system", "files", path],
                Setting::Privileged(path) => vec!["system", "security", "privileged", path],
            };
            cx.error(code, &path, message)
        }
        EtcError::IdsExhausted { kind, name } => cx.error(code, &account(kind, name), message),
        EtcError::UnknownAccount { .. } => cx.error(code, &["system", "users"], message),
    };
    Some(d)
}

/// Checks the services with the rules of [`ServiceSet::new`].
fn check_services(cx: &Cx<'_>, services: &ServiceConfig, found: &mut Vec<FileDiagnostic>) {
    let set = match ServiceSet::new(services) {
        Ok(set) => set,
        Err(errors) => {
            for error in &errors {
                let message = error.to_string();
                let d = match error {
                    ServiceError::UnsupportedInit(_) => continue,
                    ServiceError::InvalidName { service: s, .. } => {
                        cx.error("C1201", &["system", "services", s], message)
                    }
                    ServiceError::InvalidTty { .. } => {
                        cx.error("C1202", &["system", "getty", "ttys"], message)
                    }
                    ServiceError::MissingCommand { service: s, .. }
                    | ServiceError::WrongKind { service: s, .. }
                    | ServiceError::InvalidDirPath { service: s, .. } => {
                        cx.error("C1203", &["system", "services", s], message)
                    }
                    ServiceError::UnexpectedCommand {
                        service: s,
                        field: f,
                        ..
                    } => cx.error("C1203", &["system", "services", s, f], message),
                    ServiceError::OutOfRange {
                        service: s,
                        field: f,
                        ..
                    } => {
                        let top = f.split_once('.').map_or(*f, |(top, _)| top);
                        cx.error("C1203", &["system", "services", s, top], message)
                    }
                    ServiceError::RelativeCommand {
                        service: s,
                        field: f,
                        ..
                    }
                    | ServiceError::RelativePath {
                        service: s,
                        field: f,
                        ..
                    } => cx.error("C1204", &["system", "services", s, f], message),
                    ServiceError::UnknownDependency {
                        service: s,
                        suggestion,
                        ..
                    } => {
                        let mut d = cx.error("C1205", &["system", "services", s, "after"], message);
                        d.diagnostic.help =
                            suggestion.as_ref().map(|s| format!("did you mean `{s}`?"));
                        d
                    }
                    ServiceError::DisabledDependency { service: s, .. }
                    | ServiceError::EarlyDependency { service: s, .. } => {
                        cx.error("C1205", &["system", "services", s, "after"], message)
                    }
                    ServiceError::Cycle(cycle) => {
                        let message =
                            format!("dependency cycle between services: {}", cycle.join(" -> "));
                        let first = cycle.first().map_or("", String::as_str);
                        let mut d =
                            cx.error("C1206", &["system", "services", first, "after"], message);
                        for name in cycle.iter().skip(1).take(cycle.len().saturating_sub(2)) {
                            d = labeled(
                                cx,
                                d,
                                &["system", "services", name, "after"],
                                &format!("`{name}` continues the cycle"),
                            );
                        }
                        d
                    }
                    ServiceError::InvalidEnv {
                        service: s, key, ..
                    } => cx.error("C1207", &["system", "services", s, "env", key], message),
                    ServiceError::SharedDir { first, second, .. } => {
                        let d = cx.error("C1208", &["system", "services", second], message);
                        labeled(
                            cx,
                            d,
                            &["system", "services", first],
                            "the other service is declared here",
                        )
                    }
                };
                found.push(d);
            }
            return;
        }
    };
    for warning in set.warnings() {
        let Warning::EarlyReserved(s) = warning;
        let d = cx.error(
            "C1210",
            &["system", "services", s, "early"],
            warning.to_string(),
        );
        found.push(FileDiagnostic {
            file: d.file,
            diagnostic: Diagnostic::warning(
                d.diagnostic.code,
                d.diagnostic.span,
                d.diagnostic.message,
            ),
        });
    }
}

/// Directories that Laft manages, which cannot be mountpoints or contain
/// one.
const MANAGED: [&str; 10] = [
    "/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/proc", "/sys", "/dev", "/run",
];

/// Filesystem types the initramfs can mount the root filesystem from and
/// that hold the hardlinks, owners and modes a system needs.
const ROOT_TYPES: [&str; 6] = ["ext2", "ext3", "ext4", "xfs", "btrfs", "f2fs"];

/// Checks the filesystems and boot settings (`C1101` to `C1109`).
fn check_boot(cx: &Cx<'_>, boot: &BootConfig, found: &mut Vec<FileDiagnostic>) {
    if !boot.fs.contains_key("/") {
        let d = cx.error("C1101", &["system"], "no root filesystem");
        found.push(helped(d, "declare fs \"/\" { ... }"));
    }
    for (mountpoint, entry) in &boot.fs {
        let field = |f| ["system", "fs", mountpoint, f];
        let normalized = mountpoint.starts_with('/')
            && (mountpoint == "/"
                || mountpoint[1..]
                    .split('/')
                    .all(|part| !matches!(part, "" | "." | "..")));
        if !normalized {
            found.push(cx.error(
                "C1102",
                &["system", "fs", mountpoint],
                format!("fs {mountpoint:?}: mountpoint must be an absolute normalized path"),
            ));
            continue;
        }
        let managed = MANAGED
            .iter()
            .find(|&&m| mountpoint == m || mountpoint.starts_with(&format!("{m}/")))
            .copied()
            .or_else(|| mountpoint.starts_with("/laft/").then_some("/laft"));
        if let Some(managed) = managed {
            found.push(cx.error(
                "C1103",
                &["system", "fs", mountpoint],
                format!(
                    "fs {mountpoint:?}: {managed} is managed by Laft and cannot hold a mountpoint"
                ),
            ));
        }
        if mountpoint == "/" && !ROOT_TYPES.contains(&entry.fstype.as_str()) {
            found.push(cx.error(
                "C1104",
                &field("fstype"),
                format!(
                    "fs \"/\": root filesystem type {:?} is not supported, expected one of {}",
                    entry.fstype,
                    ROOT_TYPES.join(", ")
                ),
            ));
        }
        let at_boot = mountpoint == "/" || mountpoint == "/laft" || *mountpoint == boot.boot.esp;
        match &entry.device {
            Some(device) if at_boot && !boot_device(device) => found.push(cx.error(
                "C1105",
                &field("device"),
                format!(
                    "fs {mountpoint:?}: laft-boot resolves only UUID=, kernel device names and /dev/mapper/<luks>, got {device:?}"
                ),
            )),
            Some(_) => {}
            None if entry.fstype != "tmpfs" => found.push(cx.error(
                "C1109",
                &["system", "fs", mountpoint],
                format!(
                    "fs {mountpoint:?}: device is required for fstype {}",
                    entry.fstype
                ),
            )),
            None => {}
        }
        if let Some(name) = entry
            .device
            .as_deref()
            .and_then(|d| d.strip_prefix("/dev/mapper/"))
            && !boot.luks.contains_key(name)
        {
            found.push(cx.error(
                "C1108",
                &field("device"),
                format!("fs {mountpoint:?}: /dev/mapper/{name} has no luks {name:?} block"),
            ));
        }
        if mountpoint == "/" && entry.options.iter().any(|o| o == "ro") {
            found.push(cx.error(
                "C1106",
                &field("options"),
                "fs \"/\": the root filesystem must be mounted read-write",
            ));
        }
    }
    let esp = &boot.boot.esp;
    if !boot.fs.get(esp).is_some_and(|fs| fs.fstype == "vfat") {
        found.push(cx.error(
            "C1107",
            &["system", "boot", "esp"],
            format!("boot.esp is {esp:?} but there is no fs {esp:?} of type vfat"),
        ));
    }
    for (name, luks) in &boot.luks {
        if !device_name(name) {
            found.push(cx.error(
                "C1105",
                &["system", "luks", name],
                format!(
                    "luks {name:?}: a device-mapper name is one path component other than . and .."
                ),
            ));
        }
        if luks.device.starts_with("/dev/mapper/") || !boot_device(&luks.device) {
            found.push(cx.error(
                "C1105",
                &["system", "luks", name, "device"],
                format!(
                    "luks {name:?}: laft-boot unlocks only UUID= and kernel device names, got {:?}",
                    luks.device
                ),
            ));
        }
    }
}

/// Whether `name` can follow `/dev/` or `/dev/mapper/`: one path component
/// other than `.` and `..`.
fn device_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && name != "." && name != ".."
}

/// Whether laft-boot can resolve `device`: `UUID=` and printable ASCII
/// without spaces, or `/dev/<name>` or `/dev/mapper/<name>` with a
/// [`device_name`].
fn boot_device(device: &str) -> bool {
    match device.strip_prefix("UUID=") {
        Some(uuid) => !uuid.is_empty() && uuid.bytes().all(|b| b.is_ascii_graphic()),
        None => device
            .strip_prefix("/dev/mapper/")
            .or_else(|| device.strip_prefix("/dev/"))
            .is_some_and(device_name),
    }
}

/// Checks that every user and group a view names is declared (`C0904`).
fn check_view_members(cx: &Cx<'_>, etc: &EtcConfig, found: &mut Vec<FileDiagnostic>) {
    for (view, value) in entries(cx, &["system", "views"]) {
        let names = |field| match value.get(field) {
            Some(Value::Set(items)) => items
                .iter()
                .filter_map(|v| match v {
                    Value::Str(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for user in names("users") {
            if !etc.users.accounts.contains_key(user) && system_uid(user).is_none() {
                found.push(cx.error(
                    "C0904",
                    &["system", "views", view, "users"],
                    format!("view {view}: user {user:?} is not declared"),
                ));
            }
        }
        for group in names("groups") {
            if !etc.users.groups.contains_key(group) && system_gid(group).is_none() {
                found.push(cx.error(
                    "C0904",
                    &["system", "views", view, "groups"],
                    format!("view {view}: group {group:?} is not declared"),
                ));
            }
        }
    }
}
