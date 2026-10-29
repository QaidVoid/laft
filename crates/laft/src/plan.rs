//! What `laft apply` would change: packages per view, services, downloads,
//! and whether anything changes at all.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use laft_core::{Blake3Hash, Name, StoreId, Variant, Version};
use laft_index::IndexEntry;
use laft_resolve::{Resolution, Source};
use laft_store::{Generation, Root};
use laft_svc::reconcile::{self, Class};
use laft_svc::{OnChange, ServiceSet, runit};
use serde_json::{Value, json};

use crate::env::RepoIndex;
use crate::failure::Failure;
use crate::resolve::{INITRD, Resolved, SYSTEM};
use crate::ui::{self, Sym, Ui};

/// A package version as plans show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pkg {
    /// The name.
    pub(crate) name: Name,
    /// The upstream version.
    pub(crate) version: Version,
    /// The revision.
    pub(crate) rev: u32,
    /// The variant.
    pub(crate) variant: Variant,
    /// The store id.
    pub(crate) id: StoreId,
}

impl Pkg {
    /// The name with the variant when it is not the default one.
    pub(crate) fn reference(&self) -> String {
        if self.variant == Variant::default() {
            self.name.to_string()
        } else {
            format!("{}:{}", self.name, self.variant)
        }
    }

    /// The version and revision, such as `14.1.1-r1`.
    pub(crate) fn full_version(&self) -> String {
        format!("{}-r{}", self.version, self.rev)
    }

    /// The plan package of a generation's record.
    pub(crate) fn of_record(p: &laft_store::PackageRecord) -> Self {
        Self {
            name: p.name.clone(),
            version: p.version.clone(),
            rev: p.rev,
            variant: p.variant,
            id: p.id,
        }
    }
}

/// How one package differs between the active and the new generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    /// Only the new generation has it.
    Added(Pkg),
    /// Only the active generation has it.
    Removed(Pkg),
    /// It moved to a newer version.
    Upgraded(Pkg, Pkg),
    /// It moved to an older version.
    Downgraded(Pkg, Pkg),
    /// Its variant or build changed at the same version.
    Changed(Pkg, Pkg),
}

/// How a view differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewPlan {
    /// The view name, `system` for the system view.
    pub(crate) name: String,
    /// The package changes, by name.
    pub(crate) changes: Vec<Change>,
    /// The version of the active generation to keep, when nothing about
    /// the view changes.
    pub(crate) keep: Option<u64>,
}

/// How the services differ.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ServicePlan {
    /// New services.
    pub(crate) added: Vec<String>,
    /// Services that go away.
    pub(crate) removed: Vec<String>,
    /// Services that are restarted, with why.
    pub(crate) restarted: Vec<(String, &'static str)>,
    /// Services that change without a restart.
    pub(crate) changed: Vec<String>,
}

/// A package to download and add to the store.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fetch<'a> {
    /// The index entry.
    pub(crate) entry: &'a IndexEntry,
    /// The repository it comes from, as an index into the indexes.
    pub(crate) repo: usize,
}

/// Everything `laft apply` would do.
#[derive(Debug)]
pub(crate) struct Plan<'a> {
    /// The number the new generation would get.
    pub(crate) generation: Option<u64>,
    /// The active generation.
    pub(crate) previous: Option<u64>,
    /// The system view, then the declared views.
    pub(crate) views: Vec<ViewPlan>,
    /// The services.
    pub(crate) services: ServicePlan,
    /// The packages missing from the store.
    pub(crate) fetch: Vec<Fetch<'a>>,
    /// The bytes to download.
    pub(crate) download_bytes: u64,
    /// The bytes the store grows by.
    pub(crate) store_bytes: u64,
    /// Whether the configuration differs from the active generation's.
    pub(crate) config_changed: bool,
    /// Whether nothing at all would change.
    pub(crate) nothing_to_do: bool,
}

/// What a plan is computed from.
pub(crate) struct Inputs<'r, 'a> {
    /// The root.
    pub(crate) root: &'r Root,
    /// The active generation.
    pub(crate) active: Option<&'r Generation>,
    /// The resolutions.
    pub(crate) resolved: &'r Resolved<'a>,
    /// The accepted indexes the resolutions come from.
    pub(crate) indexes: &'a [RepoIndex],
    /// The evaluated configuration value.
    pub(crate) config: &'r [u8],
    /// The validated services.
    pub(crate) services: &'r ServiceSet,
    /// The declared views with their allowed users and groups.
    pub(crate) view_access: BTreeMap<Name, (Vec<String>, Vec<String>)>,
}

