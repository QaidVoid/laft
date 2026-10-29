//! `laft verify` and `laft doctor`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use laft_activate::{BootRecord, PrivilegedRecord};
use laft_core::{Blake3Hash, FileCapabilities, PkgRef, Timestamp};
use laft_pkg::Entry;
use laft_store::{Generation, PackageRecord};
use serde_json::{Value, json};

use crate::cli::VerifyArgs;
use crate::env::{Env, SyncMode};
use crate::failure::{Failure, exit};
use crate::gens;
use crate::ui::Sym;

/// The extended attribute that holds file capabilities.
const CAPABILITY: &str = "security.capability";

/// One difference that verification found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Problem {
    /// What it is about: a package or a generation.
    subject: String,
    /// The path.
    path: String,
    /// What differs.
    problem: String,
}

/// `laft verify`.
pub(crate) fn verify(env: &Env, args: &VerifyArgs) -> Result<u8, Failure> {
    let store = env.root.store();
    let numbers: Vec<u64> = if args.all {
        gens::generations(env)?
            .iter()
            .map(|g| g.generation)
            .collect()
    } else {
        let number = args
            .generation
            .or(env.active()?)
            .ok_or_else(|| Failure::general("no generation is active"))?;
        vec![number]
    };
    let generations: Vec<Generation> = numbers
        .iter()
        .map(|n| env.generation(*n))
        .collect::<Result<_, _>>()?;
    let records: Vec<&PackageRecord> = generations
        .iter()
        .flat_map(|g| g.packages.iter().chain(&g.initrd_packages))
        .collect();
    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    if args.all {
        for dir in store.list()? {
            let label = store.meta(&dir).map_or_else(
                |_| dir.clone(),
                |m| format!("{} {}-r{}", m.name, m.version, m.rev),
            );
            entries.insert(dir, label);
        }
    } else if args.pkgs.is_empty() {
        for record in &records {
            entries.insert(record.dir.clone(), label(record));
        }
    } else {
        for text in &args.pkgs {
            let pkg: PkgRef = text
                .parse()
                .map_err(|e| Failure::new(exit::USAGE, format!("{text}: {e}")))?;
            let found: Vec<&&PackageRecord> = records
                .iter()
                .filter(|r| &r.name == pkg.name() && r.variant == pkg.variant())
                .filter(|r| pkg.prefix().is_none_or(|p| p.matches(&r.version)))
                .collect();
            if found.is_empty() {
                return Err(Failure::new(
                    exit::USAGE,
                    format!("{pkg} is not in generation {}", numbers[0]),
                ));
            }
            for record in found {
                entries.insert(record.dir.clone(), label(record));
            }
        }
    }

    let mut problems = Vec::new();
    for (dir, label) in &entries {
        for difference in store.verify(dir, args.full)? {
            problems.push(Problem {
                subject: label.clone(),
                path: difference.path,
                problem: difference.problem,
            });
        }
    }
    for generation in &generations {
        let dir = gens::gen_dir(env, generation.generation);
        let subject = format!("generation {}", generation.generation);
        let privileged = privileged(env, generation, &dir, &subject, &mut problems)?;
        unexpected_privileges(
            &dir.join("usr"),
            "usr",
            &privileged,
            &subject,
            &mut problems,
        )?;
        if args.links {
            links(env, generation, &dir, &privileged, &subject, &mut problems)?;
        }
    }

    let failing: BTreeSet<&str> = problems.iter().map(|p| p.subject.as_str()).collect();
    let passed = entries
        .values()
        .filter(|l| !failing.contains(l.as_str()))
        .count();
    if env.ui.json {
        let list: Vec<Value> = problems
            .iter()
            .map(|p| json!({"subject": p.subject, "path": p.path, "problem": p.problem}))
            .collect();
        env.ui
            .doc(json!({"generations": numbers, "entries": entries.len(),
            "problems": list, "ok": problems.is_empty()}));
    } else {
        for problem in &problems {
            env.ui.out(format!(
                "{} {}  {}: {}",
                env.ui.sym(Sym::Fail),
                problem.subject,
                problem.path,
                problem.problem
            ));
        }
        let other = if problems.is_empty() { "" } else { "other " };
        env.ui.out(format!(
            "{} {passed} {other}package{}",
            env.ui.sym(Sym::Pass),
            if passed == 1 { "" } else { "s" }
        ));
    }
    Ok(if problems.is_empty() {
        0
    } else {
        exit::PROBLEMS
    })
}

