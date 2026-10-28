use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::time::{Duration, SystemTime};

use laft_core::{Blake3Hash, Name, StoreId, Timestamp};
use laft_pkg::Manifest;
use rustix::fs::FlockOperation;

use crate::error::{Result, io};
use crate::journal::{Journal, view_names};
use crate::{Generation, Lock, Root, fsx};

/// How many generations garbage collection keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcPolicy {
    /// Always keep at least this many of the newest generations.
    pub keep_generations: u64,
    /// Also keep every generation created less than this many days ago.
    pub keep_days: u64,
}

impl Default for GcPolicy {
    /// Ten generations and thirty days.
    fn default() -> Self {
        Self {
            keep_generations: 10,
            keep_days: 30,
        }
    }
}

/// Something garbage collection keeps or deletes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    /// A generation.
    Generation(u64),
    /// A version of a declared view.
    View {
        /// The view name.
        name: Name,
        /// The view version.
        version: u64,
    },
    /// A store entry.
    Entry {
        /// The entry's directory name.
        dir: String,
        /// The package, as `<name> <version>-r<rev>`, or the directory name
        /// when the manifest cannot be read.
        package: String,
    },
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generation(number) => write!(f, "gen {number}"),
            Self::View { name, version } => write!(f, "view {name}@{version}"),
            Self::Entry { package, .. } => f.write_str(package),
        }
    }
}

/// Whether garbage collection keeps something, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// What the decision is about.
    pub subject: Subject,
    /// Whether it is kept.
    pub keep: bool,
    /// Why, for example `default` or `gen 43, gen 42`.
    pub reasons: Vec<String>,
}

/// What garbage collection would delete and keep.
///
/// Generations, view versions and store entries that no root references are
/// deleted. The roots are the retained generations (the newest ones, the
/// recent ones, the active, booted and default ones and those an unfinished
/// transaction references), the view versions they list, the current
/// version of each view, versions in use by a session, pins under
/// `laft/state/pins`, and entries an unfinished transaction added.
///
/// Displaying a plan explains every decision, one per line:
///
/// ```text
/// kept     gen 43         default
/// deleted  gen 30         not among the newest 10 and older than 30 days
/// kept     glibc 2.42-r1  gen 43, gen 42
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcPlan {
    decisions: Vec<Decision>,
}

