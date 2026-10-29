//! `laft fmt`, `check`, `eval`, `options` and `explain`: the configuration
//! language tools with Laft's prelude and catalog loaded.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use laft_resolve::Upgrade;
use serde_json::json;
use stav::{AttrPath, Format, SchemaIndex};

use crate::cli::EvalFormat;
use crate::env::{Env, SyncMode};
use crate::failure::{Failure, exit};
use crate::resolve::{self, InstalledMetas};

/// The `.stav` files below `dir`, sorted, outside `secrets/`.
fn stav_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for item in fs::read_dir(dir)? {
        let path = item?.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "secrets") {
                stav_files(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "stav") {
            out.push(path);
        }
    }
    out.sort();
    Ok(())
}

/// `laft fmt`.
pub(crate) fn fmt(env: &Env, paths: &[PathBuf], check: bool, stdin: bool) -> Result<u8, Failure> {
    if stdin {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        return match stav::format(&text) {
            Ok(formatted) if check => Ok(if formatted == text { 0 } else { exit::FAILURE }),
            Ok(formatted) => {
                print_raw(&formatted);
                Ok(0)
            }
            Err(diagnostics) => {
                crate::report::emit_source(&env.ui, "<stdin>", &text, &diagnostics);
                Err(Failure::new(exit::CONFIG, "standard input does not parse"))
            }
        };
    }
    let mut files = paths.to_vec();
    if files.is_empty() {
        stav_files(&env.config_dir(), &mut files)?;
    }
    let mut changed = Vec::new();
    let mut broken = 0;
    for file in &files {
        let text = fs::read_to_string(file)
            .map_err(|e| Failure::general(format!("{}: {e}", file.display())))?;
        let formatted = match stav::format(&text) {
            Ok(formatted) => formatted,
            Err(diagnostics) => {
                crate::report::emit_source(
                    &env.ui,
                    &file.display().to_string(),
                    &text,
                    &diagnostics,
                );
                broken += 1;
                continue;
            }
        };
        if formatted == text {
            continue;
        }
        changed.push(file.display().to_string());
        if check {
            if env.ui.verbose > 0 && !env.ui.json {
                let name = file.display().to_string();
                env.ui
                    .out(crate::diff::unified(&name, &name, &text, &formatted));
            }
        } else {
            crate::fsx::replace(file, formatted.as_bytes())
                .map_err(|e| Failure::general(format!("{}: {e}", file.display())))?;
        }
    }
    if env.ui.json {
        env.ui
            .doc(json!({"files": files.len(), "changed": changed, "check": check}));
    } else {
        for file in &changed {
            env.ui.out(file);
        }
    }
    if broken > 0 {
        return Err(Failure::new(
            exit::CONFIG,
            format!(
                "{broken} file{} do not parse",
                if broken == 1 { "" } else { "s" }
            ),
        ));
    }
    Ok(if check && !changed.is_empty() {
        exit::FAILURE
    } else {
        0
    })
}

/// Writes `text` raw to standard output, ignoring a closed pipe.
fn print_raw(text: &str) {
    use std::io::Write;
    let _ = io::stdout().lock().write_all(text.as_bytes());
}

/// The directory and entry file of `file`, absolute.
fn split(file: &Path) -> Result<(PathBuf, PathBuf), Failure> {
    let file = fs::canonicalize(file)
        .map_err(|e| Failure::new(exit::CONFIG, format!("{}: {e}", file.display())))?;
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    Ok((dir, file))
}

/// `laft check`.
pub(crate) fn check(env: &Env, file: Option<&Path>, refresh: bool) -> Result<u8, Failure> {
    if let Some(file) = file {
        let (dir, file) = split(file)?;
        let evaluation = laft_config::evaluate(&dir, &file).map_err(Failure::config)?;
        if !evaluation.warnings.is_empty() && !env.ui.json {
            crate::report::emit(&env.ui, &evaluation.warnings);
        }
        report_valid(env, &file.display().to_string());
        return Ok(0);
    }
    let lock = if refresh {
        Some(env.lock("check")?)
    } else {
        None
    };
    let mode = if refresh {
        SyncMode::Always
    } else {
        SyncMode::Cached
    };
    let (loaded, indexes) = env.load_with_indexes(mode, lock.as_ref())?;
    drop(lock);
    let active = env.active()?.map(|n| env.generation(n)).transpose()?;
    let installed = InstalledMetas::of(&env.root, active.as_ref());
    let services = resolve::services(&loaded.config)?;
    let universe = resolve::universe(&indexes);
    let host = laft_activate::Host::detect(&loaded.config.boot);
    resolve::resolve(
        &universe,
        &loaded.config,
        &services,
        &installed,
        &Upgrade::None,
        &host,
    )?;
    report_valid(env, &env.config.display().to_string());
    Ok(0)
}

