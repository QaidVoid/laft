use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, symlink};
use std::path::{Path, PathBuf};

use laft_core::{Blake3Hash, Name, StoreId, Timestamp, Variant, Version};
use laft_pkg::{Entry, Identity};
use serde::{Deserialize, Serialize};

use crate::error::{Conflict, Error, Result, io};
use crate::{CarriedModule, CarryOver, Store, fault};

/// The version of the `view.json` format.
pub(crate) const FORMAT: u32 = 1;

/// Links kept free below the filesystem's limit, so that a store file never
/// gets close enough for a later step to fail with `EMLINK`.
const LINK_HEADROOM: u64 = 16;

/// The view name of the system view, which no declared view may use.
pub(crate) const SYSTEM: &str = "system";

/// Why a package is in a view, as the resolver explains it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Reason {
    /// The configuration or the command line asked for the package.
    Requested {
        /// Where the request was made, for example
        /// `/etc/laft/system.stav:14:15`.
        origin: String,
        /// The request as written, for example `llvm@19`.
        text: String,
    },
    /// Another package depends on it.
    Dependency {
        /// The name of the depending package.
        by: Name,
        /// The dependency it satisfies, for example `so:libz.so.1`.
        dep: String,
    },
}

/// A store entry to place in a view, with the reasons it is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInput {
    /// The entry's directory name in the store.
    pub dir: String,
    /// Why the package is in the view.
    pub reasons: Vec<Reason>,
}

/// A package of a view or generation, as recorded in `view.json` and
/// `gen.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRecord {
    /// The package name.
    pub name: Name,
    /// The upstream version.
    pub version: Version,
    /// The Laft revision.
    pub rev: u32,
    /// The build variant.
    pub variant: Variant,
    /// The store id.
    pub id: StoreId,
    /// The store entry's directory name.
    pub dir: String,
    /// Why the package is in the view.
    pub reasons: Vec<Reason>,
}

/// What a declared view is assembled from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewInput {
    /// The view name. It follows the package name grammar and is never
    /// `system`.
    pub name: Name,
    /// The resolved packages.
    pub packages: Vec<PackageInput>,
    /// The member the resolution chose for every alternatives group present
    /// in the view, by group. Only the chosen member contributes the
    /// group's paths.
    pub alternatives: BTreeMap<Name, Name>,
    /// Users allowed to enter the view, with their uids, resolved by the
    /// caller.
    pub users: BTreeMap<String, u32>,
    /// Groups whose members may enter the view, with their gids, resolved by
    /// the caller.
    pub groups: BTreeMap<String, u32>,
}

/// The manifest of a view version, stored as `view.json` next to its `usr`
/// tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewManifest {
    /// The format version, 1.
    pub version: u32,
    /// The view name.
    pub name: Name,
    /// The view version.
    pub view_gen: u64,
    /// When the version was assembled.
    pub created: Timestamp,
    /// The packages, sorted by name.
    pub packages: Vec<PackageRecord>,
    /// The effective alternatives choices, by group.
    pub alternatives: BTreeMap<Name, Name>,
    /// The names of users allowed to enter the view, sorted.
    pub users: Vec<String>,
    /// The names of groups whose members may enter the view, sorted.
    pub groups: Vec<String>,
    /// The uids of `users`, sorted.
    pub allowed_uids: Vec<u32>,
    /// The gids of `groups`, sorted.
    pub allowed_gids: Vec<u32>,
    /// What the triggers produced, by trigger instance.
    pub triggers: BTreeMap<String, TriggerRecord>,
}

/// What one trigger instance produced in a view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerRecord {
    /// The hash of the trigger's inputs.
    pub fingerprint: Blake3Hash,
    /// How the run ended.
    pub status: TriggerStatus,
    /// The paths it produced, relative to the view tree, sorted.
    pub outputs: Vec<String>,
}

/// How a trigger run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerStatus {
    /// It ran and succeeded, or its outputs were reused.
    Ok,
    /// Its tool is not in the view, so it did not run.
    Skipped,
    /// It failed, and it is optional.
    Failed,
}

/// One path of a view tree.
#[derive(Debug)]
enum Node {
    Dir,
    File(PathBuf),
    Symlink(String),
}

/// A view tree computed from its packages, ready to be created.
#[derive(Debug)]
pub(crate) struct Layout {
    /// The packages, sorted by name.
    pub(crate) packages: Vec<PackageRecord>,
    /// The effective alternatives choices.
    pub(crate) alternatives: BTreeMap<Name, Name>,
    /// The carried kernel modules, in the order given.
    pub(crate) carried: Vec<CarriedModule>,
    /// Every path with its node and the index of its owner in `owners`.
    nodes: BTreeMap<String, (Node, usize)>,
    /// The store id of every package, then of every carried entry.
    owners: Vec<StoreId>,
}

/// A package loaded for assembly.
struct Loaded {
    identity: Identity,
    record: PackageRecord,
    entries: Vec<Entry>,
    groups: Vec<laft_pkg::Alternative>,
}

/// The record of the store entry `input` names.
pub(crate) fn record(store: &Store, input: &PackageInput) -> Result<PackageRecord> {
    let manifest = store.manifest(&input.dir)?;
    Ok(package_record(&manifest, input))
}

