//! Converting the evaluated configuration into plain values, and finding
//! the source of each part of it.

use std::collections::{BTreeMap, BTreeSet};

use laft_activate::{BootConfig, EtcConfig, Fs, Group};
use laft_core::{Name, PkgRef, Variant};
use laft_index::PublicKey;
use laft_resolve::Request;
use laft_store::GcPolicy;
use laft_svc::ServiceConfig;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use stav::{
    AttrPath, Checked, Diagnostic, Evaluated, FileDiagnostic, FileId, SourceSpan, Span, Value,
};

use crate::{Gc, Repo, View, host};

/// The evaluated configuration, with the means to point diagnostics at its
/// sources.
pub(crate) struct Cx<'a> {
    checked: &'a Checked,
    evaluated: &'a Evaluated,
    /// The place that wrote each `Pkg` value, by its path in the value. An
    /// element of a list or set has its index as the last key.
    pkgs: BTreeMap<Vec<String>, SourceSpan>,
    /// The prelude and the base part, which diagnostics point into only
    /// when nothing else contributed.
    host_files: Vec<FileId>,
}

impl<'a> Cx<'a> {
    /// Returns the context of `evaluated`, the evaluation of `checked`.
    pub(crate) fn new(checked: &'a Checked, evaluated: &'a Evaluated) -> Self {
        let mut pkgs = BTreeMap::new();
        let mut spans = evaluated.symbols("Pkg").map(|(_, span)| span);
        collect_pkgs(evaluated.value(), &mut Vec::new(), &mut spans, &mut pkgs);
        let host_files = checked
            .files()
            .filter(|&(_, name)| name == host::PRELUDE_NAME || name == host::BASE_NAME)
            .map(|(file, _)| file)
            .collect();
        Self {
            checked,
            evaluated,
            pkgs,
            host_files,
        }
    }

    /// Returns the value at `path`.
    pub(crate) fn value(&self, path: &[&str]) -> Option<&'a Value> {
        self.evaluated
            .value()
            .at(&AttrPath::new(path.iter().copied()))
    }

    /// Converts the value at `path`, or records why it does not convert.
    pub(crate) fn get<T: DeserializeOwned>(
        &self,
        path: &[&str],
        found: &mut Vec<FileDiagnostic>,
    ) -> Option<T> {
        self.evaluated
            .get(&AttrPath::new(path.iter().copied()))
            .map_err(|e| found.extend(e.iter().cloned()))
            .ok()
    }

    /// Returns where the attribute at `path` was written: its first
    /// contribution outside the prelude and the base part, or else its
    /// first contribution, or else that of the nearest enclosing attribute.
    pub(crate) fn span(&self, path: &[&str]) -> Option<SourceSpan> {
        (0..=path.len()).rev().find_map(|n| {
            let spans = self
                .evaluated
                .spans(&AttrPath::new(path[..n].iter().copied()));
            let own = spans.iter().find(|s| !self.host_files.contains(&s.file));
            own.or(spans.first()).copied()
        })
    }

    /// Returns every `Pkg` value with the place that wrote it.
    pub(crate) fn symbols(&self) -> impl Iterator<Item = (&'a str, SourceSpan)> {
        self.evaluated.symbols("Pkg")
    }

    /// Returns where the `Pkg` value at `path` was written.
    pub(crate) fn pkg_span(&self, path: &[&str]) -> Option<SourceSpan> {
        let key: Vec<String> = path.iter().map(|&k| k.to_owned()).collect();
        self.pkgs.get(&key).copied()
    }

    /// Returns an error about the attribute at `path`.
    pub(crate) fn error(
        &self,
        code: &'static str,
        path: &[&str],
        message: impl Into<String>,
    ) -> FileDiagnostic {
        error_at(code, self.span(path), message)
    }

    /// Returns the place `span` names as `file:line:column`. A start inside
    /// a character names that character.
    pub(crate) fn location(&self, span: SourceSpan) -> String {
        let name = self
            .checked
            .files()
            .find(|&(file, _)| file == span.file)
            .map_or("<unknown>", |(_, name)| name);
        let text = self.checked.source(span.file).unwrap_or_default();
        let start = (0..=span.span.start.min(text.len()))
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        let before = text.get(..start).unwrap_or_default();
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or(before).chars().count() + 1;
        format!("{name}:{line}:{column}")
    }
}

