//! `laft etc status`, `diff` and `reset`.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde_json::json;

use crate::diff;
use crate::env::Env;
use crate::failure::{Failure, exit};

/// How a path of `/etc` differs from the active generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Drift {
    /// The path, relative to `/etc`.
    pub(crate) path: String,
    /// What differs.
    pub(crate) problem: &'static str,
}

/// The paths of `/etc` that differ from generation `number`.
pub(crate) fn drift(env: &Env, number: u64) -> Result<Vec<Drift>, Failure> {
    let generation = env.generation(number)?;
    let static_dir = static_dir(env, number);
    let mut out = Vec::new();
    for path in &generation.etc.managed {
        let live = env.path("/etc").join(path);
        let problem = match fs::symlink_metadata(&live) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => "missing",
            Err(e) => return Err(Failure::general(format!("{}: {e}", live.display()))),
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&live)?;
                if target != PathBuf::from("/etc/static").join(path) {
                    "a symlink to somewhere else"
                } else if fs::symlink_metadata(static_dir.join(path)).is_err() {
                    "dangling"
                } else {
                    continue;
                }
            }
            Ok(_) => "replaced by the user",
        };
        out.push(Drift {
            path: path.clone(),
            problem,
        });
    }
    for path in &generation.etc.copy {
        if ["passwd", "group", "shadow", "gshadow"].contains(&path.as_str()) {
            continue;
        }
        let live = fs::read(env.path("/etc").join(path)).ok();
        let intended = fs::read(static_dir.join(path)).ok();
        if live != intended {
            out.push(Drift {
                path: path.clone(),
                problem: if live.is_none() {
                    "missing"
                } else {
                    "changed outside laft"
                },
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn static_dir(env: &Env, number: u64) -> PathBuf {
    env.root.path().join(format!("laft/gens/{number}/etc"))
}

fn active(env: &Env) -> Result<u64, Failure> {
    env.active()?
        .ok_or_else(|| Failure::general("no generation is active"))
}

fn relative(path: &str) -> String {
    path.trim_start_matches("/etc/")
        .trim_start_matches('/')
        .to_owned()
}

/// `laft etc status`.
pub(crate) fn status(env: &Env) -> Result<u8, Failure> {
    let drift = drift(env, active(env)?)?;
    if env.ui.json {
        let paths: Vec<_> = drift
            .iter()
            .map(|d| json!({"path": format!("/etc/{}", d.path), "problem": d.problem}))
            .collect();
        env.ui.doc(json!({"paths": paths}));
    } else if drift.is_empty() {
        env.ui.out("/etc matches the active generation");
    } else {
        let rows: Vec<Vec<String>> = drift
            .iter()
            .map(|d| vec![format!("/etc/{}", d.path), d.problem.to_owned()])
            .collect();
        env.ui.out(crate::ui::table(&[], &rows).trim_end());
    }
    Ok(0)
}

/// `laft etc diff`.
pub(crate) fn diff(env: &Env, paths: &[String]) -> Result<u8, Failure> {
    let number = active(env)?;
    let selected: Vec<String> = if paths.is_empty() {
        drift(env, number)?.into_iter().map(|d| d.path).collect()
    } else {
        paths.iter().map(|p| relative(p)).collect()
    };
    let static_dir = static_dir(env, number);
    let mut out = String::new();
    let mut docs = Vec::new();
    for path in selected {
        let live = env.path("/etc").join(&path);
        let live_text = match fs::symlink_metadata(&live) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&live)?;
                if target == PathBuf::from("/etc/static").join(&path) {
                    continue;
                }
                format!("symlink to {}\n", target.display())
            }
            Ok(_) => fs::read_to_string(&live).unwrap_or_default(),
            Err(_) => String::new(),
        };
        let intended = fs::read_to_string(static_dir.join(&path)).unwrap_or_default();
        let text = diff::unified(
            &format!("/etc/{path}"),
            &format!("generation {number}"),
            &live_text,
            &intended,
        );
        if !text.is_empty() {
            docs.push(json!({"path": format!("/etc/{path}"), "diff": text}));
            out.push_str(&text);
        }
    }
    if env.ui.json {
        env.ui.doc(json!({"generation": number, "diffs": docs}));
    } else {
        env.ui.page(&out);
    }
    Ok(0)
}

/// `laft etc reset`.
pub(crate) fn reset(env: &Env, paths: &[String]) -> Result<u8, Failure> {
    env.require_write("etc reset")?;
    if paths.is_empty() {
        return Err(Failure::new(exit::USAGE, "name the paths to reset"));
    }
    env.ui.start("etc reset");
    env.ui.confirm(&format!("reset {}?", paths.join(", ")))?;
    let lock = env.lock("etc reset")?;
    let kept = laft_activate::reset_etc(&lock, paths)?;
    for path in &kept {
        env.ui.note(format!(
            "kept the previous /etc/{path} as /etc/{path}.laft-old"
        ));
    }
    env.ui
        .event(json!({"event": "result", "reset": paths, "kept": kept}));
    env.ui
        .note(format!("done        reset {}", paths.join(", ")));
    Ok(0)
}
