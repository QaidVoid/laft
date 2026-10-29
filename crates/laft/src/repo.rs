//! `laft repo` and `laft view list` and `show`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use laft_core::{Name, Timestamp};
use serde_json::json;

use crate::env::{Env, SyncMode};
use crate::failure::{Failure, exit};
use crate::ui::{self, table};

fn names(texts: &[String]) -> Result<BTreeSet<Name>, Failure> {
    texts
        .iter()
        .map(|n| {
            n.parse()
                .map_err(|e| Failure::new(exit::USAGE, format!("{n}: {e}")))
        })
        .collect()
}

/// `laft repo sync`.
pub(crate) fn sync(env: &Env, wanted: &[String]) -> Result<u8, Failure> {
    let wanted = names(wanted)?;
    env.require_write("repo sync")?;
    env.ui.start("repo sync");
    let lock = env.lock("repo sync")?;
    let loaded = env.load(&|_| Ok(()))?;
    if let Some(unknown) = wanted
        .iter()
        .find(|n| !loaded.config.repos.contains_key(*n))
    {
        return Err(Failure::new(
            exit::USAGE,
            format!("no repository {unknown} is configured"),
        ));
    }
    let synced = env.indexes_of(&loaded.config, SyncMode::Always, Some(&lock), &|n| {
        wanted.is_empty() || wanted.contains(n)
    })?;
    let repos: Vec<_> = synced
        .iter()
        .map(|r| json!({"repo": r.name, "generation": r.accepted.record.generation}))
        .collect();
    env.ui.event(json!({"event": "result", "repos": repos}));
    if !env.ui.json {
        for repo in &synced {
            env.ui.out(format!(
                "{}  generation {}",
                repo.name, repo.accepted.record.generation
            ));
        }
    }
    Ok(0)
}

/// `laft repo list`.
pub(crate) fn list(env: &Env) -> Result<u8, Failure> {
    let loaded = env.load(&|_| Ok(()))?;
    let fetcher = env.fetcher();
    let state = env.index_state();
    let now = Timestamp::now();
    let mut rows = Vec::new();
    let mut docs = Vec::new();
    for (name, repo) in &loaded.config.repos {
        let trust = laft_index::Repo {
            name: name.clone(),
            channel: repo.channel.clone(),
            arch: crate::env::ARCH,
            key: repo.key.clone(),
            keys: repo.keys.clone(),
        };
        let mirrors = laft_fetch::Mirrors::new(&repo.url, &repo.mirrors, repo.allow_http)?;
        let request = laft_fetch::SyncRequest {
            repo: &trust,
            mirrors: &mirrors,
            state: &state,
            now,
            allow_expired: true,
        };
        let cached = fetcher.cached(&request);
        let status = match (&cached, repo.enable) {
            (_, false) => "disabled".to_owned(),
            (Ok(c), _) if c.index.expired => "expired".into(),
            (Ok(_), _) => "ok".into(),
            (Err(e), _) if matches!(e.kind(), laft_fetch::ErrorKind::NotFound) => {
                "not synced".into()
            }
            (Err(_), _) => "unusable".into(),
        };
        let index = cached.as_ref().ok().map(|c| &c.index.index);
        let expires = index.map(|i| {
            let left = i.expires.as_secs().saturating_sub(now.as_secs());
            ui::duration(std::time::Duration::from_secs(left))
        });
        docs.push(json!({
            "name": name, "url": repo.url, "channel": repo.channel, "priority": repo.priority,
            "enabled": repo.enable,
            "generation": index.map(|i| i.generation),
            "timestamp": index.map(|i| i.timestamp.to_string()),
            "expires": index.map(|i| i.expires.to_string()),
            "packages": index.map(|i| i.packages.len()),
            "status": status,
        }));
        rows.push(vec![
            name.to_string(),
            repo.channel.to_string(),
            repo.priority.to_string(),
            index.map_or_else(|| "-".into(), |i| i.generation.to_string()),
            index.map_or_else(|| "-".into(), |i| ui::age(i.timestamp, now)),
            expires.unwrap_or_else(|| "-".into()),
            index.map_or_else(|| "-".into(), |i| i.packages.len().to_string()),
            status,
        ]);
    }
    if env.ui.json {
        env.ui.doc(json!({"repos": docs}));
    } else {
        env.ui.out(
            table(
                &[
                    "NAME",
                    "CHANNEL",
                    "PRIO",
                    "GENERATION",
                    "AGE",
                    "EXPIRES",
                    "PKGS",
                    "STATUS",
                ],
                &rows,
            )
            .trim_end(),
        );
    }
    Ok(0)
}