/// Returns an error with `code` at `span`, or without a place when `span`
/// is `None`.
pub(crate) fn error_at(
    code: &'static str,
    span: Option<SourceSpan>,
    message: impl Into<String>,
) -> FileDiagnostic {
    FileDiagnostic {
        file: span.map(|s| s.file),
        diagnostic: Diagnostic::error(
            code,
            span.map_or(Span { start: 0, end: 0 }, |s| s.span),
            message,
        ),
    }
}

/// Returns `d` with the suggested fix `help`.
pub(crate) fn helped(mut d: FileDiagnostic, help: impl Into<String>) -> FileDiagnostic {
    d.diagnostic.help = Some(help.into());
    d
}

/// Pairs every `Pkg` value inside `value` with the next span of `spans`,
/// which lists the places of the `Pkg` values in the order of the value.
fn collect_pkgs(
    value: &Value,
    path: &mut Vec<String>,
    spans: &mut impl Iterator<Item = SourceSpan>,
    out: &mut BTreeMap<Vec<String>, SourceSpan>,
) {
    let mut visit = |key: String, value: &Value, spans: &mut _, out: &mut _| {
        path.push(key);
        collect_pkgs(value, path, spans, out);
        path.pop();
    };
    match value {
        Value::Symbol { ty, .. } if ty == "Pkg" => {
            if let Some(span) = spans.next() {
                out.insert(path.clone(), span);
            }
        }
        Value::List(items) | Value::Set(items) => {
            for (i, item) in items.iter().enumerate() {
                visit(i.to_string(), item, spans, out);
            }
        }
        Value::Map(fields)
        | Value::Struct { fields, .. }
        | Value::Variant {
            fields: Some(fields),
            ..
        } => {
            for (key, item) in fields {
                visit(key.clone(), item, spans, out);
            }
        }
        _ => {}
    }
}

/// Returns the entries of the map at `path`, or none.
pub(crate) fn entries<'a>(cx: &Cx<'a>, path: &[&str]) -> &'a [(String, Value)] {
    match cx.value(path) {
        Some(Value::Map(entries)) => entries,
        _ => &[],
    }
}

/// Returns the `Pkg` values of the set at `path` with their paths.
fn pkg_values<'a>(cx: &Cx<'a>, path: &[&str]) -> Vec<(Vec<String>, &'a str)> {
    let items = match cx.value(path) {
        Some(Value::Set(items) | Value::List(items)) => items.as_slice(),
        _ => &[],
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            Value::Symbol { text, .. } => {
                let mut key: Vec<String> = path.iter().map(|&k| k.to_owned()).collect();
                key.push(i.to_string());
                Some((key, text.as_str()))
            }
            _ => None,
        })
        .collect()
}

/// Returns the package requests of the set of `Pkg` at `path`, each with
/// the place that wrote it as its origin.
pub(crate) fn requests(cx: &Cx<'_>, path: &[&str]) -> Vec<Request> {
    pkg_values(cx, path)
        .into_iter()
        .filter_map(|(key, text)| {
            let pkg = text.parse::<PkgRef>().ok()?;
            let key: Vec<&str> = key.iter().map(String::as_str).collect();
            let origin = cx
                .pkg_span(&key)
                .map_or_else(|| key.join("."), |span| cx.location(span));
            Some(Request { pkg, origin })
        })
        .collect()
}

/// Returns the package name of `text` when it names a package without a
/// version or variant.
fn bare_name(text: &str) -> Option<Name> {
    let pkg = text.parse::<PkgRef>().ok()?;
    (pkg.prefix().is_none() && pkg.variant() == Variant::default()).then(|| pkg.name().clone())
}

