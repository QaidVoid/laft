//! `laft search`, `laft info`, `laft files`, `laft owns` and `laft why`:
//! questions about packages that only read.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::fs;

use laft_core::{Name, PkgRef, Variant};
use laft_index::IndexEntry;
use laft_pkg::{Entry, Manifest, Reader};
use laft_store::{Generation, PackageRecord, Reason};
use serde_json::{Value, json};

use crate::env::{Env, RepoIndex, SyncMode};
use crate::failure::{Failure, exit};
use crate::resolve::SYSTEM;
use crate::ui::{self, table};

/// The cached indexes of the configured repositories.
fn cached_indexes(env: &Env) -> Result<Vec<RepoIndex>, Failure> {
    let loaded = env.load(&|_| Ok(()))?;
    env.indexes(&loaded.config, SyncMode::Cached, None)
}

/// The active generation, if any.
fn active(env: &Env) -> Result<Option<Generation>, Failure> {
    env.active()?.map(|n| env.generation(n)).transpose()
}

fn parse_ref(text: &str) -> Result<PkgRef, Failure> {
    text.parse()
        .map_err(|e| Failure::new(exit::USAGE, format!("{text}: {e}")))
}

/// `laft search`.
pub(crate) fn search(
    env: &Env,
    query: &str,
    repo: Option<&str>,
    all_versions: bool,
) -> Result<u8, Failure> {
    let indexes = cached_indexes(env)?;
    let installed: BTreeMap<Name, Variant> = active(env)?
        .map(|g| {
            g.packages
                .iter()
                .map(|p| (p.name.clone(), p.variant))
                .collect()
        })
        .unwrap_or_default();
    let needle = query.to_lowercase();
    let mut found: BTreeMap<(u8, Name), Vec<(&RepoIndex, &IndexEntry)>> = BTreeMap::new();
    for index in indexes
        .iter()
        .filter(|r| repo.is_none_or(|n| r.name.as_str() == n))
    {
        for entry in &index.accepted.index.packages {
            let name = entry.meta.name.as_str();
            let rank = if name == needle {
                0
            } else if name.starts_with(&needle) {
                1
            } else if name.contains(&needle) {
                2
            } else if entry.meta.summary.to_lowercase().contains(&needle) {
                3
            } else {
                continue;
            };
            found
                .entry((rank, entry.meta.name.clone()))
                .or_default()
                .push((index, entry));
        }
    }
    let mut rows = Vec::new();
    let mut docs = Vec::new();
    for ((_, name), mut entries) in found {
        entries.sort_by(|a, b| {
            (&b.1.meta.version, b.1.meta.rev).cmp(&(&a.1.meta.version, a.1.meta.rev))
        });
        let newest = (&entries[0].1.meta.version, entries[0].1.meta.rev);
        let mut versions: Vec<(&laft_core::Version, u32)> = entries
            .iter()
            .map(|(_, e)| (&e.meta.version, e.meta.rev))
            .collect();
        versions.dedup();
        if !all_versions {
            versions.truncate(1);
        }
        for (version, rev) in versions {
            let at: Vec<&(&RepoIndex, &IndexEntry)> = entries
                .iter()
                .filter(|(_, e)| (&e.meta.version, e.meta.rev) == (version, rev))
                .collect();
            let variants: BTreeSet<String> =
                at.iter().map(|(_, e)| e.meta.variant.to_string()).collect();
            let (index, entry) = at[0];
            let installed = installed.get(&name).filter(|_| (version, rev) == newest);
            let summary = &entry.meta.summary;
            docs.push(json!({
                "name": name,
                "version": format!("{version}-r{rev}"),
                "variants": variants,
                "repo": index.name,
                "summary": summary,
                "installed": installed.map(ToString::to_string),
            }));
            let mut summary = summary.clone();
            if let Some(variant) = installed {
                summary.push_str(&format!("  [installed: {variant}]"));
            }
            rows.push(vec![
                name.to_string(),
                format!("{version}-r{rev}"),
                variants.into_iter().collect::<Vec<_>>().join(" "),
                index.name.to_string(),
                summary,
            ]);
        }
    }
    if env.ui.json {
        env.ui.doc(json!({"results": docs}));
        return Ok(0);
    }
    let mut text = table(&[], &rows);
    if let Some((columns, _)) = ui::terminal_size() {
        text = text
            .lines()
            .map(|l| ui::truncate(l, columns, env.ui.ascii))
            .collect::<Vec<_>>()
            .join("\n");
        text.push('\n');
    }
    env.ui.page(&text);
    Ok(0)
}