/// `laft repo reset`.
pub(crate) fn reset(env: &Env, name: &str) -> Result<u8, Failure> {
    let name: Name = name
        .parse()
        .map_err(|e| Failure::new(exit::USAGE, format!("{name}: {e}")))?;
    env.require_write("repo reset")?;
    env.ui.start("repo reset");
    env.ui.note(format!(
        "repo reset forgets the accepted index and keys of {name}, so the next sync trusts what it finds like a first sync"
    ));
    env.ui.confirm("proceed?")?;
    let _lock = env.lock("repo reset")?;
    env.index_state().reset(&name)?;
    env.ui
        .event(json!({"event": "result", "repo": name, "reset": true}));
    env.ui.note(format!("done        {name} was reset"));
    Ok(0)
}

/// `laft view list`.
pub(crate) fn view_list(env: &Env) -> Result<u8, Failure> {
    let loaded = env.load(&|_| Ok(()))?;
    let generation = env.active()?.map(|n| env.generation(n)).transpose()?;
    let mut rows = Vec::new();
    let mut docs = Vec::new();
    for (name, view) in &loaded.config.views {
        let version = generation.as_ref().and_then(|g| g.views.get(name).copied());
        let manifest = version.and_then(|v| env.root.view(name, v).ok());
        let users: Vec<&String> = view.users.iter().collect();
        let groups: Vec<&String> = view.groups.iter().collect();
        docs.push(json!({"name": name, "version": version,
            "packages": manifest.as_ref().map(|m| m.packages.len()),
            "users": users, "groups": groups}));
        rows.push(vec![
            name.to_string(),
            version.map_or_else(|| "-".into(), |v| v.to_string()),
            manifest
                .as_ref()
                .map_or_else(|| "-".into(), |m| m.packages.len().to_string()),
            view.users.iter().cloned().collect::<Vec<_>>().join(","),
            view.groups.iter().cloned().collect::<Vec<_>>().join(","),
        ]);
    }
    if env.ui.json {
        env.ui.doc(json!({"views": docs}));
    } else {
        env.ui
            .out(table(&["VIEW", "VERSION", "PKGS", "USERS", "GROUPS"], &rows).trim_end());
    }
    Ok(0)
}

/// `laft view show`.
pub(crate) fn view_show(env: &Env, name: &str) -> Result<u8, Failure> {
    let name: Name = name
        .parse()
        .map_err(|e| Failure::new(exit::USAGE, format!("{name}: {e}")))?;
    let generation = env
        .active()?
        .map(|n| env.generation(n))
        .transpose()?
        .ok_or_else(|| Failure::general("no generation is active"))?;
    let version = generation.views.get(&name).ok_or_else(|| {
        Failure::general(format!(
            "view {name} is not in generation {}",
            generation.generation
        ))
    })?;
    let view = env.root.view(&name, *version)?;
    if env.ui.json {
        env.ui
            .doc(serde_json::to_value(&view).map_err(Failure::general)?);
        return Ok(0);
    }
    let system: std::collections::BTreeMap<&Name, &laft_store::PackageRecord> =
        generation.packages.iter().map(|p| (&p.name, p)).collect();
    let mut out = String::new();
    let _ = writeln!(out, "view        {name}@{version}");
    let _ = writeln!(out, "created     {}", ui::local_time(view.created));
    let _ = writeln!(out, "packages    {}", view.packages.len());
    let _ = writeln!(out, "users       {}", view.users.join(", "));
    let _ = writeln!(out, "groups      {}", view.groups.join(", "));
    for (group, member) in &view.alternatives {
        let _ = writeln!(out, "alternative {group} = {member}");
    }
    let differing: Vec<Vec<String>> = view
        .packages
        .iter()
        .filter(|p| system.get(&p.name).is_none_or(|s| s.id != p.id))
        .map(|p| {
            let in_system = system
                .get(&p.name)
                .map_or_else(|| "-".into(), |s| format!("{}-r{}", s.version, s.rev));
            vec![
                p.name.to_string(),
                format!("{}-r{}", p.version, p.rev),
                in_system,
            ]
        })
        .collect();
    if !differing.is_empty() {
        out.push('\n');
        out.push_str(&table(&["PACKAGE", "VIEW", "SYSTEM"], &differing));
    }
    env.ui.out(out.trim_end());
    Ok(0)
}
