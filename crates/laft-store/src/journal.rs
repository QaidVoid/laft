//! The transaction journal and the number counters.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use laft_core::Name;
use serde::{Deserialize, Serialize};

use crate::error::{Result, io};
use crate::root::{read_json, to_json};
use crate::store::is_entry_name;
use crate::{Root, fsx};

/// The version of the journal format.
const FORMAT: u32 = 1;

/// How far a transaction has come, as recorded in its journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Numbers are reserved, nothing else changed.
    Planned,
    /// Missing packages are being downloaded.
    Fetching,
    /// Packages are being added to the store and may not be durable yet.
    Storing,
    /// Every package is in the store and durable.
    Stored,
    /// Views and the generation are being assembled.
    Assembling,
    /// The generation is complete and durable.
    Assembled,
    /// The generation's boot entry is installed.
    BootInstalled,
    /// The running system is being switched to the generation.
    Activating,
    /// The running system uses the generation.
    Activated,
    /// The generation is the default. The journal is about to be removed.
    Complete,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Planned => "planned",
            Self::Fetching => "fetching",
            Self::Storing => "storing",
            Self::Stored => "stored",
            Self::Assembling => "assembling",
            Self::Assembled => "assembled",
            Self::BootInstalled => "boot-installed",
            Self::Activating => "activating",
            Self::Activated => "activated",
            Self::Complete => "complete",
        })
    }
}

/// What a transaction does once its generation is assembled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Activate the generation now and make it the default.
    Switch,
    /// Make the generation the default for the next boot.
    Boot,
    /// Only build the generation.
    BuildOnly,
}

/// The record of a transaction in progress, rewritten atomically at every
/// state change so that recovery knows what to undo or finish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Journal {
    pub(crate) format: u32,
    pub(crate) txid: String,
    pub(crate) command: String,
    pub(crate) mode: Mode,
    pub(crate) boot_id: String,
    pub(crate) generation: Option<u64>,
    pub(crate) previous: Option<u64>,
    pub(crate) views: BTreeMap<Name, u64>,
    pub(crate) state: State,
    pub(crate) activation: Vec<String>,
    pub(crate) added: Vec<String>,
    pub(crate) started: String,
}

impl Journal {
    pub(crate) fn new(
        txid: String,
        command: String,
        mode: Mode,
        generation: u64,
        previous: Option<u64>,
        views: BTreeMap<Name, u64>,
        started: String,
    ) -> Self {
        Self {
            format: FORMAT,
            txid,
            command,
            mode,
            boot_id: boot_id(),
            generation: Some(generation),
            previous,
            views,
            state: State::Planned,
            activation: Vec::new(),
            added: Vec::new(),
            started,
        }
    }

    fn path(root: &Root, txid: &str) -> PathBuf {
        root.journal_dir().join(format!("{txid}.json"))
    }

    /// Writes the journal atomically.
    pub(crate) fn save(&self, root: &Root) -> Result<()> {
        fsx::write_atomic(&Self::path(root, &self.txid), &to_json(self)?, 0o600)
    }

    /// Removes the journal, which ends the transaction.
    pub(crate) fn remove(&self, root: &Root) -> Result<()> {
        let path = Self::path(root, &self.txid);
        match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(io(&path)(e)),
            _ => {}
        }
        fsx::sync_dir(&root.journal_dir())
    }

    /// Every journal under `root`, oldest first. Each is validated before any
    /// is returned, so a corrupt journal stops recovery before it touches
    /// anything.
    pub(crate) fn all(root: &Root) -> Result<Vec<Journal>> {
        let dir = root.journal_dir();
        let mut paths: Vec<PathBuf> = Vec::new();
        let items = match fs::read_dir(&dir) {
            Ok(items) => items,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(&dir)(e)),
        };
        for item in items {
            let path = item.map_err(io(&dir))?.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            if name.is_some_and(|n| !n.starts_with('.') && n.ends_with(".json")) {
                paths.push(path);
            }
        }
        paths.sort();
        let mut journals = Vec::new();
        for path in paths {
            let journal: Journal = read_json(&path)?;
            if journal.format != FORMAT || Self::path(root, &journal.txid) != path {
                return Err(crate::error::corrupt(&path, "unsupported journal"));
            }
            if let Some(dir) = journal.added.iter().find(|dir| !is_entry_name(dir)) {
                return Err(crate::error::corrupt(
                    &path,
                    format!("{dir:?} is not a store entry name"),
                ));
            }
            journals.push(journal);
        }
        Ok(journals)
    }
}

/// The id of the current boot, which tells recovery whether the machine
/// rebooted since a transaction started. Empty when the kernel does not
/// provide one, and recovery then treats the boot as unknown.
pub(crate) fn boot_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|id| id.trim().to_owned())
        .unwrap_or_default()
}

/// The next generation and view numbers to hand out.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Counters {
    pub(crate) generation: u64,
    pub(crate) views: BTreeMap<Name, u64>,
}

impl Counters {
    fn path(root: &Root) -> PathBuf {
        root.state_dir().join("counters.json")
    }

    /// Reads the counters. Numbers already present on disk are never handed
    /// out again, even when the counters file is missing or behind.
    pub(crate) fn load(root: &Root) -> Result<Self> {
        let path = Self::path(root);
        let mut counters: Self = if path.exists() {
            read_json(&path)?
        } else {
            Self::default()
        };
        let after = |numbers: Vec<u64>| numbers.last().map_or(1, |n| n + 1);
        counters.generation = counters.generation.max(after(root.generations()?));
        for name in view_names(&root.views_dir())? {
            let next = after(root.view_versions(&name)?);
            let counter = counters.views.entry(name).or_insert(1);
            *counter = (*counter).max(next);
        }
        Ok(counters)
    }

    pub(crate) fn save(&self, root: &Root) -> Result<()> {
        fsx::write_atomic(&Self::path(root), &to_json(self)?, 0o600)
    }
}

/// The names of the view directories under `dir`.
pub(crate) fn view_names(dir: &Path) -> Result<Vec<Name>> {
    let mut names = Vec::new();
    for item in fs::read_dir(dir).map_err(io(dir))? {
        let name = item.map_err(io(dir))?.file_name();
        if let Some(name) = name.to_str().and_then(|n| n.parse().ok()) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}