/// The index entries that `pkg` names, best first.
fn matching<'a>(indexes: &'a [RepoIndex], pkg: &PkgRef) -> Vec<(&'a RepoIndex, &'a IndexEntry)> {
    let mut found: Vec<(&RepoIndex, &IndexEntry)> = indexes
        .iter()
        .flat_map(|r| r.accepted.index.packages.iter().map(move |e| (r, e)))
        .filter(|(_, e)| &e.meta.name == pkg.name())
        .filter(|(_, e)| e.meta.variant == pkg.variant())
        .filter(|(_, e)| pkg.prefix().is_none_or(|p| p.matches(&e.meta.version)))
        .collect();
    found.sort_by(|a, b| {
        (b.0.repo.priority, &b.1.meta.version, b.1.meta.rev).cmp(&(
            a.0.repo.priority,
            &a.1.meta.version,
            a.1.meta.rev,
        ))
    });
    found
}

/// The views of the active generation that hold `name`, with the record.
fn installed_in(
    env: &Env,
    generation: Option<&Generation>,
    name: &Name,
) -> Vec<(String, PackageRecord)> {
    let Some(generation) = generation else {
        return Vec::new();
    };
    let mut out: Vec<(String, PackageRecord)> = generation
        .packages
        .iter()
        .filter(|p| &p.name == name)
        .map(|p| (SYSTEM.to_owned(), p.clone()))
        .collect();
    for (view, version) in &generation.views {
        if let Ok(manifest) = env.root.view(view, *version) {
            out.extend(
                manifest
                    .packages
                    .into_iter()
                    .filter(|p| &p.name == name)
                    .map(|p| (view.to_string(), p)),
            );
        }
    }
    out
}

/// `laft info`.
pub(crate) fn info(env: &Env, text: &str) -> Result<u8, Failure> {
    let pkg = parse_ref(text)?;
    let indexes = cached_indexes(env)?;
    let found = matching(&indexes, &pkg);
    let generation = active(env)?;
    let views = installed_in(env, generation.as_ref(), pkg.name());
    let Some((index, entry)) = found.first() else {
        return Err(Failure::general(format!("no package matches {pkg}")));
    };
    let meta = &entry.meta;
    let variants: BTreeSet<String> = indexes
        .iter()
        .flat_map(|r| r.accepted.index.packages.iter())
        .filter(|e| e.meta.name == meta.name && e.meta.version == meta.version)
        .map(|e| e.meta.variant.to_string())
        .collect();
    if env.ui.json {
        let mut value = serde_json::to_value(entry).map_err(Failure::general)?;
        if let Value::Object(map) = &mut value {
            map.insert("repo".into(), json!(index.name));
            let names: Vec<&String> = views.iter().map(|(v, _)| v).collect();
            map.insert("installed_in".into(), json!(names));
        }
        env.ui.doc(value);
        return Ok(0);
    }
    let mut out = String::new();
    let mut line = |key: &str, value: String| {
        if !value.is_empty() {
            let _ = writeln!(out, "{key:<13}{value}");
        }
    };
    let join = |items: Vec<String>| items.join(", ");
    line("name", meta.name.to_string());
    line("version", format!("{}-r{}", meta.version, meta.rev));
    line("variants", join(variants.into_iter().collect()));
    line("repo", index.name.to_string());
    line("summary", meta.summary.clone());
    line("homepage", meta.homepage.clone().unwrap_or_default());
    line("license", join(meta.license.clone()));
    line(
        "depends",
        join(meta.depends.iter().map(ToString::to_string).collect()),
    );
    line(
        "provides",
        join(meta.provides.iter().map(ToString::to_string).collect()),
    );
    line(
        "alternatives",
        join(
            meta.alternatives
                .iter()
                .map(|a| a.group.to_string())
                .collect(),
        ),
    );
    line(
        "triggers",
        join(meta.triggers.iter().map(ToString::to_string).collect()),
    );
    line("download", ui::size(entry.size));
    line("installed", ui::size(meta.installed_size));
    line("store id", entry.id.to_string());
    line(
        "views",
        join(
            views
                .iter()
                .map(|(view, p)| format!("{view} ({}-r{})", p.version, p.rev))
                .collect(),
        ),
    );
    env.ui.out(out.trim_end());
    Ok(0)
}