/// The packages of a resolution as plan packages.
fn resolved_packages(resolution: &Resolution<'_>) -> BTreeMap<Name, Pkg> {
    resolution
        .packages
        .iter()
        .map(|(name, s)| {
            let meta = s.candidate.meta;
            let pkg = Pkg {
                name: name.clone(),
                version: meta.version.clone(),
                rev: meta.rev,
                variant: meta.variant,
                id: s.candidate.id,
            };
            (name.clone(), pkg)
        })
        .collect()
}

/// How the packages `old` became `new`, by name.
pub(crate) fn changes(old: &BTreeMap<Name, Pkg>, new: &BTreeMap<Name, Pkg>) -> Vec<Change> {
    let names: BTreeSet<&Name> = old.keys().chain(new.keys()).collect();
    let mut out = Vec::new();
    for name in names {
        match (old.get(name), new.get(name)) {
            (None, Some(n)) => out.push(Change::Added(n.clone())),
            (Some(o), None) => out.push(Change::Removed(o.clone())),
            (Some(o), Some(n)) if o.id == n.id => {}
            (Some(o), Some(n)) => {
                let (a, b) = ((&o.version, o.rev), (&n.version, n.rev));
                out.push(if b > a {
                    Change::Upgraded(o.clone(), n.clone())
                } else if b < a {
                    Change::Downgraded(o.clone(), n.clone())
                } else {
                    Change::Changed(o.clone(), n.clone())
                });
            }
            (None, None) => {}
        }
    }
    out
}

/// Computes the plan.
///
/// # Errors
///
/// Fails when the services cannot be rendered or the active generation's
/// services cannot be read.
///
/// # Panics
///
/// Panics when a resolved package comes from none of `inputs.indexes`,
/// which the resolution of those indexes rules out.
pub(crate) fn plan<'a>(inputs: &Inputs<'_, 'a>) -> Result<Plan<'a>, Failure> {
    let store = inputs.root.store();
    let active = inputs.active;
    let mut views = Vec::new();

    let old_system = active
        .map(|g| {
            g.packages
                .iter()
                .map(|p| (p.name.clone(), Pkg::of_record(p)))
                .collect()
        })
        .unwrap_or_default();
    let system_changes = changes(&old_system, &resolved_packages(&inputs.resolved.system));
    let alternatives_changed =
        active.is_none_or(|g| g.alternatives != inputs.resolved.system.alternatives);
    views.push(ViewPlan {
        name: SYSTEM.to_owned(),
        keep: None,
        changes: system_changes,
    });
    let old_initrd = active
        .map(|g| {
            g.initrd_packages
                .iter()
                .map(|p| (p.name.clone(), Pkg::of_record(p)))
                .collect()
        })
        .unwrap_or_default();
    let initrd_changes = changes(&old_initrd, &resolved_packages(&inputs.resolved.initrd));
    for (name, resolution) in &inputs.resolved.views {
        let old_view = active
            .and_then(|g| g.views.get(name).map(|v| (*v, inputs.root.view(name, *v))))
            .and_then(|(version, view)| Some((version, view.ok()?)));
        let old_packages = old_view
            .as_ref()
            .map(|(_, v)| {
                v.packages
                    .iter()
                    .map(|p| (p.name.clone(), Pkg::of_record(p)))
                    .collect()
            })
            .unwrap_or_default();
        let changes = changes(&old_packages, &resolved_packages(resolution));
        let keep = old_view.and_then(|(version, view)| {
            let access = inputs.view_access.get(name);
            let same_access = access
                .is_some_and(|(users, groups)| &view.users == users && &view.groups == groups);
            (changes.is_empty() && same_access && view.alternatives == resolution.alternatives)
                .then_some(version)
        });
        views.push(ViewPlan {
            name: name.to_string(),
            changes,
            keep,
        });
    }

    let mut fetch = Vec::new();
    let mut seen = BTreeSet::new();
    let (mut download_bytes, mut store_bytes) = (0, 0);
    let all = [&inputs.resolved.system, &inputs.resolved.initrd]
        .into_iter()
        .chain(inputs.resolved.views.values());
    for resolution in all {
        for selected in resolution.packages.values() {
            let Source::Index { repo, entry } = selected.candidate.source else {
                continue;
            };
            let meta = selected.candidate.meta;
            let dir = entry.id.dir_name(&meta.name, &meta.version, meta.variant);
            if store.path(&dir).exists() || !seen.insert(entry.id) {
                continue;
            }
            let repo =
                inputs.indexes.iter().position(|r| &r.name == repo).expect(
                    "a resolved package comes from one of the indexes it was resolved against",
                );
            download_bytes += entry.size;
            store_bytes += meta.installed_size;
            fetch.push(Fetch { entry, repo });
        }
    }

    let number = inputs.root.next_generation()?;
    let generation = Some(number);
    let services = service_plan(inputs, number)?;
    let config_changed = active.is_none_or(|g| g.config.hash != Blake3Hash::of(inputs.config));
    let declared: BTreeSet<&Name> = inputs.resolved.views.keys().collect();
    let views_same = active.is_none_or(|g| g.views.keys().collect::<BTreeSet<_>>() == declared)
        && views[1..]
            .iter()
            .all(|v| v.keep.is_some() || v.name == INITRD);
    let nothing_to_do = active.is_some_and(|g| g.laft_version == env!("CARGO_PKG_VERSION"))
        && !config_changed
        && views[0].changes.is_empty()
        && initrd_changes.is_empty()
        && !alternatives_changed
        && views_same;
    if !initrd_changes.is_empty() {
        views.push(ViewPlan {
            name: INITRD.to_owned(),
            changes: initrd_changes,
            keep: None,
        });
    }
    Ok(Plan {
        generation,
        previous: active.map(|g| g.generation),
        views,
        services,
        fetch,
        download_bytes,
        store_bytes,
        config_changed,
        nothing_to_do,
    })
}

