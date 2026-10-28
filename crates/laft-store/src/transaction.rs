use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use laft_core::{Blake3Hash, Name, StoreId, Timestamp};

use crate::error::{Error, Result, io};
use crate::generation::{ConfigRecord, EtcRecord, FORMAT as GEN_FORMAT};
use crate::journal::{Counters, Journal};
use crate::root::to_json;
use crate::store::Prepared;
use crate::view::{self, SYSTEM};
use crate::{Generation, GenerationInput, Lock, Root, State, ViewInput, ViewManifest, fault, fsx};

/// What a transaction is going to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The command that runs the transaction, for example `apply`.
    pub command: String,
    /// The active system generation, which becomes the parent of the new
    /// one.
    pub previous: Option<u64>,
    /// The declared views that get a new version.
    pub views: BTreeSet<Name>,
}

/// A transaction that adds packages to the store and assembles views and a
/// new generation from them.
///
/// Every step is recorded in a journal under `laft/state/journal` before
/// its effects become visible, so that [`Lock::recover`] can finish or roll
/// back a transaction that was interrupted at any point. Dropping a
/// transaction without finishing it leaves the journal in place, exactly as
/// a crash would.
///
/// The steps run in this order:
///
/// 1. [`Transaction::begin`] reserves the generation number and view
///    versions.
/// 2. [`Transaction::add`] adds each package to the store.
/// 3. [`Transaction::assemble_view`] assembles each planned view, then
///    [`Transaction::assemble_generation`] the generation.
/// 4. [`Transaction::make_default`] makes it the default generation, or
///    [`Transaction::close`] leaves it built but unused.
///
/// ```no_run
/// use std::collections::BTreeSet;
/// use std::path::Path;
///
/// use laft_store::{
///     GenerationInput, Init, PackageInput, Plan, Root, Transaction,
/// };
///
/// let root = Root::default();
/// let lock = root.lock("apply")?;
/// lock.recover()?;
/// let plan = Plan {
///     command: "apply".into(),
///     previous: root.default_generation()?,
///     views: BTreeSet::new(),
/// };
/// let mut tx = Transaction::begin(&lock, &plan)?;
/// let id = "4k7cq2r9x0m1n3b5v6c8z7l2k4j6h8g0".parse()?;
/// let dir = tx.add(Path::new("ripgrep-14.1.1-r1.x86_64-v3.lpkg"), &id)?;
/// tx.assemble_generation(&GenerationInput {
///     packages: vec![PackageInput { dir, reasons: vec![] }],
///     alternatives: Default::default(),
///     views: Default::default(),
///     message: None,
///     laft_version: "0.1.0".into(),
///     arch: "x86_64-v3".into(),
///     init: Init { backend: "runit".into(), exec: vec!["/usr/bin/runit".into()] },
///     config: b"{}".to_vec(),
///     config_files: vec![],
///     indexes: vec![],
/// })?;
/// let generation = tx.make_default()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Transaction<'l> {
    lock: &'l Lock,
    journal: Journal,
    generation: u64,
}

impl<'l> Transaction<'l> {
    /// Starts a transaction: reserves the next generation number and a new
    /// version for every planned view, and records the plan in the journal.
    /// Reserved numbers are never handed out again, even when the
    /// transaction fails.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Pending`] while an interrupted transaction awaits
    /// recovery, when a planned view is named `system`, or when the state
    /// cannot be written.
    pub fn begin(lock: &'l Lock, plan: &Plan) -> Result<Self> {
        let root = lock.root();
        if let Some(journal) = Journal::all(root)?.into_iter().next() {
            return Err(Error::Pending {
                txid: journal.txid,
                state: journal.state,
            });
        }
        if plan.views.iter().any(|name| name.as_str() == SYSTEM) {
            return Err(Error::Invalid(format!(
                "a declared view cannot be named {SYSTEM:?}"
            )));
        }
        fsx::clear_dir(&root.tmp_dir())?;
        let mut counters = Counters::load(root)?;
        let generation = counters.generation;
        counters.generation += 1;
        let mut views = BTreeMap::new();
        for name in &plan.views {
            let next = counters.views.entry(name.clone()).or_insert(1);
            views.insert(name.clone(), *next);
            *next += 1;
        }
        counters.save(root)?;
        fault::point("begin.counters")?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let journal = Journal::new(
            format!("{millis}-{}", std::process::id()),
            plan.command.clone(),
            generation,
            plan.previous,
            views,
        );
        journal.save(root)?;
        fault::point("begin.journal")?;
        Ok(Self {
            lock,
            journal,
            generation,
        })
    }