impl GcPlan {
    /// Every decision: generations newest first, then view versions, then
    /// store entries.
    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// Deletes everything the plan does not keep: generations first, then
    /// view versions and views left without versions, then store entries.
    /// Each is renamed into the staging directory before it is removed, so
    /// readers see it whole or not at all.
    ///
    /// The plan must have been made under `lock`.
    ///
    /// # Errors
    ///
    /// Fails when something cannot be deleted. Whatever was deleted stays
    /// deleted, and the next collection finishes the job.
    pub fn execute(&self, lock: &Lock) -> Result<()> {
        let root = lock.root();
        let tmp = root.tmp_dir();
        let store = root.store();
        let mut views = BTreeSet::new();
        for decision in self.decisions.iter().filter(|d| !d.keep) {
            match &decision.subject {
                Subject::Generation(number) => fsx::retire(
                    &root.gen_dir(*number),
                    &tmp.join(format!("trash-gen-{number}")),
                )?,
                Subject::View { name, version } => {
                    fsx::retire(
                        &root.view_dir(name, *version),
                        &tmp.join(format!("trash-view-{name}-{version}")),
                    )?;
                    views.insert(name);
                }
                Subject::Entry { dir, .. } => {
                    fsx::retire(&store.path(dir), &tmp.join(format!("trash-{dir}")))?;
                }
            }
        }
        for name in views {
            let dir = root.views_dir().join(name.as_str());
            if fs::read_dir(&dir).map_err(io(&dir))?.next().is_none() {
                fs::remove_dir(&dir).map_err(io(&dir))?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for GcPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for decision in &self.decisions {
            let verdict = if decision.keep { "kept" } else { "deleted" };
            writeln!(
                f,
                "{verdict:<8} {:<14} {}",
                decision.subject.to_string(),
                decision.reasons.join(", ")
            )?;
        }
        Ok(())
    }
}

/// Decides what garbage collection keeps under `policy`, with `now` as the
/// current time. Nothing is deleted. Recover interrupted transactions with
/// [`Lock::recover`] first.
///
/// # Errors
///
/// Fails when the generations, views, store or state cannot be read.
pub fn plan_gc(lock: &Lock, policy: &GcPolicy, now: SystemTime) -> Result<GcPlan> {
    let root = lock.root();
    let journals = Journal::all(root)?;
    let mut decisions = Vec::new();
    let mut entry_reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut live_ids: BTreeMap<StoreId, Vec<String>> = BTreeMap::new();
    let mut view_reasons: BTreeMap<(Name, u64), Vec<String>> = BTreeMap::new();

    let numbers = root.generations()?;
    let mut generations: BTreeMap<u64, Generation> = BTreeMap::new();
    for &number in &numbers {
        if let Ok(generation) = root.generation(number) {
            generations.insert(number, generation);
        }
    }
    let newest: BTreeSet<u64> = generations
        .keys()
        .rev()
        .take(usize::try_from(policy.keep_generations).unwrap_or(usize::MAX))
        .copied()
        .collect();
    let horizon = now.checked_sub(Duration::from_secs(policy.keep_days.saturating_mul(86_400)));
    let run = root.run_dir();
    let pointers = [
        (fsx::link_number(&run.join("system")), "active system"),
        (fsx::link_number(&run.join("booted")), "booted"),
        (root.default_generation().ok().flatten(), "default"),
    ];
    for number in numbers.into_iter().rev() {
        let mut reasons: Vec<String> = pointers
            .iter()
            .filter(|(target, _)| *target == Some(number))
            .map(|(_, why)| (*why).to_owned())
            .collect();
        if journals.iter().any(|j| j.generation == Some(number)) {
            reasons.push(UNFINISHED.into());
        }
        let Some(generation) = generations.get(&number) else {
            reasons.push("gen.json is unreadable".into());
            decisions.push(keep(Subject::Generation(number), reasons));
            continue;
        };
        if newest.contains(&number) {
            reasons.push(format!("newest {}", policy.keep_generations));
        }
        let created = generation
            .created
            .parse::<Timestamp>()
            .ok()
            .map(SystemTime::from);
        if policy.keep_days > 0 && created.zip(horizon).is_none_or(|(c, h)| c > h) {
            reasons.push(format!("younger than {} days", policy.keep_days));
        }
        if reasons.is_empty() {
            let mut why = format!("not among the newest {}", policy.keep_generations);
            if policy.keep_days > 0 {
                why.push_str(&format!(" and older than {} days", policy.keep_days));
            }
            decisions.push(delete(Subject::Generation(number), why));
            continue;
        }
        let tag = format!("gen {number}");
        for package in generation
            .packages
            .iter()
            .chain(&generation.initrd_packages)
        {
            add_reason(&mut entry_reasons, package.dir.clone(), &tag);
        }
        for module in &generation.carried_modules {
            add_reason(&mut live_ids, module.id, &tag);
        }
        for (name, version) in &generation.views {
            add_reason(&mut view_reasons, (name.clone(), *version), &tag);
        }
        decisions.push(keep(Subject::Generation(number), reasons));
    }

    for journal in &journals {
        for (name, version) in &journal.views {
            add_reason(&mut view_reasons, (name.clone(), *version), UNFINISHED);
        }
        for dir in &journal.added {
            add_reason(&mut entry_reasons, dir.clone(), UNFINISHED);
        }
    }
    for name in view_names(&root.views_dir())? {
        let current = fsx::link_number(&root.views_dir().join(name.as_str()).join("current"));
        for version in root.view_versions(&name)? {
            let mut reasons = view_reasons
                .remove(&(name.clone(), version))
                .unwrap_or_default();
            if current == Some(version) {
                reasons.push("current".into());
            }
            if in_use(root, &name, version)? {
                reasons.push("in use".into());
            }
            let subject = Subject::View {
                name: name.clone(),
                version,
            };
            if reasons.is_empty() {
                decisions.push(delete(
                    subject,
                    "no retained generation references it".into(),
                ));
                continue;
            }
            match root.view(&name, version) {
                Ok(view) => {
                    let tag = format!("view {name}@{version}");
                    for package in &view.packages {
                        add_reason(&mut entry_reasons, package.dir.clone(), &tag);
                    }
                }
                Err(_) => reasons.push("view.json is unreadable".into()),
            }
            decisions.push(keep(subject, reasons));
        }
    }

    for dir in pins(root)? {
        add_reason(&mut entry_reasons, dir, "pinned");
    }
    let store = root.store();
    for dir in store.list()? {
        let manifest = fs::read(store.path(&dir).join(".laft/manifest.json")).ok();
        let id = manifest
            .as_deref()
            .map(|bytes| StoreId::from_manifest_hash(&Blake3Hash::of(bytes)));
        let package = manifest
            .as_deref()
            .and_then(|bytes| Manifest::from_json(bytes).ok())
            .map_or_else(|| dir.clone(), |m| crate::view::label(m.package()));
        let mut reasons = entry_reasons.remove(&dir).unwrap_or_default();
        for why in id.and_then(|id| live_ids.get(&id)).into_iter().flatten() {
            if !reasons.contains(why) {
                reasons.push(why.clone());
            }
        }
        let subject = Subject::Entry { dir, package };
        if reasons.is_empty() {
            decisions.push(delete(
                subject,
                "no retained generation or view references it".into(),
            ));
        } else {
            decisions.push(keep(subject, reasons));
        }
    }
    Ok(GcPlan { decisions })
}

const UNFINISHED: &str = "unfinished transaction";

/// Records `reason` for `key` unless it is recorded already.
fn add_reason<K: Ord>(reasons: &mut BTreeMap<K, Vec<String>>, key: K, reason: &str) {
    let list = reasons.entry(key).or_default();
    if !list.iter().any(|r| r == reason) {
        list.push(reason.to_owned());
    }
}

fn keep(subject: Subject, reasons: Vec<String>) -> Decision {
    Decision {
        subject,
        keep: true,
        reasons,
    }
}

fn delete(subject: Subject, reason: String) -> Decision {
    Decision {
        subject,
        keep: false,
        reasons: vec![reason],
    }
}

/// Whether a session holds the lock on the view version's `view.json`.
fn in_use(root: &Root, name: &Name, version: u64) -> Result<bool> {
    let path = root.view_dir(name, version).join("view.json");
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(io(&path)(e)),
    };
    match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(false),
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(true),
        Err(e) => Err(io(&path)(e)),
    }
}

/// The store entries that symlinks in `laft/state/pins` point at.
fn pins(root: &Root) -> Result<Vec<String>> {
    let dir = root.pins_dir();
    let mut pinned = Vec::new();
    for item in fs::read_dir(&dir).map_err(io(&dir))? {
        let path = item.map_err(io(&dir))?.path();
        if let Ok(target) = fs::read_link(&path)
            && let Some(name) = target.file_name()
        {
            pinned.push(name.to_string_lossy().into_owned());
        }
    }
    Ok(pinned)
}

#[cfg(test)]
#[path = "gc_tests.rs"]
mod tests;