fn label(record: &PackageRecord) -> String {
    format!("{} {}-r{}", record.name, record.version, record.rev)
}

/// Checks the privileged copies of a generation against `privileged.json`
/// and returns their paths relative to the generation directory.
fn privileged(
    env: &Env,
    generation: &Generation,
    dir: &Path,
    subject: &str,
    problems: &mut Vec<Problem>,
) -> Result<BTreeSet<String>, Failure> {
    let record = match PrivilegedRecord::read(dir) {
        Ok(record) => record,
        Err(_) if !dir.join(laft_activate::PRIVILEGED_FILE).exists() => {
            return Ok(BTreeSet::new());
        }
        Err(e) => return Err(e.into()),
    };
    let store = env.root.store();
    let mut paths = BTreeSet::new();
    for file in &record.files {
        let rel = file.path.trim_start_matches('/').to_owned();
        paths.insert(rel.clone());
        let mut report = |problem: String| {
            problems.push(Problem {
                subject: subject.to_owned(),
                path: file.path.clone(),
                problem,
            });
        };
        let path = dir.join(&rel);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) => {
                report(format!("privileged copy is unreadable: {e}"));
                continue;
            }
        };
        if meta.uid() != file.uid || meta.gid() != file.gid {
            report(format!(
                "owner is {}:{}, privileged.json says {}:{}",
                meta.uid(),
                meta.gid(),
                file.uid,
                file.gid
            ));
        }
        if meta.mode() & 0o7777 != file.mode {
            report(format!(
                "mode is {:o}, privileged.json says {:o}",
                meta.mode() & 0o7777,
                file.mode
            ));
        }
        let expected = file
            .caps
            .as_deref()
            .and_then(|c| c.parse::<FileCapabilities>().ok())
            .map(|c| c.to_xattr().to_vec());
        if capabilities(&path)? != expected {
            report("capabilities differ from privileged.json".into());
        }
        let source = generation
            .packages
            .iter()
            .find(|p| p.id == file.source)
            .map(|p| store.path(&p.dir).join(&rel));
        let same = source
            .as_ref()
            .and_then(|s| fs::read(s).ok())
            .zip(fs::read(&path).ok())
            .is_some_and(|(a, b)| Blake3Hash::of(&a) == Blake3Hash::of(&b));
        if !same {
            report("content differs from the store file it was copied from".into());
        }
    }
    Ok(paths)
}

/// The capabilities of `path`, as the raw extended attribute.
fn capabilities(path: &Path) -> Result<Option<Vec<u8>>, Failure> {
    let mut buf = [0u8; 64];
    match rustix::fs::lgetxattr(path, CAPABILITY, &mut buf) {
        Ok(len) => Ok(Some(buf[..len].to_vec())),
        Err(rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP) => Ok(None),
        Err(e) => Err(Failure::general(format!("{}: {e}", path.display()))),
    }
}

/// Reports every setuid, setgid or capability carrying file below `dir`
/// that is not a privileged copy.
fn unexpected_privileges(
    dir: &Path,
    rel: &str,
    privileged: &BTreeSet<String>,
    subject: &str,
    problems: &mut Vec<Problem>,
) -> Result<(), Failure> {
    let items = match fs::read_dir(dir) {
        Ok(items) => items,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Failure::general(format!("{}: {e}", dir.display()))),
    };
    for item in items {
        let item = item?;
        let path = item.path();
        let name = format!("{rel}/{}", item.file_name().to_string_lossy());
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            unexpected_privileges(&path, &name, privileged, subject, problems)?;
        } else if meta.is_file() && !privileged.contains(&name) {
            if meta.mode() & 0o6000 != 0 {
                problems.push(Problem {
                    subject: subject.to_owned(),
                    path: format!("/{name}"),
                    problem: "setuid or setgid file that privileged.json does not list".into(),
                });
            }
            if capabilities(&path)?.is_some() {
                problems.push(Problem {
                    subject: subject.to_owned(),
                    path: format!("/{name}"),
                    problem: "file capabilities that privileged.json does not list".into(),
                });
            }
        }
    }
    Ok(())
}