    /// The number of the generation this transaction builds.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The version reserved for view `name`, if it was planned.
    pub fn view_version(&self, name: &Name) -> Option<u64> {
        self.journal.views.get(name).copied()
    }

    /// Adds the package file `lpkg` to the store and returns its entry's
    /// directory name.
    ///
    /// The package must have the store id `expected`. It is unpacked and
    /// verified in the staging directory, its files lose any write bits and
    /// get the modification time 1 second after the epoch, its directories
    /// get mode `0o555`, and the entry is published with one rename. Adding
    /// a package whose entry exists does nothing. A new entry is recorded in
    /// the journal before it is published, so that recovery removes it if
    /// the transaction is interrupted before the store is synced.
    ///
    /// # Errors
    ///
    /// Fails when assembly has started, when the package is invalid or does
    /// not match its manifest, when its id is not `expected`, or when the
    /// store cannot be written. A failed add leaves no entry and no staging
    /// directory behind.
    pub fn add(&mut self, lpkg: &Path, expected: &StoreId) -> Result<String> {
        match self.journal.state {
            State::Planned | State::Fetching => self.set_state(State::Storing, "add.storing")?,
            State::Storing => {}
            _ => {
                return Err(Error::Invalid(
                    "packages must be added before assembly starts".into(),
                ));
            }
        }
        let root = self.lock.root();
        let store = root.store();
        match store.stage(lpkg, expected, &root.tmp_dir(), &self.journal.txid)? {
            Prepared::Exists(dir) => Ok(dir),
            Prepared::Staged(staged) => {
                fault::point("add.staged")?;
                let dir = staged.dir_name.clone();
                self.journal.added.push(dir.clone());
                self.journal.save(root)?;
                fault::point("add.recorded")?;
                store.publish(staged)?;
                fault::point("add.published")?;
                Ok(dir)
            }
        }
    }

    /// Assembles a new version of the planned view `input.name` from store
    /// entries and returns the version.
    ///
    /// The view's `usr` tree holds a hard link to every store file, a copy
    /// of every symlink and a directory with mode `0o555` for every
    /// directory of its packages. Only paths under `usr` are used. The
    /// version appears with one rename, together with its `view.json`.
    ///
    /// # Errors
    ///
    /// Fails when the view was not planned or is assembled already, when a
    /// package is missing from the store, when two packages own a path
    /// (naming both and the path), when alternatives cannot be resolved,
    /// when a store file is near the hard link limit, or when writing
    /// fails. Nothing is created in the views directory then.
    pub fn assemble_view(&mut self, input: &ViewInput) -> Result<u64> {
        let version = self.view_version(&input.name).ok_or_else(|| {
            Error::Invalid(format!(
                "view {:?} is not part of this transaction",
                input.name.as_str()
            ))
        })?;
        self.start_assembly()?;
        let root = self.lock.root();
        let target = root.view_dir(&input.name, version);
        if target.exists() {
            return Err(Error::Invalid(format!(
                "view {}@{version} is assembled already",
                input.name
            )));
        }
        let layout = view::layout(
            &root.store(),
            input.name.as_str(),
            &input.packages,
            &input.alternatives,
        )?;
        let staging = self.staging(&format!("view-{}-{version}", input.name))?;
        layout.create(&staging)?;
        let ids = |accounts: &BTreeMap<String, u32>| {
            let ids: BTreeSet<u32> = accounts.values().copied().collect();
            ids.into_iter().collect()
        };
        let manifest = ViewManifest {
            version: view::FORMAT,
            name: input.name.clone(),
            view_gen: version,
            created: Timestamp::now(),
            packages: layout.packages,
            alternatives: layout.alternatives,
            users: input.users.keys().cloned().collect(),
            groups: input.groups.keys().cloned().collect(),
            allowed_uids: ids(&input.users),
            allowed_gids: ids(&input.groups),
            triggers: BTreeMap::new(),
        };
        let path = staging.join("view.json");
        fsx::write_new(&path, &to_json(&path, &manifest)?, 0o444)?;
        fsx::seal(&staging, false)?;
        fault::point("view.staged")?;
        fsx::ensure_dir(&root.views_dir().join(input.name.as_str()), 0o755)?;
        publish_tree(&staging, &target)?;
        fault::point("view.published")?;
        Ok(version)
    }