fn service_plan(inputs: &Inputs<'_, '_>, generation: u64) -> Result<ServicePlan, Failure> {
    let new = runit::render(inputs.services, generation)?;
    let old = match inputs.active {
        Some(g) => {
            let dir = inputs
                .root
                .path()
                .join(format!("laft/gens/{}/services", g.generation));
            if dir.join("spec").exists() {
                runit::load_units(&dir)?
            } else {
                BTreeMap::new()
            }
        }
        None => BTreeMap::new(),
    };
    let mut plan = ServicePlan::default();
    for (name, class) in reconcile::plan(&old, new.units()).classes {
        match class {
            Class::Added => plan.added.push(name),
            Class::Removed => plan.removed.push(name),
            Class::Restructured => plan.restarted.push((name, "definition changed")),
            Class::Changed(OnChange::Restart) => {
                plan.restarted.push((name, "configuration changed"));
            }
            Class::Changed(_) => plan.changed.push(name),
            Class::Unchanged => {}
        }
    }
    Ok(plan)
}

impl Plan<'_> {
    /// Whether the plan changes anything.
    pub(crate) fn has_changes(&self) -> bool {
        !self.nothing_to_do
    }

    /// The plan as human output.
    pub(crate) fn render(&self, ui: &Ui) -> String {
        let mut out = String::new();
        if self.nothing_to_do {
            out.push_str("system is up to date\n");
            return out;
        }
        let generation = self
            .generation
            .map_or_else(|| "?".into(), |g| g.to_string());
        match self.previous {
            Some(previous) => {
                let _ = writeln!(out, "generation {generation} (from {previous})");
            }
            None => {
                let _ = writeln!(out, "generation {generation}");
            }
        }
        for view in &self.views {
            let title = if view.name == SYSTEM || view.name == INITRD {
                view.name.clone()
            } else {
                format!("view {}", view.name)
            };
            if view.changes.is_empty() {
                let _ = writeln!(out, "  {title:<18}{} unchanged", ui.sym(Sym::Unchanged));
                continue;
            }
            let _ = writeln!(out, "  {title}");
            out.push_str(&render_changes(ui, &view.changes));
        }
        let s = &self.services;
        if !(s.added.is_empty()
            && s.removed.is_empty()
            && s.restarted.is_empty()
            && s.changed.is_empty())
        {
            let _ = writeln!(out, "  services");
            let mut rows: Vec<(String, &str, &str)> = Vec::new();
            rows.extend(
                s.restarted
                    .iter()
                    .map(|(n, why)| (ui.sym(Sym::Restarted), n.as_str(), *why)),
            );
            rows.extend(
                s.changed
                    .iter()
                    .map(|n| (ui.sym(Sym::Changed), n.as_str(), "")),
            );
            rows.extend(s.added.iter().map(|n| (ui.sym(Sym::Added), n.as_str(), "")));
            rows.extend(
                s.removed
                    .iter()
                    .map(|n| (ui.sym(Sym::Removed), n.as_str(), "")),
            );
            let width = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
            for (sym, name, why) in rows {
                let line = format!("    {sym} {name:<width$}  {why}");
                let _ = writeln!(out, "{}", line.trim_end());
            }
        }
        let _ = writeln!(
            out,
            "\n  download {} {} store {} {} {} package{}",
            ui::size(self.download_bytes),
            ui.dot(),
            ui::size_delta(i64::try_from(self.store_bytes).unwrap_or(i64::MAX)),
            ui.dot(),
            self.fetch.len(),
            if self.fetch.len() == 1 { "" } else { "s" }
        );
        out
    }

    /// The plan as JSON, the `plan` event of `laft apply` and the document
    /// of `laft plan`.
    pub(crate) fn to_json(&self) -> Value {
        let mut views = serde_json::Map::new();
        for view in &self.views {
            views.insert(view.name.clone(), changes_json(&view.changes));
        }
        let s = &self.services;
        json!({
            "generation": if self.nothing_to_do { None } else { self.generation },
            "previous": self.previous,
            "views": views,
            "services": {
                "added": s.added,
                "removed": s.removed,
                "restarted": s.restarted.iter().map(|(n, _)| n).collect::<Vec<_>>(),
                "changed": s.changed,
            },
            "config_changed": self.config_changed,
            "download_bytes": self.download_bytes,
            "store_bytes": self.store_bytes,
            "packages": self.fetch.len(),
            "nothing_to_do": self.nothing_to_do,
        })
    }
}