fn report_valid(env: &Env, what: &str) {
    if env.ui.json {
        env.ui.doc(json!({"valid": true, "file": what}));
    } else {
        env.ui.out(format!("{what} is valid"));
    }
}

/// `laft eval`.
pub(crate) fn eval(
    env: &Env,
    attr: Option<&str>,
    file: Option<&Path>,
    format: EvalFormat,
) -> Result<u8, Failure> {
    let (dir, entry) = match file {
        Some(file) => split(file)?,
        None => (env.config_dir(), env.config.clone()),
    };
    let evaluation = laft_config::evaluate(&dir, &entry).map_err(Failure::config)?;
    let mut keys = vec!["system".to_owned()];
    if let Some(attr) = attr.filter(|a| !a.is_empty()) {
        let path: AttrPath = attr
            .parse()
            .map_err(|e| Failure::new(exit::USAGE, format!("{attr}: {e}")))?;
        keys.extend(path.keys().iter().cloned());
    }
    let format = match format {
        EvalFormat::Json => Format::Json,
        EvalFormat::Toml => Format::Toml,
        EvalFormat::Yaml => Format::Yaml,
    };
    let text = evaluation
        .evaluated
        .render_at(&AttrPath::new(keys), format)
        .map_err(Failure::config)?;
    print_raw(&text);
    if !text.ends_with('\n') {
        print_raw("\n");
    }
    Ok(0)
}

/// One configuration option.
struct ConfigOption {
    path: String,
    ty: String,
    doc: Option<String>,
}

/// Every option reachable from the `System` struct, depth first.
fn options_of(schema: &SchemaIndex) -> Vec<ConfigOption> {
    fn walk(
        schema: &SchemaIndex,
        name: &str,
        prefix: &str,
        seen: &mut BTreeSet<String>,
        out: &mut Vec<ConfigOption>,
    ) {
        let Some(s) = schema.structs.iter().find(|s| s.name == name) else {
            return;
        };
        if !seen.insert(name.to_owned()) {
            return;
        }
        for field in &s.fields {
            let path = format!("{prefix}{}", field.name);
            out.push(ConfigOption {
                path: path.clone(),
                ty: field.ty.clone(),
                doc: field.doc.clone(),
            });
            let inner = field
                .ty
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .rfind(|word| schema.structs.iter().any(|s| s.name == *word));
            if let Some(inner) = inner {
                let label = if field.ty.starts_with("Map<") {
                    format!("{path}.<name>.")
                } else {
                    format!("{path}.")
                };
                walk(schema, inner, &label, seen, out);
            }
        }
        seen.remove(name);
    }
    let mut out = Vec::new();
    walk(schema, "System", "", &mut BTreeSet::new(), &mut out);
    out
}

/// `laft options`.
pub(crate) fn options(env: &Env, prefix: Option<&str>, all: bool) -> Result<u8, Failure> {
    let evaluation =
        laft_config::evaluate(&env.config_dir(), &env.config).map_err(Failure::config)?;
    let schema = evaluation.checked.schema();
    let options: Vec<ConfigOption> = options_of(&schema)
        .into_iter()
        .filter(|o| prefix.is_none_or(|p| o.path.starts_with(p)))
        .collect();
    if env.ui.json {
        let list: Vec<_> = options
            .iter()
            .map(|o| json!({"path": o.path, "type": o.ty, "doc": o.doc}))
            .collect();
        env.ui.doc(json!({"options": list}));
        return Ok(0);
    }
    let mut text = String::new();
    if all {
        for option in &options {
            text.push_str(&format!("{}  {}\n", option.path, option.ty));
            for line in option.doc.as_deref().unwrap_or_default().lines() {
                text.push_str(&format!("    {line}\n"));
            }
            text.push('\n');
        }
    } else {
        let rows: Vec<Vec<String>> = options
            .iter()
            .map(|o| {
                let first = o
                    .doc
                    .as_deref()
                    .and_then(|d| d.lines().next())
                    .unwrap_or_default();
                vec![o.path.clone(), o.ty.clone(), first.trim().to_owned()]
            })
            .collect();
        text = crate::ui::table(&[], &rows);
    }
    env.ui.page(&text);
    Ok(0)
}

/// `laft explain`.
pub(crate) fn explain(env: &Env, code: &str) -> Result<u8, Failure> {
    let text = crate::explain::explain(code)
        .ok_or_else(|| Failure::new(exit::USAGE, format!("no explanation for {code}")))?;
    if env.ui.json {
        env.ui
            .doc(json!({"code": code.to_ascii_uppercase(), "text": text}));
    } else {
        print_raw(&text);
    }
    Ok(0)
}

#[cfg(test)]
#[path = "lang_tests.rs"]
mod tests;