    /// Assembles the generation and returns its number.
    ///
    /// The generation directory holds the system view as `usr`, the
    /// configuration as `config.json`, and `gen.json`, which is written last.
    /// Its `etc`, `services` and `boot` directories are created empty. The
    /// generation appears with one rename.
    ///
    /// # Errors
    ///
    /// Fails as [`Self::assemble_view`] does, when the generation is
    /// assembled already, and when a view version in `input.views` does not
    /// exist.
    pub fn assemble_generation(&mut self, input: &GenerationInput) -> Result<u64> {
        let number = self.generation();
        self.start_assembly()?;
        let root = self.lock.root();
        let target = root.gen_dir(number);
        if target.exists() {
            return Err(Error::Invalid(format!(
                "generation {number} is assembled already"
            )));
        }
        for (name, version) in &input.views {
            root.view(name, *version)?;
        }
        let layout = view::layout(&root.store(), SYSTEM, &input.packages, &input.alternatives)?;
        let staging = self.staging(&format!("gen-{number}"))?;
        layout.create(&staging)?;
        for dir in ["etc", "services", "boot"] {
            fsx::create_dir(&staging.join(dir), 0o755)?;
        }
        fsx::write_new(&staging.join("config.json"), &input.config, 0o444)?;
        let generation = Generation {
            format: GEN_FORMAT,
            generation: number,
            parent: self.journal.previous,
            created: Timestamp::now(),
            message: input.message.clone(),
            laft_version: input.laft_version.clone(),
            arch: input.arch.clone(),
            init: input.init.clone(),
            config: ConfigRecord {
                hash: Blake3Hash::of(&input.config),
                files: input.config_files.clone(),
            },
            indexes: input.indexes.clone(),
            packages: layout.packages,
            initrd_packages: Vec::new(),
            alternatives: layout.alternatives,
            views: input.views.clone(),
            carried_modules: Vec::new(),
            etc: EtcRecord::default(),
        };
        let path = staging.join("gen.json");
        fsx::write_new(&path, &to_json(&path, &generation)?, 0o444)?;
        fsx::seal(&staging, false)?;
        fault::point("gen.staged")?;
        publish_tree(&staging, &target)?;
        fault::point("gen.published")?;
        Ok(number)
    }

    /// Finishes the transaction with the generation built but neither
    /// active nor the default, and returns its number.
    ///
    /// # Errors
    ///
    /// Fails when the generation was not assembled or the state cannot be
    /// written.
    pub fn close(mut self) -> Result<u64> {
        let number = self.finish_assembly()?;
        self.journal.remove(self.lock.root())?;
        Ok(number)
    }