/// The manifest of the package `pkg` names: from the store when it holds
/// the package, otherwise from the package file.
fn manifest_of(env: &Env, pkg: &PkgRef) -> Result<(String, Manifest), Failure> {
    let generation = active(env)?;
    let installed = installed_in(env, generation.as_ref(), pkg.name())
        .into_iter()
        .map(|(_, p)| p)
        .find(|p| p.variant == pkg.variant() && pkg.prefix().is_none_or(|x| x.matches(&p.version)));
    let store = env.root.store();
    if let Some(record) = installed {
        let label = format!("{} {}-r{}", record.name, record.version, record.rev);
        return Ok((label, store.manifest(&record.dir)?));
    }
    let indexes = cached_indexes(env)?;
    let found = matching(&indexes, pkg);
    let Some((index, entry)) = found.first() else {
        return Err(Failure::general(format!("no package matches {pkg}")));
    };
    let label = format!(
        "{} {}-r{}",
        entry.meta.name, entry.meta.version, entry.meta.rev
    );
    let dir = entry
        .id
        .dir_name(&entry.meta.name, &entry.meta.version, entry.meta.variant);
    if store.path(&dir).exists() {
        return Ok((label, store.manifest(&dir)?));
    }
    // The command only reads, so a package that is not cached is fetched
    // into a private directory instead of the shared cache.
    let cached = env
        .path("/laft/cache/pkgs")
        .join(format!("{}.lpkg", entry.id));
    let private = tempfile::tempdir()?;
    let path = if cached.is_file() {
        cached
    } else {
        let mut config = laft_fetch::Config::new(private.path());
        config.offline = env.offline;
        laft_fetch::Fetcher::new(config).download_package(&index.mirrors, entry, &mut |_| {})?
    };
    let file = fs::File::open(&path)?;
    let reader = Reader::new(file).map_err(|e| Failure::new(exit::NETWORK, e))?;
    Ok((label, reader.manifest().clone()))
}