fn package_record(manifest: &laft_pkg::Manifest, input: &PackageInput) -> PackageRecord {
    let identity = manifest.package();
    PackageRecord {
        name: identity.name.clone(),
        version: identity.version.clone(),
        rev: identity.rev,
        variant: identity.variant,
        id: manifest.store_id(),
        dir: input.dir.clone(),
        reasons: input.reasons.clone(),
    }
}

/// Computes the tree of view `view` from `packages` and the carried kernel
/// modules `carry_over`, applying the alternatives `choices` of the
/// resolution, and checks that no path has two owners and that no store
/// file is near the hard link limit. Nothing is written.
pub(crate) fn layout(
    store: &Store,
    view: &str,
    packages: &[PackageInput],
    choices: &BTreeMap<Name, Name>,
    carry_over: &[CarryOver],
) -> Result<Layout> {
    let mut loaded = Vec::with_capacity(packages.len() + carry_over.len());
    for input in packages {
        let (manifest, meta) = store.load(&input.dir)?;
        loaded.push(Loaded {
            record: package_record(&manifest, input),
            identity: manifest.package().clone(),
            entries: manifest.entries().to_vec(),
            groups: meta.alternatives,
        });
    }
    loaded.sort_by(|a, b| a.identity.name.cmp(&b.identity.name));
    if let Some(pair) = loaded
        .windows(2)
        .find(|pair| pair[0].identity.name == pair[1].identity.name)
    {
        return Err(Error::DuplicatePackage {
            view: view.to_owned(),
            name: pair[0].identity.name.clone(),
        });
    }

    let (alternatives, mut skipped) = apply_alternatives(view, &loaded, choices)?;

    let mut carried = Vec::with_capacity(carry_over.len());
    let mut kernels = vec![None; loaded.len()];
    for carry in carry_over {
        let manifest = store.manifest(&carry.dir)?;
        carried.push(CarriedModule {
            kernel: carry.kernel.clone(),
            id: manifest.store_id(),
        });
        loaded.push(Loaded {
            record: package_record(
                &manifest,
                &PackageInput {
                    dir: carry.dir.clone(),
                    reasons: Vec::new(),
                },
            ),
            identity: manifest.package().clone(),
            entries: manifest.entries().to_vec(),
            groups: Vec::new(),
        });
        kernels.push(Some(format!("usr/lib/modules/{}", carry.kernel)));
        skipped.push(BTreeSet::new());
    }

    let mut nodes: BTreeMap<String, (Node, usize)> = BTreeMap::new();
    let mut conflicts: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, package) in loaded.iter().enumerate() {
        let skip = &skipped[index];
        let store_dir = store.path(&package.record.dir);
        for entry in &package.entries {
            let path = entry.path();
            if !(path == "usr" || path.starts_with("usr/")) || is_skipped(skip, path) {
                continue;
            }
            if let Some(modules) = &kernels[index]
                && !(modules.starts_with(&format!("{path}/"))
                    || path == modules
                    || path.starts_with(&format!("{modules}/")))
            {
                continue;
            }
            match (nodes.get(path), entry) {
                (None, _) => {
                    let node = match entry {
                        Entry::Dir { .. } => Node::Dir,
                        Entry::File { .. } => Node::File(store_dir.join(path)),
                        Entry::Symlink { target, .. } => Node::Symlink(target.clone()),
                    };
                    nodes.insert(path.to_owned(), (node, index));
                }
                (Some((Node::Dir, _)), Entry::Dir { .. }) => {}
                (Some((_, owner)), _) => {
                    let owners = conflicts
                        .entry(path.to_owned())
                        .or_insert_with(|| vec![*owner]);
                    owners.push(index);
                }
            }
        }
    }
    if !conflicts.is_empty() {
        return Err(Error::Conflicts {
            view: view.to_owned(),
            conflicts: conflicts
                .into_iter()
                .map(|(path, owners)| Conflict {
                    path,
                    owners: owners
                        .into_iter()
                        .map(|i| label(&loaded[i].identity))
                        .collect(),
                })
                .collect(),
        });
    }
    check_headroom(&nodes, store)?;
    let owners = loaded.iter().map(|l| l.record.id).collect();
    loaded.truncate(packages.len());
    Ok(Layout {
        packages: loaded.into_iter().map(|l| l.record).collect(),
        alternatives,
        carried,
        nodes,
        owners,
    })
}

/// Per package, in the order of the loaded packages, the paths it does not
/// contribute to the view.
type Skipped = Vec<BTreeSet<String>>;