    /// Makes the generation the one the machine boots by default, pointing
    /// `laft/gens/default` at it with one rename, finishes the transaction
    /// and returns the generation's number.
    ///
    /// # Errors
    ///
    /// Fails as [`Self::close`] does.
    pub fn make_default(mut self) -> Result<u64> {
        let number = self.finish_assembly()?;
        let root = self.lock.root();
        set_default(root, number, &self.journal.txid)?;
        fault::point("default.linked")?;
        self.set_state(State::Complete, "complete")?;
        self.journal.remove(root)?;
        Ok(number)
    }

    fn set_state(&mut self, state: State, point: &'static str) -> Result<()> {
        self.journal.state = state;
        self.journal.save(self.lock.root())?;
        fault::point(point)
    }

    /// Makes the store durable if packages were added, then records that
    /// assembly started.
    fn start_assembly(&mut self) -> Result<()> {
        if self.journal.state == State::Storing {
            fsx::sync_fs(&self.lock.root().laft())?;
            fault::point("store.synced")?;
            self.set_state(State::Stored, "store.stored")?;
        }
        match self.journal.state {
            State::Planned | State::Fetching | State::Stored => {
                self.set_state(State::Assembling, "assemble.started")
            }
            State::Assembling => Ok(()),
            _ => Err(Error::Invalid("assembly has finished already".into())),
        }
    }

    fn finish_assembly(&mut self) -> Result<u64> {
        let number = self.generation();
        let root = self.lock.root();
        if self.journal.state != State::Assembling || !root.gen_dir(number).exists() {
            return Err(Error::Invalid(format!(
                "generation {number} has not been assembled"
            )));
        }
        fsx::sync_fs(&root.laft())?;
        self.set_state(State::Assembled, "assembled")?;
        Ok(number)
    }

    /// A fresh staging directory for this transaction.
    fn staging(&self, name: &str) -> Result<std::path::PathBuf> {
        let path = self
            .lock
            .root()
            .tmp_dir()
            .join(format!("{name}.{}", self.journal.txid));
        fsx::remove_tree(&path)?;
        fsx::create_dir(&path, 0o700)?;
        Ok(path)
    }
}

/// Renames a sealed staging tree into place and seals its top directory.
fn publish_tree(staging: &Path, target: &Path) -> Result<()> {
    fs::rename(staging, target).map_err(io(target))?;
    fsx::set_mode(target, fsx::SEALED_DIR)
}

fn set_default(root: &Root, number: u64, txid: &str) -> Result<()> {
    fsx::replace_symlink(
        &root.gens_dir().join("default"),
        &number.to_string(),
        &root.tmp_dir().join(format!("default.{txid}")),
    )
}

/// What recovery found and did about one interrupted transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// The interrupted transaction.
    pub txid: String,
    /// Its command.
    pub command: String,
    /// The state it was interrupted in.
    pub state: State,
    /// The generation it built.
    pub generation: Option<u64>,
    /// What recovery did.
    pub outcome: Outcome,
}

/// What recovery did about an interrupted transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing visible had changed, or the partial generation and views were
    /// removed. Store entries that were already durable stay until garbage
    /// collection.
    RolledBack,
    /// The store entries that the transaction had added, which may not have
    /// been durable, were removed.
    RemovedEntries(Vec<String>),
    /// The generation is complete but was never activated. It is kept.
    Built,
    /// The generation was activated, and recovery made it the default.
    MadeDefault,
    /// Activation was interrupted during this boot. The journal is kept for
    /// the activation step to resume.
    ActivationInterrupted,
    /// The machine rebooted during activation, so it runs the default
    /// generation. The new generation is kept.
    RebootedDuringActivation,
}