/// Returns the alternatives choices of the map at `path`, after recording
/// every group that is not a name and every member that is not a bare
/// package name (`C0902`).
pub(crate) fn alternatives(
    cx: &Cx<'_>,
    path: &[&str],
    found: &mut Vec<FileDiagnostic>,
) -> BTreeMap<Name, Name> {
    let mut out = BTreeMap::new();
    for (group, member) in entries(cx, path) {
        let mut at = path.to_vec();
        at.push(group);
        let Ok(name) = group.parse::<Name>() else {
            found.push(cx.error(
                "C0902",
                &at,
                format!("alternatives group `{group}` is not a valid name"),
            ));
            continue;
        };
        let Value::Symbol { text, .. } = member else {
            continue;
        };
        match bare_name(text) {
            Some(member) => {
                out.insert(name, member);
            }
            None => {
                let span = cx.pkg_span(&at).or_else(|| cx.span(&at));
                found.push(helped(error_at(
                        "C0902",
                        span,
                        format!("the choice for alternatives group `{group}` is `{text}`, not a bare package name"),
                    ),
                    "an alternatives choice names a package without `@version` or `:variant`"),
                );
            }
        }
    }
    out
}

/// Converts the identity, accounts, files, kernel modules and sysctls, and
/// adds every implicit primary group to the declared groups.
pub(crate) fn etc(cx: &Cx<'_>, found: &mut Vec<FileDiagnostic>) -> Option<EtcConfig> {
    let mut etc: EtcConfig = cx.get(&["system"], found)?;
    for (name, user) in &etc.users.accounts {
        if user.group.is_none() && !etc.users.groups.contains_key(name) {
            let group = Group {
                system: user.system,
                ..Group::default()
            };
            etc.users.groups.insert(name.clone(), group);
        }
    }
    Some(etc)
}

/// Converts the init system, services and gettys, after recording every
/// service package that is not a bare package name (`C1209`).
pub(crate) fn services(cx: &Cx<'_>, found: &mut Vec<FileDiagnostic>) -> Option<ServiceConfig> {
    let mut valid = true;
    for (service, _) in entries(cx, &["system", "services"]) {
        for (key, text) in pkg_values(cx, &["system", "services", service, "packages"]) {
            if bare_name(text).is_none() {
                valid = false;
                let key: Vec<&str> = key.iter().map(String::as_str).collect();
                found.push(helped(
                    error_at(
                        "C1209",
                        cx.pkg_span(&key),
                        format!("service {service}: package `{text}` is not a bare package name"),
                    ),
                    "a service names its packages without `@version` or `:variant`",
                ));
            }
        }
    }
    if !valid {
        return None;
    }
    cx.get(&["system"], found)
}

/// The fields of a filesystem other than its type.
#[derive(Deserialize)]
struct FsFields {
    device: Option<String>,
    options: Vec<String>,
    fsck: Option<bool>,
}

/// Converts the boot settings, filesystems and LUKS devices.
pub(crate) fn boot(cx: &Cx<'_>, found: &mut Vec<FileDiagnostic>) -> Option<BootConfig> {
    let boot = cx.get(&["system", "boot"], found);
    let luks = cx.get(&["system", "luks"], found);
    let mut fs = BTreeMap::new();
    for (mountpoint, value) in entries(cx, &["system", "fs"]) {
        let fields: FsFields = cx.get(&["system", "fs", mountpoint], found)?;
        let fstype = match value.get("fstype") {
            Some(Value::Variant {
                name, fields: None, ..
            }) => name.clone(),
            Some(variant @ Value::Variant { .. }) => match variant.get("name") {
                Some(Value::Str(name)) => name.clone(),
                _ => continue,
            },
            _ => continue,
        };
        let entry = Fs {
            device: fields.device,
            fstype,
            options: fields.options,
            fsck: fields.fsck,
        };
        fs.insert(mountpoint.clone(), entry);
    }
    Some(BootConfig {
        boot: boot?,
        fs,
        luks: luks?,
    })
}

/// The fields of the retention policy.
#[derive(Deserialize)]
struct GcFields {
    keep_generations: u64,
    keep_days: u64,
    auto: bool,
    keep_downloads: bool,
}

/// Converts the retention policy.
pub(crate) fn gc(cx: &Cx<'_>, found: &mut Vec<FileDiagnostic>) -> Option<Gc> {
    let gc: GcFields = cx.get(&["system", "gc"], found)?;
    Some(Gc {
        policy: GcPolicy {
            keep_generations: gc.keep_generations,
            keep_days: gc.keep_days,
        },
        auto: gc.auto,
        keep_downloads: gc.keep_downloads,
    })
}