/// Reports files of the generation's system view that are not the store
/// file of a package that ships the path.
fn links(
    env: &Env,
    generation: &Generation,
    dir: &Path,
    privileged: &BTreeSet<String>,
    subject: &str,
    problems: &mut Vec<Problem>,
) -> Result<(), Failure> {
    let store = env.root.store();
    let mut inodes: BTreeMap<String, BTreeSet<(u64, u64)>> = BTreeMap::new();
    for package in &generation.packages {
        for entry in store.manifest(&package.dir)?.entries() {
            if let Entry::File { path, .. } = entry
                && path.starts_with("usr/")
                && let Ok(meta) = fs::metadata(store.path(&package.dir).join(path))
            {
                inodes
                    .entry(path.clone())
                    .or_default()
                    .insert((meta.dev(), meta.ino()));
            }
        }
    }
    for (path, owners) in &inodes {
        if privileged.contains(path) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(dir.join(path)) else {
            continue;
        };
        if !owners.contains(&(meta.dev(), meta.ino())) {
            problems.push(Problem {
                subject: subject.to_owned(),
                path: format!("/{path}"),
                problem: "not linked to its store file".into(),
            });
        }
    }
    Ok(())
}

/// The outcome of one health check.
struct Check {
    name: &'static str,
    /// `None` when the check does not apply, otherwise whether it passed.
    passed: Option<bool>,
    detail: String,
}

fn check(name: &'static str, passed: bool, detail: impl Into<String>) -> Check {
    Check {
        name,
        passed: Some(passed),
        detail: detail.into(),
    }
}

fn skip(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        passed: None,
        detail: detail.into(),
    }
}

/// `laft doctor`.
pub(crate) fn doctor(env: &Env) -> Result<u8, Failure> {
    let live = env.live();
    let mut checks = vec![
        layout(env),
        etc_static(env),
        run_state(env, live),
        laft_mount(live),
        generation_mounts(env, live),
        journal(env),
        tmp(env),
        space(env),
        indexes(env),
        boot_files(env),
        static_binaries(env),
    ];
    checks.push(clock(env));
    let failed = checks.iter().filter(|c| c.passed == Some(false)).count();
    if env.ui.json {
        let list: Vec<Value> = checks
            .iter()
            .map(|c| json!({"check": c.name, "passed": c.passed, "detail": c.detail}))
            .collect();
        env.ui.doc(json!({"checks": list, "ok": failed == 0}));
    } else {
        let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
        for c in &checks {
            let mark = match c.passed {
                Some(true) => env.ui.sym(Sym::Pass),
                Some(false) => env.ui.sym(Sym::Fail),
                None => env.ui.sym(Sym::Unchanged),
            };
            let line = format!("{mark} {:<width$}  {}", c.name, c.detail);
            env.ui.out(line.trim_end());
        }
    }
    Ok(if failed == 0 { 0 } else { exit::PROBLEMS })
}

fn layout(env: &Env) -> Check {
    let expected = [
        ("bin", "usr/bin"),
        ("sbin", "usr/bin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib"),
        ("usr", "/run/laft/system/usr"),
    ];
    let wrong: Vec<String> = expected
        .iter()
        .filter(|(link, target)| {
            fs::read_link(env.path(link)).ok().as_deref() != Some(Path::new(target))
        })
        .map(|(link, target)| format!("/{link} -> {target}"))
        .collect();
    if wrong.is_empty() {
        check("layout", true, "")
    } else {
        check(
            "layout",
            false,
            format!("missing or wrong: {}", wrong.join(", ")),
        )
    }
}