/// Package changes as indented lines with symbols and aligned columns.
pub(crate) fn render_changes(ui: &Ui, changes: &[Change]) -> String {
    let arrow = ui.arrow();
    let rows: Vec<(String, String, String)> = changes
        .iter()
        .map(|change| match change {
            Change::Added(n) => (ui.sym(Sym::Added), n.reference(), n.full_version()),
            Change::Removed(o) => (ui.sym(Sym::Removed), o.reference(), o.full_version()),
            Change::Upgraded(o, n) => (
                ui.sym(Sym::Upgraded),
                n.reference(),
                format!("{} {arrow} {}", o.full_version(), n.full_version()),
            ),
            Change::Downgraded(o, n) => (
                ui.sym(Sym::Downgraded),
                n.reference(),
                format!("{} {arrow} {}", o.full_version(), n.full_version()),
            ),
            Change::Changed(o, n) => (
                ui.sym(Sym::Changed),
                n.reference(),
                if o.variant == n.variant {
                    format!("{} rebuilt", n.full_version())
                } else {
                    format!("{} {arrow} {}", o.reference(), n.reference())
                },
            ),
        })
        .collect();
    let width = rows.iter().map(|r| ui::width(&r.1)).max().unwrap_or(0);
    let mut out = String::new();
    for (sym, name, detail) in rows {
        let pad = " ".repeat(width - ui::width(&name) + 2);
        let _ = writeln!(out, "    {sym} {name}{pad}{detail}");
    }
    out
}

/// Package changes as JSON lists by kind.
pub(crate) fn changes_json(changes: &[Change]) -> Value {
    let pkg = |p: &Pkg| {
        json!({"name": p.name, "version": p.full_version(),
            "variant": p.variant.to_string(), "id": p.id.to_string()})
    };
    let pair = |o: &Pkg, n: &Pkg| json!({"name": n.name, "from": pkg(o), "to": pkg(n)});
    let (mut added, mut removed, mut upgraded, mut downgraded, mut changed) =
        (vec![], vec![], vec![], vec![], vec![]);
    for change in changes {
        match change {
            Change::Added(n) => added.push(pkg(n)),
            Change::Removed(o) => removed.push(pkg(o)),
            Change::Upgraded(o, n) => upgraded.push(pair(o, n)),
            Change::Downgraded(o, n) => downgraded.push(pair(o, n)),
            Change::Changed(o, n) => changed.push(pair(o, n)),
        }
    }
    json!({"added": added, "removed": removed, "upgraded": upgraded,
        "downgraded": downgraded, "changed": changed, "unchanged": changes.is_empty()})
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