/// Applies `choices`, the member the resolution chose for every
/// alternatives group present in the view, and returns them with, per
/// package, the paths it does not contribute.
fn apply_alternatives(
    view: &str,
    loaded: &[Loaded],
    choices: &BTreeMap<Name, Name>,
) -> Result<(BTreeMap<Name, Name>, Skipped)> {
    let mut groups: BTreeMap<&Name, Vec<(usize, &[String])>> = BTreeMap::new();
    for (index, package) in loaded.iter().enumerate() {
        for alternative in &package.groups {
            groups
                .entry(&alternative.group)
                .or_default()
                .push((index, &alternative.paths));
        }
    }
    let mut effective = BTreeMap::new();
    let mut skipped = vec![BTreeSet::new(); loaded.len()];
    for (group, members) in groups {
        let present = || {
            members
                .iter()
                .map(|(i, _)| loaded[*i].identity.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let chosen = match choices.get(group) {
            Some(chosen)
                if members
                    .iter()
                    .any(|(i, _)| loaded[*i].identity.name == *chosen) =>
            {
                chosen
            }
            Some(chosen) => {
                return Err(Error::Invalid(format!(
                    "view {view:?}: the resolution chooses {chosen} for alternatives group {group}, but its members in the view are {}",
                    present()
                )));
            }
            None => {
                return Err(Error::Invalid(format!(
                    "view {view:?}: the resolution chooses no member of alternatives group {group}, whose members in the view are {}",
                    present()
                )));
            }
        };
        for (index, paths) in members {
            if loaded[index].identity.name != *chosen {
                skipped[index].extend(paths.iter().cloned());
            }
        }
        effective.insert(group.clone(), chosen.clone());
    }
    Ok((effective, skipped))
}

/// Whether `path` or one of its ancestors is in `skip`.
fn is_skipped(skip: &BTreeSet<String>, path: &str) -> bool {
    if skip.is_empty() {
        return false;
    }
    let mut prefix = path;
    loop {
        if skip.contains(prefix) {
            return true;
        }
        match prefix.rsplit_once('/') {
            Some((parent, _)) => prefix = parent,
            None => return false,
        }
    }
}

/// How people refer to a package: `<name> <version>-r<rev>`, plus
/// `+<variant>` for variants other than dynamic.
pub(crate) fn label(identity: &Identity) -> String {
    let mut label = format!("{} {}-r{}", identity.name, identity.version, identity.rev);
    if identity.variant != Variant::Dynamic {
        label = format!("{label}+{}", identity.variant);
    }
    label
}

/// Fails when linking every file of `nodes` once more would bring a store
/// file within [`LINK_HEADROOM`] of the filesystem's hard link limit. Files
/// that share an inode in the store count together.
fn check_headroom(nodes: &BTreeMap<String, (Node, usize)>, store: &Store) -> Result<()> {
    let limit = link_max(&store.path(""))?;
    let mut inodes: HashMap<(u64, u64), (u64, u64)> = HashMap::new();
    let mut files = Vec::new();
    for (node, _) in nodes.values() {
        if let Node::File(path) = node {
            let metadata = fs::symlink_metadata(path).map_err(io(path))?;
            let key = (metadata.dev(), metadata.ino());
            inodes.entry(key).or_insert((metadata.nlink(), 0)).1 += 1;
            files.push((path, key));
        }
    }
    for (path, key) in files {
        let (links, added) = inodes[&key];
        if links + added > limit.saturating_sub(LINK_HEADROOM) {
            return Err(Error::LinkLimit {
                path: path.clone(),
                links,
                limit,
            });
        }
    }
    Ok(())
}

/// The hard link limit of the filesystem holding `dir`.
///
/// The limit is not a property the kernel reports, so it is looked up by
/// filesystem type. Every other filesystem, ext4 included, gets the ext4
/// limit, the smallest of the supported ones.
fn link_max(dir: &Path) -> Result<u64> {
    if let Some(limit) = fault::link_max() {
        return Ok(limit);
    }
    const BTRFS: u32 = 0x9123_683e;
    const XFS: u32 = 0x5846_5342;
    const F2FS: u32 = 0xf2f5_2010;
    const TMPFS: u32 = 0x0102_1994;
    let file = File::open(dir).map_err(io(dir))?;
    let stat = rustix::fs::fstatfs(&file).map_err(io(dir))?;
    Ok(match stat.f_type as u32 {
        BTRFS => 65_535,
        XFS => (1 << 31) - 1,
        F2FS | TMPFS => u64::from(u32::MAX),
        _ => 65_000,
    })
}

impl Layout {
    /// Creates the tree as `tree/usr`, with directories still writable.
    pub(crate) fn create(&self, tree: &Path) -> Result<()> {
        let mut builder = DirBuilder::new();
        builder.mode(0o755);
        if !self.nodes.contains_key("usr") {
            let usr = tree.join("usr");
            builder.create(&usr).map_err(io(&usr))?;
        }
        for (path, (node, _)) in &self.nodes {
            let target = tree.join(path);
            match node {
                Node::Dir => builder.create(&target).map_err(io(&target))?,
                Node::File(source) => fs::hard_link(source, &target).map_err(io(&target))?,
                Node::Symlink(link) => symlink(link, &target).map_err(io(&target))?,
            }
        }
        Ok(())
    }

    /// The store id of the entry that contributes `path`, relative to the
    /// tree, or `None` when no entry does. A directory that several entries
    /// ship belongs to the first of them in name order.
    pub(crate) fn owner(&self, path: &str) -> Option<StoreId> {
        self.nodes.get(path).map(|(_, owner)| self.owners[*owner])
    }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