impl fmt::Display for Recovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "recovered: transaction {} ({}) was interrupted while {}",
            self.txid, self.command, self.state
        )?;
        let number = self.generation.unwrap_or_default();
        if self.generation.is_some() {
            write!(f, " generation {number}")?;
        }
        match &self.outcome {
            Outcome::RolledBack => write!(f, "\n  rolled back"),
            Outcome::RemovedEntries(dirs) => {
                write!(
                    f,
                    "\n  rolled back, removed {} new store entries",
                    dirs.len()
                )
            }
            Outcome::Built => write!(
                f,
                "\n  generation {number} was built but not activated. Run 'laft switch {number}' to use it."
            ),
            Outcome::MadeDefault => write!(f, "\n  generation {number} is now the default"),
            Outcome::ActivationInterrupted => {
                write!(f, "\n  activation of generation {number} must be resumed")
            }
            Outcome::RebootedDuringActivation => write!(
                f,
                "\n  the machine rebooted, so generation {number} was not activated"
            ),
        }
    }
}

impl Lock {
    /// Finishes or rolls back every interrupted transaction, according to
    /// the state its journal recorded, and reports what it did.
    ///
    /// | State | Recovery |
    /// |---|---|
    /// | planned, fetching | nothing to undo |
    /// | storing | the entries the transaction added are removed |
    /// | stored, assembling | the partial generation and views are removed |
    /// | assembled, boot-installed | the generation is kept but not made the default |
    /// | activating in the same boot, or when either boot id is unknown | nothing is done, the journal stays for activation to resume |
    /// | activating after a reboot | the generation is kept but not made the default |
    /// | activated, complete | the generation is made the default |
    ///
    /// In every case except an activation that stays, the staging
    /// directory is emptied, the filesystem synced, and the journal removed.
    ///
    /// # Errors
    ///
    /// Fails when a journal cannot be read or the tree cannot be changed.
    /// Recovery can be run again after a failure.
    pub fn recover(&self) -> Result<Vec<Recovery>> {
        let root = self.root();
        let mut recovered = Vec::new();
        for journal in Journal::all(root)? {
            recovered.push(recover(root, journal)?);
        }
        Ok(recovered)
    }
}

fn recover(root: &Root, journal: Journal) -> Result<Recovery> {
    let tmp = root.tmp_dir();
    let store = root.store();
    let outcome = match journal.state {
        State::Planned | State::Fetching => Outcome::RolledBack,
        State::Storing => {
            for dir in &journal.added {
                fsx::retire(&store.path(dir), &tmp.join(format!("trash-{dir}")))?;
            }
            Outcome::RemovedEntries(journal.added.clone())
        }
        State::Stored | State::Assembling => {
            if let Some(number) = journal.generation {
                fsx::retire(
                    &root.gen_dir(number),
                    &tmp.join(format!("trash-gen-{number}")),
                )?;
            }
            for (name, version) in &journal.views {
                let trash = tmp.join(format!("trash-view-{name}-{version}"));
                fsx::retire(&root.view_dir(name, *version), &trash)?;
            }
            Outcome::RolledBack
        }
        State::Assembled | State::BootInstalled => Outcome::Built,
        State::Activating if same_boot(&journal.boot_id, &crate::journal::boot_id()) => {
            return Ok(report(journal, Outcome::ActivationInterrupted));
        }
        State::Activating => Outcome::RebootedDuringActivation,
        State::Activated | State::Complete => {
            if let Some(number) = journal.generation {
                set_default(root, number, &journal.txid)?;
            }
            Outcome::MadeDefault
        }
    };
    fsx::clear_dir(&tmp)?;
    fsx::sync_fs(&root.laft())?;
    journal.remove(root)?;
    Ok(report(journal, outcome))
}

/// Whether the machine still runs the boot `recorded` names. An empty id is
/// unknown, and an unknown boot counts as the same one, so that recovery
/// keeps the journal instead of assuming a reboot it cannot prove.
fn same_boot(recorded: &str, current: &str) -> bool {
    recorded.is_empty() || current.is_empty() || recorded == current
}

fn report(journal: Journal, outcome: Outcome) -> Recovery {
    Recovery {
        txid: journal.txid,
        command: journal.command,
        state: journal.state,
        generation: journal.generation,
        outcome,
    }
}

#[cfg(test)]
#[path = "transaction_tests.rs"]
mod tests;