fn etc_static(env: &Env) -> Check {
    let target = fs::read_link(env.path("/etc/static")).ok();
    let ok = target.as_deref() == Some(Path::new("/run/laft/system/etc"));
    check(
        "etc static",
        ok,
        if ok {
            ""
        } else {
            "/etc/static must link to /run/laft/system/etc"
        },
    )
}

fn run_state(env: &Env, live: bool) -> Check {
    if !live {
        return skip("run state", "no system runs below the root");
    }
    let valid = |link: &str| {
        crate::env::link_number(&env.path(link))
            .ok()
            .flatten()
            .is_some_and(|n| env.root.generation(n).is_ok())
    };
    let ok = valid("/run/laft/system") && valid("/run/laft/booted");
    check(
        "run state",
        ok,
        if ok {
            ""
        } else {
            "/run/laft/system or /run/laft/booted does not name a valid generation"
        },
    )
}

/// The mounts of this process: mountpoint and options, from
/// `/proc/self/mountinfo`.
fn mounts() -> Vec<(String, BTreeSet<String>)> {
    let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let point = fields.get(4)?.replace("\\040", " ");
            let options = fields.get(5)?.split(',').map(str::to_owned).collect();
            Some((point, options))
        })
        .collect()
}

fn laft_mount(live: bool) -> Check {
    if !live {
        return skip("laft mount", "only checked on the running system");
    }
    let found = mounts().into_iter().rev().find(|(p, _)| p == "/laft");
    match found {
        Some((_, options)) => {
            let ok = ["ro", "nosuid", "nodev"]
                .iter()
                .all(|o| options.contains(*o));
            check(
                "laft mount",
                ok,
                if ok {
                    ""
                } else {
                    "/laft must be mounted ro,nosuid,nodev"
                },
            )
        }
        None => check("laft mount", false, "/laft is not a mountpoint"),
    }
}

fn generation_mounts(env: &Env, live: bool) -> Check {
    if !live {
        return skip("generation mounts", "only checked on the running system");
    }
    let wanted: BTreeSet<String> = ["/run/laft/system", "/run/laft/booted"]
        .iter()
        .filter_map(|l| fs::read_link(env.path(l)).ok())
        .map(|t| format!("/run/laft/{}", t.display()))
        .collect();
    let mut problems = Vec::new();
    for (point, options) in mounts() {
        if !point.starts_with("/run/laft/mounts/") {
            continue;
        }
        if !options.contains("ro") || !options.contains("nodev") || options.contains("nosuid") {
            problems.push(format!("{point} is not ro,nodev without nosuid"));
        }
        if !wanted.contains(&point) {
            problems.push(format!("{point} is neither system nor booted"));
        }
    }
    check(
        "generation mounts",
        problems.is_empty(),
        problems.join("; "),
    )
}

fn journal(env: &Env) -> Check {
    let pending = fs::read_dir(env.path("/laft/state/journal"))
        .map(|d| {
            d.filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .count()
        })
        .unwrap_or(0);
    check(
        "journal",
        pending == 0,
        if pending == 0 {
            String::new()
        } else {
            format!("{pending} interrupted transaction waits for recovery")
        },
    )
}

/// When the running system booted.
fn boot_time() -> Option<SystemTime> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    let secs: u64 = text
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

fn tmp(env: &Env) -> Check {
    let Some(booted) = boot_time() else {
        return skip("tmp", "the boot time is unknown");
    };
    let old = fs::read_dir(env.path("/laft/tmp"))
        .map(|d| {
            d.filter_map(Result::ok)
                .filter(|e| {
                    e.metadata()
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t < booted)
                })
                .count()
        })
        .unwrap_or(0);
    check(
        "tmp",
        old == 0,
        if old == 0 {
            String::new()
        } else {
            format!("/laft/tmp holds {old} entries older than this boot")
        },
    )
}