/// `laft files`.
pub(crate) fn files(env: &Env, text: &str, long: bool) -> Result<u8, Failure> {
    let pkg = parse_ref(text)?;
    let (label, manifest) = manifest_of(env, &pkg)?;
    let describe = |entry: &Entry| -> (String, &'static str, u32, String, u64) {
        let owner = |o: &Option<String>, g: &Option<String>| {
            format!(
                "{}:{}",
                o.as_deref().unwrap_or("root"),
                g.as_deref().unwrap_or("root")
            )
        };
        match entry {
            Entry::Dir {
                path,
                mode,
                owner: o,
                group,
            } => (format!("/{path}/"), "dir", *mode, owner(o, group), 0),
            Entry::File {
                path,
                mode,
                size,
                owner: o,
                group,
                ..
            } => (format!("/{path}"), "file", *mode, owner(o, group), *size),
            Entry::Symlink { path, target } => (
                format!("/{path} -> {target}"),
                "link",
                0o777,
                "root:root".into(),
                0,
            ),
        }
    };
    if env.ui.json {
        let entries: Vec<Value> = manifest
            .entries()
            .iter()
            .map(|e| {
                let (path, kind, mode, owner, size) = describe(e);
                json!({"path": path, "type": kind, "mode": format!("{mode:o}"), "owner": owner, "size": size})
            })
            .collect();
        env.ui.doc(json!({"package": label, "files": entries}));
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = manifest
        .entries()
        .iter()
        .map(|e| {
            let (path, kind, mode, owner, size) = describe(e);
            if long {
                vec![
                    kind.into(),
                    format!("{mode:04o}"),
                    owner,
                    size.to_string(),
                    path,
                ]
            } else {
                vec![path]
            }
        })
        .collect();
    env.ui.page(&table(&[], &rows));
    Ok(0)
}

/// The generation and the tree directory of view `view` in it.
fn view_tree(
    env: &Env,
    generation: &Generation,
    view: &str,
) -> Result<(Vec<PackageRecord>, std::path::PathBuf), Failure> {
    if view == SYSTEM {
        let dir = env
            .root
            .path()
            .join(format!("laft/gens/{}", generation.generation));
        return Ok((generation.packages.clone(), dir));
    }
    let name: Name = view
        .parse()
        .map_err(|e| Failure::new(exit::USAGE, format!("{view}: {e}")))?;
    let version = generation.views.get(&name).ok_or_else(|| {
        Failure::general(format!(
            "view {view} is not in generation {}",
            generation.generation
        ))
    })?;
    let manifest = env.root.view(&name, *version)?;
    let dir = env.root.path().join(format!("laft/views/{view}/{version}"));
    Ok((manifest.packages, dir))
}

/// `laft owns`.
pub(crate) fn owns(env: &Env, target: &str, view: Option<&str>) -> Result<u8, Failure> {
    let path = if target.contains('/') {
        target.to_owned()
    } else {
        format!("/usr/bin/{target}")
    };
    let rel = path
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_owned();
    let generation = active(env)?.ok_or_else(|| Failure::general("no generation is active"))?;
    let view = view.unwrap_or(SYSTEM);
    let (packages, dir) = view_tree(env, &generation, view)?;
    let store = env.root.store();
    let mut answer = None;
    if rel.starts_with("etc/") && view == SYSTEM {
        answer = etc_owner(env, &generation, &rel, &packages)?;
    } else {
        for package in &packages {
            let manifest = store.manifest(&package.dir)?;
            if manifest.entries().iter().any(|e| e.path() == rel) {
                answer = Some(json!({"kind": "package", "package": package.name,
                    "version": format!("{}-r{}", package.version, package.rev),
                    "variant": package.variant.to_string()}));
                break;
            }
        }
        if answer.is_none() {
            answer = trigger_owner(&dir, &rel);
        }
    }
    let Some(answer) = answer else {
        return Err(Failure::general(format!(
            "nothing in view {view} provides {path}"
        )));
    };
    if env.ui.json {
        env.ui.doc(json!({"path": path, "owner": answer}));
        return Ok(0);
    }
    let text = match answer["kind"].as_str() {
        Some("package") => {
            let variant = answer["variant"].as_str().unwrap_or_default();
            format!(
                "{path} is owned by {} {} ({variant})",
                answer["package"].as_str().unwrap_or_default(),
                answer["version"].as_str().unwrap_or_default()
            )
        }
        Some("trigger") => format!(
            "{path} is generated by trigger {}",
            answer["trigger"].as_str().unwrap_or_default()
        ),
        Some("file") => format!(
            "{path} is managed, from file \"{path}\" at {}",
            answer["location"].as_str().unwrap_or("the configuration")
        ),
        Some("default") => format!(
            "{path} is managed, the default of {}",
            answer["package"].as_str().unwrap_or_default()
        ),
        _ => format!("{path} is managed, generated by laft"),
    };
    env.ui.out(text);
    Ok(0)
}

/// Which trigger instance produced `rel` in the tree `dir`, from its
/// trigger records.
fn trigger_owner(dir: &std::path::Path, rel: &str) -> Option<Value> {
    let candidates = [dir.join("triggers.json"), dir.join("view.json")];
    for path in candidates {
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let records = if path.ends_with("view.json") {
            value["triggers"].clone()
        } else {
            value["records"].clone()
        };
        if let Some(map) = records.as_object() {
            for (instance, record) in map {
                let outputs = record["outputs"].as_array();
                if outputs.is_some_and(|o| o.iter().any(|p| p.as_str() == Some(rel))) {
                    let name = instance.split(':').next().unwrap_or(instance);
                    return Some(json!({"kind": "trigger", "trigger": name, "instance": instance}));
                }
            }
        }
    }
    None
}

/// Where the managed path `rel` below `etc/` of `generation` comes from.
fn etc_owner(
    env: &Env,
    generation: &Generation,
    rel: &str,
    packages: &[PackageRecord],
) -> Result<Option<Value>, Failure> {
    let inside = rel.trim_start_matches("etc/");
    let managed = generation.etc.managed.iter().any(|p| p == inside)
        || generation.etc.copy.iter().any(|p| p == inside);
    if !managed {
        return Ok(None);
    }
    if let Some(location) = file_block_location(env, &format!("/{rel}")) {
        return Ok(Some(json!({"kind": "file", "location": location})));
    }
    let store = env.root.store();
    for package in packages {
        if store
            .manifest(&package.dir)?
            .entries()
            .iter()
            .any(|e| e.path() == rel)
        {
            return Ok(Some(json!({"kind": "default", "package": package.name})));
        }
    }
    Ok(Some(json!({"kind": "generated"})))
}

/// The source location of the `file` block for the absolute path `path`
/// in the configuration, as `file:line:column`.
fn file_block_location(env: &Env, path: &str) -> Option<String> {
    let dir = env.config_dir();
    let evaluation = laft_config::evaluate(&dir, &env.config).ok()?;
    let attr = stav::AttrPath::new(["system", "files", path]);
    let span = evaluation.evaluated.spans(&attr).into_iter().next()?;
    let name = evaluation
        .checked
        .files()
        .find(|(id, _)| *id == span.file)
        .map(|(_, n)| n.to_owned())?;
    let text = evaluation.checked.source(span.file)?;
    let before = text.get(..span.span.start)?;
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    Some(format!("{name}:{line}:{column}"))
}

/// `laft why`.
pub(crate) fn why(env: &Env, name: &str, view: Option<&str>, all: bool) -> Result<u8, Failure> {
    let name: Name = name
        .parse()
        .map_err(|e| Failure::new(exit::USAGE, format!("{name}: {e}")))?;
    let generation = active(env)?.ok_or_else(|| Failure::general("no generation is active"))?;
    let mut views = vec![view.unwrap_or(SYSTEM).to_owned()];
    if all {
        views = std::iter::once(SYSTEM.to_owned())
            .chain(generation.views.keys().map(ToString::to_string))
            .collect();
    }
    let mut docs = Vec::new();
    let mut out = String::new();
    for view in views {
        let (packages, _) = view_tree(env, &generation, &view)?;
        let by_name: BTreeMap<&Name, &PackageRecord> =
            packages.iter().map(|p| (&p.name, p)).collect();
        let Some(record) = by_name.get(&name) else {
            if !all {
                return Err(Failure::general(format!("{name} is not in view {view}")));
            }
            continue;
        };
        let chain = chain(&by_name, &name);
        let _ = writeln!(
            out,
            "{} {}-r{} is in view \"{view}\" because:",
            record.name, record.version, record.rev
        );
        let mut indent = String::from("  ");
        let mut steps = Vec::new();
        for (package, reason) in &chain {
            match reason {
                Reason::Dependency { by, dep } => {
                    let by_record = by_name.get(by);
                    let version = by_record
                        .map_or_else(String::new, |p| format!(" {}-r{}", p.version, p.rev));
                    let _ = writeln!(out, "{indent}{by}{version} needs {dep}");
                    steps.push(json!({"package": package, "by": by, "dep": dep}));
                }
                Reason::Requested { origin, text } => {
                    let _ = writeln!(out, "{indent}requested at {origin} ({text})");
                    steps.push(json!({"package": package, "requested": text, "origin": origin}));
                }
            }
            indent.push_str("  ");
        }
        docs.push(json!({"view": view, "package": record.name,
            "version": format!("{}-r{}", record.version, record.rev), "chain": steps,
            "reasons": record.reasons}));
    }
    if env.ui.json {
        env.ui.doc(json!({"why": docs}));
    } else {
        env.ui.out(out.trim_end());
    }
    Ok(0)
}

/// The shortest chain of reasons from `name` back to a request.
fn chain<'a>(
    packages: &BTreeMap<&Name, &'a PackageRecord>,
    name: &Name,
) -> Vec<(Name, &'a Reason)> {
    let mut queue = VecDeque::from([(name.clone(), Vec::new())]);
    let mut seen = BTreeSet::from([name.clone()]);
    while let Some((current, path)) = queue.pop_front() {
        let Some(record) = packages.get(&current) else {
            continue;
        };
        if let Some(reason) = record
            .reasons
            .iter()
            .find(|r| matches!(r, Reason::Requested { .. }))
        {
            let mut path: Vec<(Name, &Reason)> = path;
            path.push((current, reason));
            return path;
        }
        for reason in &record.reasons {
            if let Reason::Dependency { by, .. } = reason
                && seen.insert(by.clone())
            {
                let mut next = path.clone();
                next.push((current.clone(), reason));
                queue.push_back((by.clone(), next));
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