/// The fields of a view other than its packages and alternatives.
#[derive(Deserialize)]
struct ViewFields {
    inherit: Option<String>,
    users: BTreeSet<String>,
    groups: BTreeSet<String>,
}

/// Converts the views, after recording every view name that is not a
/// package name or is `system` (`C0903`) and every invalid alternatives
/// choice.
pub(crate) fn views(cx: &Cx<'_>, found: &mut Vec<FileDiagnostic>) -> BTreeMap<Name, View> {
    let mut out = BTreeMap::new();
    for (label, _) in entries(cx, &["system", "views"]) {
        let path = ["system", "views", label];
        let name = label
            .parse::<Name>()
            .ok()
            .filter(|n| n.as_str() != "system");
        let Some(name) = name else {
            found.push(cx.error(
                "C0903",
                &path,
                format!(
                    "`{label}` cannot name a view: it must be a package name other than `system`"
                ),
            ));
            continue;
        };
        let Some(fields) = cx.get::<ViewFields>(&path, found) else {
            continue;
        };
        let view = View {
            inherit: fields.inherit.is_some(),
            packages: requests(cx, &["system", "views", label, "packages"]),
            alternatives: alternatives(cx, &["system", "views", label, "alternatives"], found),
            users: fields.users,
            groups: fields.groups,
        };
        out.insert(name, view);
    }
    out
}

/// The fields of a repository as written.
#[derive(Deserialize)]
struct RepoFields {
    url: String,
    mirrors: Vec<String>,
    key: String,
    keys: Vec<String>,
    channel: String,
    priority: i64,
    enable: bool,
    allow_http: bool,
}

/// Converts the repositories, after recording every repository name that
/// is not a package name (`C0803`), every URL with a scheme the client does
/// not accept (`C0801`) and every key that is not a minisign public key
/// (`C0802`).
pub(crate) fn repos(cx: &Cx<'_>, found: &mut Vec<FileDiagnostic>) -> BTreeMap<Name, Repo> {
    let mut out = BTreeMap::new();
    for (label, _) in entries(cx, &["system", "repos"]) {
        let path = ["system", "repos", label];
        let Some(fields) = cx.get::<RepoFields>(&path, found) else {
            continue;
        };
        let errors = found.len();
        let name = label.parse::<Name>().map_err(|_| {
            found.push(cx.error(
                "C0803",
                &path,
                format!("`{label}` cannot name a repository: it must be a package name"),
            ));
        });
        let urls = std::iter::once(("url", &fields.url))
            .chain(fields.mirrors.iter().map(|m| ("mirrors", m)));
        for (field, url) in urls {
            let at = ["system", "repos", label, field];
            if url.starts_with("http://") && !fields.allow_http {
                found.push(helped(
                    cx.error("C0801", &at, format!("repo {label}: `{url}` uses http:// without allow_http")),
                    "use https://, or set `allow_http = true` for a repository on a trusted network"),
                );
            } else if !["https://", "http://", "file://"]
                .iter()
                .any(|s| url.starts_with(s))
            {
                found.push(cx.error(
                    "C0801",
                    &at,
                    format!("repo {label}: `{url}` has an unsupported scheme, expected https://, http:// or file://"),
                ));
            }
        }
        let mut key = |field: &str, text: &str| {
            text.parse::<PublicKey>()
                .map_err(|e| {
                    found.push(cx.error(
                        "C0802",
                        &["system", "repos", label, field],
                        format!("repo {label}: {e}"),
                    ));
                })
                .ok()
        };
        let main = key("key", &fields.key);
        let keys: Vec<Option<PublicKey>> = fields.keys.iter().map(|k| key("keys", k)).collect();
        if found.len() != errors {
            continue;
        }
        let (Ok(name), Some(main), Ok(channel)) = (name, main, fields.channel.parse::<Name>())
        else {
            continue;
        };
        let repo = Repo {
            url: fields.url,
            mirrors: fields.mirrors,
            key: main,
            keys: keys.into_iter().flatten().collect(),
            channel,
            priority: fields.priority,
            enable: fields.enable,
            allow_http: fields.allow_http,
        };
        out.insert(name, repo);
    }
    out
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