fn free_share(path: &Path) -> Option<f64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    (stat.f_blocks > 0).then(|| stat.f_bavail as f64 / stat.f_blocks as f64)
}

fn space(env: &Env) -> Check {
    let mut low = Vec::new();
    for path in ["/laft", "/boot"] {
        if let Some(share) = free_share(&env.path(path))
            && share < 0.10
        {
            low.push(format!("{path} has {:.0}% free", share * 100.0));
        }
    }
    check("space", low.is_empty(), low.join(", "))
}

fn indexes(env: &Env) -> Check {
    let Ok(loaded) = env.load(&|_| Ok(())) else {
        return skip("indexes", "the configuration does not load");
    };
    let Ok(indexes) = env.indexes(&loaded.config, SyncMode::Cached, None) else {
        return check("indexes", false, "an index is not synced");
    };
    let soon = Timestamp::now().as_secs() + 3 * 86_400;
    let expiring: Vec<String> = indexes
        .iter()
        .filter(|r| r.accepted.index.expires.as_secs() < soon)
        .map(|r| r.name.to_string())
        .collect();
    check(
        "indexes",
        expiring.is_empty(),
        if expiring.is_empty() {
            String::new()
        } else {
            format!("expiring within 3 days: {}", expiring.join(", "))
        },
    )
}

fn boot_files(env: &Env) -> Check {
    let esp = env.esp_dir().join("laft");
    let mut missing = Vec::new();
    let generations = gens::generations(env).unwrap_or_default();
    for generation in &generations {
        let Ok(record) = BootRecord::read(&gens::gen_dir(env, generation.generation)) else {
            continue;
        };
        for file in [
            format!("{}.vmlinuz", record.kernel_id),
            format!("{}.initramfs", record.initramfs),
        ] {
            if !esp.join(&file).exists() {
                missing.push(format!("{}: {file}", generation.generation));
            }
        }
    }
    check("boot", missing.is_empty(), missing.join(", "))
}

/// Whether the ELF file `bytes` has a `PT_INTERP` program header.
fn has_interpreter(bytes: &[u8]) -> Option<bool> {
    if bytes.get(..4)? != b"\x7fELF" || *bytes.get(4)? != 2 {
        return None;
    }
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let offset = usize::try_from(u64::from_le_bytes(bytes.get(32..40)?.try_into().ok()?)).ok()?;
    let size = usize::from(u16_at(54)?);
    let count = usize::from(u16_at(56)?);
    Some((0..count).any(|i| u32_at(offset + i * size) == Some(3)))
}

fn static_binaries(env: &Env) -> Check {
    let Ok(Some(active)) = env.active() else {
        return skip("static binaries", "no generation is active");
    };
    let dir = gens::gen_dir(env, active);
    let mut dynamic = Vec::new();
    for rel in [
        "usr/bin/laft",
        "usr/bin/laft-enter",
        "usr/lib/laft/laft-boot",
    ] {
        let Ok(bytes) = fs::read(dir.join(rel)) else {
            continue;
        };
        if has_interpreter(&bytes) != Some(false) {
            dynamic.push(format!("/{rel}"));
        }
    }
    check(
        "static binaries",
        dynamic.is_empty(),
        if dynamic.is_empty() {
            String::new()
        } else {
            format!("not static executables: {}", dynamic.join(", "))
        },
    )
}

fn clock(env: &Env) -> Check {
    let newest = env
        .load(&|_| Ok(()))
        .ok()
        .and_then(|l| env.indexes(&l.config, SyncMode::Cached, None).ok())
        .and_then(|i| i.iter().map(|r| r.accepted.index.timestamp).max());
    match newest {
        Some(newest) => {
            let ok = Timestamp::now() > newest;
            check(
                "clock",
                ok,
                if ok {
                    ""
                } else {
                    "the clock is behind the newest index"
                },
            )
        }
        None => skip("clock", "no index is synced"),
    }
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
