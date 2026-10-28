//! Triggers: caches and indexes that are a function of a tree's contents,
//! computed while the tree is assembled, by fixed programs from the tree
//! itself running in a sandbox.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirBuilder};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use laft_core::{Instances, StoreId, TRIGGER_TABLE_VERSION, TRIGGERS, Trigger, glob_matches};
use laft_sandbox::{FixedSandbox, NOBODY};
use laft_store::{TriggerRecord, TriggerStatus};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, Warning, corrupt, io};
use crate::{fsx, tree};

/// How long one run of a trigger's tool may take before it is killed.
const TIMEOUT: Duration = Duration::from_secs(300);

/// The most of a tool's standard error that a message quotes, in bytes.
const STDERR_LIMIT: usize = 4096;

/// The name of the trigger records in a generation.
pub(crate) const RECORDS_FILE: &str = "triggers.json";

/// The format version of `triggers.json`.
const RECORDS_VERSION: u32 = 1;

/// A store entry whose files are in the tree, with the triggers its
/// payload activates.
pub(crate) struct Contributor {
    pub(crate) id: StoreId,
    pub(crate) triggers: Vec<String>,
}

/// Runs every trigger instance of the tree `tree`, which holds `usr` and
/// `etc`, in table order, and returns a record per instance.
///
/// `owner` names the store entry that contributes a tree path, and
/// `previous` is the directory of the previous tree of the same view,
/// whose outputs are reused when an instance's fingerprint is unchanged.
/// A failed optional instance adds a warning to `warnings`.
pub(crate) fn run(
    tree: &Path,
    contributors: &[Contributor],
    owner: impl Fn(&str) -> Option<StoreId>,
    previous: Option<&Path>,
    warnings: &mut Vec<Warning>,
) -> Result<BTreeMap<String, TriggerRecord>> {
    let reusable = match previous {
        Some(dir) => read_records(dir)?,
        None => BTreeMap::new(),
    };
    let etc = etc_digest(tree)?;
    let mut records = BTreeMap::new();
    for trigger in TRIGGERS {
        let mut inputs: Vec<String> = contributors
            .iter()
            .filter(|c| c.triggers.iter().any(|t| t == trigger.name))
            .map(|c| c.id.to_string())
            .collect();
        inputs.sort();
        inputs.dedup();
        if inputs.is_empty() {
            continue;
        }
        for key in instances(tree, trigger)? {
            let instance = match &key {
                Some(key) => format!("{}:{key}", trigger.name),
                None => trigger.name.to_owned(),
            };
            let mut tool = None;
            for candidate in trigger.tools {
                if tree::lstat(tree, &candidate[1..])?.is_some() {
                    tool = Some(candidate);
                    break;
                }
            }
            let Some(tool) = tool else {
                let fingerprint = fingerprint(trigger, &instance, &inputs, None, &etc)?;
                records.insert(
                    instance,
                    record(fingerprint, TriggerStatus::Skipped, vec![]),
                );
                continue;
            };
            let tool_id = owner(&tool[1..]);
            let fingerprint = fingerprint(trigger, &instance, &inputs, tool_id, &etc)?;
            let outputs: Vec<String> = trigger
                .outputs
                .iter()
                .map(|o| o.replace("{}", key.as_deref().unwrap_or_default()))
                .collect();
            if let (Some(dir), Some(old)) = (previous, reusable.get(&instance))
                && old.fingerprint == fingerprint
                && old.status == TriggerStatus::Ok
                && reuse(dir, tree, &old.outputs).is_ok()
            {
                records.insert(instance, old.clone());
                continue;
            }
            let run = Run {
                tree,
                trigger,
                key: key.as_deref(),
                tool,
                outputs: &outputs,
            };
            match run.execute() {
                Ok(produced) => {
                    records.insert(instance, record(fingerprint, TriggerStatus::Ok, produced));
                }
                Err(Failure::Fatal(error)) => return Err(error),
                Err(failure) if trigger.required => return Err(failure.into_error(instance)),
                Err(failure) => {
                    warnings.push(Warning::TriggerFailed {
                        instance: instance.clone(),
                        reason: failure.to_string(),
                    });
                    records.insert(instance, record(fingerprint, TriggerStatus::Failed, vec![]));
                }
            }
        }
    }
    Ok(records)
}

fn record(
    fingerprint: laft_core::Blake3Hash,
    status: TriggerStatus,
    outputs: Vec<String>,
) -> TriggerRecord {
    TriggerRecord {
        fingerprint,
        status,
        outputs,
    }
}

/// The trigger records of a tree, stored as `triggers.json`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Records {
    /// The format version, [`RECORDS_VERSION`].
    version: u32,
    /// The record of every trigger instance.
    records: BTreeMap<String, TriggerRecord>,
}

/// Only the version of `triggers.json`, which is read first.
#[derive(Deserialize)]
struct RecordsVersion {
    version: u32,
}

/// Writes `records` as `triggers.json` into the tree directory `dir`, with
/// mode `0o444`.
pub(crate) fn write_records(dir: &Path, records: &BTreeMap<String, TriggerRecord>) -> Result<()> {
    let path = dir.join(RECORDS_FILE);
    let value = Records {
        version: RECORDS_VERSION,
        records: records.clone(),
    };
    let bytes = laft_core::to_canonical_json(&value)
        .map_err(|e| corrupt(&path, format!("not canonical JSON: {e}")))?;
    fsx::write_new(&path, &bytes, 0o444)
}

/// The trigger records of the tree directory `dir`. They only serve to
/// reuse outputs, so a tree without `triggers.json`, or with one of
/// another format version, has none.
///
/// # Errors
///
/// Fails when the file cannot be read, or claims the current version but
/// does not match it.
pub(crate) fn read_records(dir: &Path) -> Result<BTreeMap<String, TriggerRecord>> {
    let path = dir.join(RECORDS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(io(&path)(e)),
    };
    match serde_json::from_slice::<RecordsVersion>(&bytes) {
        Ok(found) if found.version == RECORDS_VERSION => {}
        _ => return Ok(BTreeMap::new()),
    }
    let records: Records = serde_json::from_slice(&bytes).map_err(|e| corrupt(&path, e))?;
    Ok(records.records)
}

/// The instance keys of `trigger` in `tree`: `None` for a trigger with one
/// instance, otherwise every kernel release or icon theme, sorted.
fn instances(tree: &Path, trigger: &Trigger) -> Result<Vec<Option<String>>> {
    let parent = match trigger.instances {
        Instances::One => return Ok(vec![None]),
        Instances::Kernel => "usr/lib/modules",
        Instances::IconTheme => "usr/share/icons",
    };
    let mut keys = Vec::new();
    for name in tree::subdirs(tree, parent)? {
        let dir = format!("{parent}/{name}");
        let wanted = match trigger.instances {
            Instances::IconTheme => tree::lstat(tree, &format!("{dir}/index.theme"))?.is_some(),
            _ => has_activating_file(tree, &dir, trigger)?,
        };
        if wanted {
            keys.push(Some(name));
        }
    }
    Ok(keys)
}

/// Whether a file or symlink below `dir` activates `trigger`.
fn has_activating_file(tree: &Path, dir: &str, trigger: &Trigger) -> Result<bool> {
    for path in tree::walk(tree, dir)? {
        if trigger.activated_by(&path) && !tree::lstat(tree, &path)?.is_some_and(|m| m.is_dir()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What a fingerprint covers: everything a trigger's output depends on.
/// The tools run with the tree's `etc` as `/etc` and may read it, so its
/// whole digest is included.
#[derive(Serialize)]
struct Fingerprint<'a> {
    trigger: &'a str,
    instance: &'a str,
    table: u32,
    inputs: &'a [String],
    tool: Option<String>,
    etc: String,
}

fn fingerprint(
    trigger: &Trigger,
    instance: &str,
    inputs: &[String],
    tool: Option<StoreId>,
    etc: &laft_core::Blake3Hash,
) -> Result<laft_core::Blake3Hash> {
    let value = Fingerprint {
        trigger: trigger.name,
        instance,
        table: TRIGGER_TABLE_VERSION,
        inputs,
        tool: tool.map(|id| id.to_string()),
        etc: etc.to_string(),
    };
    let bytes = laft_core::to_canonical_json(&value)
        .map_err(|e| Error::Invalid(format!("trigger {instance}: {e}")))?;
    Ok(laft_core::Blake3Hash::of(&bytes))
}

/// The digest of the tree's `etc`: the type, permission bits and content or
/// link target of every entry.
fn etc_digest(tree: &Path) -> Result<laft_core::Blake3Hash> {
    let mut entries = BTreeMap::new();
    for path in tree::walk(tree, "etc")? {
        let full = tree.join(&path);
        let metadata = fs::symlink_metadata(&full).map_err(io(&full))?;
        let mode = metadata.permissions().mode() & 0o7777;
        let entry = if metadata.is_dir() {
            format!("dir {mode:o}")
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&full).map_err(io(&full))?;
            format!("link {}", target.to_string_lossy())
        } else {
            let bytes = fs::read(&full).map_err(io(&full))?;
            format!("file {mode:o} {}", laft_core::Blake3Hash::of(&bytes))
        };
        entries.insert(path, entry);
    }
    let bytes = laft_core::to_canonical_json(&entries)
        .map_err(|e| Error::Invalid(format!("the digest of etc: {e}")))?;
    Ok(laft_core::Blake3Hash::of(&bytes))
}

/// Links the outputs `outputs` of the previous tree `old` into `tree`.
/// Either every output is linked or none is.
fn reuse(old: &Path, tree: &Path, outputs: &[String]) -> Result<()> {
    let mut made = Vec::new();
    let result = (|| {
        for path in outputs {
            let (from, to) = (old.join(path), tree.join(path));
            let metadata = fs::symlink_metadata(&from).map_err(io(&from))?;
            if metadata.is_dir() {
                DirBuilder::new().mode(0o755).create(&to).map_err(io(&to))?;
            } else if metadata.file_type().is_symlink() {
                let target = fs::read_link(&from).map_err(io(&from))?;
                symlink(target, &to).map_err(io(&to))?;
            } else {
                fs::hard_link(&from, &to).map_err(io(&to))?;
            }
            made.push(to);
        }
        Ok(())
    })();
    if result.is_err() {
        for path in made.iter().rev() {
            remove(path);
        }
    }
    result
}

/// Removes a file, symlink or empty directory, ignoring failures, because
/// it only undoes what a failed step created.
fn remove(path: &Path) {
    let _ = fs::remove_file(path).or_else(|_| fs::remove_dir(path));
}

/// Why a trigger run failed.
enum Failure {
    /// The sandbox could not be set up.
    Sandbox(laft_sandbox::Error),
    /// The tool failed or timed out.
    Tool(String),
    /// Something that fails the assembly even for an optional trigger.
    Fatal(Error),
}

impl Failure {
    fn into_error(self, instance: String) -> Error {
        match self {
            Self::Sandbox(source) => Error::Sandbox { instance, source },
            Self::Tool(reason) => Error::Trigger { instance, reason },
            Self::Fatal(error) => error,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sandbox(e) => write!(f, "cannot set up the sandbox: {e}"),
            Self::Tool(reason) => f.write_str(reason),
            Self::Fatal(e) => e.fmt(f),
        }
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Fatal(error)
    }
}

/// One trigger instance to run.
struct Run<'a> {
    tree: &'a Path,
    trigger: &'a Trigger,
    key: Option<&'a str>,
    tool: &'a str,
    outputs: &'a [String],
}

impl Run<'_> {
    /// Runs the instance and returns its outputs, sorted. On failure,
    /// everything the run added to the tree is removed again.
    fn execute(&self) -> Result<Vec<String>, Failure> {
        let mut created = Vec::new();
        let mut before = BTreeMap::new();
        let result = self.execute_into(&mut created, &mut before);
        if result.is_err() {
            let mut added: BTreeSet<String> = created.into_iter().collect();
            for output in self.outputs {
                let found = tree::walk(self.tree, output).unwrap_or_default();
                added.extend(found.into_iter().filter(|p| !before.contains_key(p)));
            }
            for path in added.iter().rev() {
                remove(&self.tree.join(path));
            }
        }
        result
    }

    /// The steps of [`Self::execute`]. Fills `created` with the directories
    /// it creates and `before` with the identity of every entry the output
    /// directories held before the tool ran.
    fn execute_into(
        &self,
        created: &mut Vec<String>,
        before: &mut BTreeMap<String, Identity>,
    ) -> Result<Vec<String>, Failure> {
        let root = fsx::is_root();
        for output in self.outputs {
            if let Err(reason) = self.create_output(output, created)? {
                return Err(Failure::Tool(reason));
            }
            for path in std::iter::once(output.clone()).chain(tree::walk(self.tree, output)?) {
                if let Some(metadata) = tree::lstat(self.tree, &path)? {
                    before.insert(path, Identity::of(&metadata));
                }
            }
            if root {
                chown(&self.tree.join(output), NOBODY)?;
            }
        }
        self.run_tool()?;
        let after = self.settle(before, root)?;
        let mut outputs: BTreeSet<String> = created.iter().cloned().collect();
        outputs.extend(after);
        Ok(outputs.into_iter().collect())
    }

    /// Creates the output directory `output` and its missing parents,
    /// adding each one it creates to `created`. An existing path that is
    /// not a plain directory is refused with the reason.
    fn create_output(&self, output: &str, created: &mut Vec<String>) -> Result<Result<(), String>> {
        let mut path = String::new();
        for part in output.split('/') {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            match tree::lstat(self.tree, &path)? {
                Some(metadata) if metadata.is_dir() => {}
                Some(_) => return Ok(Err(format!("/{path} is not a directory"))),
                None => {
                    let dir = self.tree.join(&path);
                    DirBuilder::new()
                        .mode(0o755)
                        .create(&dir)
                        .map_err(io(&dir))?;
                    created.push(path.clone());
                }
            }
        }
        Ok(Ok(()))
    }

    /// Runs the tool once, or once per matching file.
    fn run_tool(&self) -> Result<(), Failure> {
        let Some(glob) = self.trigger.each_file else {
            return self.run_once(self.key.unwrap_or_default());
        };
        let base = glob
            .split('/')
            .take_while(|part| !part.contains('*'))
            .collect::<Vec<_>>()
            .join("/");
        for path in tree::walk(self.tree, &base)? {
            let is_dir = tree::lstat(self.tree, &path)?.is_some_and(|m| m.is_dir());
            if !is_dir && glob_matches(glob, &path) {
                self.run_once(&format!("/{path}"))?;
            }
        }
        Ok(())
    }

    fn run_once(&self, value: &str) -> Result<(), Failure> {
        let mut sandbox = FixedSandbox::new(self.tree, self.tree.join("etc"));
        for output in self.outputs {
            sandbox = sandbox.writable(output);
        }
        let args: Vec<String> = self
            .trigger
            .args
            .iter()
            .map(|a| a.replace("{}", value))
            .collect();
        let mut child = sandbox
            .command(self.tool)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Failure::Sandbox)?;
        let mut stderr = child.stderr.take();
        let reader = thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(stderr) = stderr.as_mut() {
                let _ = stderr.read_to_end(&mut text);
            }
            text
        });
        let pid = child.id();
        let (done, finished) = mpsc::channel();
        let waiter = thread::spawn(move || {
            let _ = done.send(child.wait());
        });
        let status = match finished.recv_timeout(TIMEOUT) {
            Ok(status) => status,
            Err(_) => {
                // The waiter has not reaped the child, so the pid still
                // names it. Killing it kills every process of the sandbox.
                if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
                let _ = waiter.join();
                let _ = reader.join();
                return Err(Failure::Tool(format!(
                    "{} did not finish within {} seconds",
                    self.tool,
                    TIMEOUT.as_secs()
                )));
            }
        };
        let _ = waiter.join();
        let stderr = reader.join().unwrap_or_default();
        let status =
            status.map_err(|e| Failure::Tool(format!("waiting for {}: {e}", self.tool)))?;
        if status.success() {
            return Ok(());
        }
        let mut text = String::from_utf8_lossy(&stderr).trim_end().to_owned();
        if text.len() > STDERR_LIMIT {
            let mut cut = text.len() - STDERR_LIMIT;
            while !text.is_char_boundary(cut) {
                cut += 1;
            }
            text = format!("...{}", &text[cut..]);
        }
        let mut reason = format!("{} {} failed with {status}", self.tool, args.join(" "));
        if !text.is_empty() {
            reason = format!("{reason}:\n{text}");
        }
        Err(Failure::Tool(reason))
    }

    /// After a run: gives every new entry of the output directories back to
    /// root and makes new files read-only, checks that every entry that was
    /// there before is still the same, unchanged file, and returns the new
    /// entries.
    fn settle(
        &self,
        before: &BTreeMap<String, Identity>,
        root: bool,
    ) -> Result<Vec<String>, Failure> {
        let mut new = Vec::new();
        for output in self.outputs {
            if root {
                chown(&self.tree.join(output), 0)?;
            }
            for path in tree::walk(self.tree, output)? {
                if before.contains_key(&path) {
                    continue;
                }
                let full = self.tree.join(&path);
                let metadata = fs::symlink_metadata(&full).map_err(io(&full))?;
                if root {
                    rustix::fs::chownat(
                        rustix::fs::CWD,
                        &full,
                        Some(rustix::process::Uid::ROOT),
                        Some(rustix::process::Gid::ROOT),
                        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
                    )
                    .map_err(|e| io(&full)(e.into()))?;
                }
                if metadata.is_file() {
                    let mode = metadata.permissions().mode() & 0o7777 & !0o7222;
                    fs::set_permissions(&full, fs::Permissions::from_mode(mode))
                        .map_err(io(&full))?;
                }
                new.push(path);
            }
        }
        for (path, identity) in before {
            let same = tree::lstat(self.tree, path)?.is_some_and(|m| Identity::of(&m) == *identity);
            if !same {
                return Err(Failure::Fatal(Error::Trigger {
                    instance: self.trigger.name.to_owned(),
                    reason: format!(
                        "{} replaced, changed or removed /{path}, which it may not change",
                        self.tool
                    ),
                }));
            }
        }
        Ok(new)
    }
}

/// What identifies an entry that existed before a tool ran. A directory is
/// its inode, because adding entries to it is allowed. Anything else also
/// carries its change time, which every write, permission change and link
/// updates, so that a file rewritten in place is noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    ctime: Option<(i64, i64)>,
}

impl Identity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            ctime: (!metadata.is_dir()).then(|| (metadata.ctime(), metadata.ctime_nsec())),
        }
    }
}

/// Gives the directory `path` to uid and gid `id`.
fn chown(path: &Path, id: u32) -> Result<()> {
    let (uid, gid) = (
        rustix::process::Uid::from_raw(id),
        rustix::process::Gid::from_raw(id),
    );
    rustix::fs::chown(path, Some(uid), Some(gid)).map_err(|e| io(path)(e.into()))
}

#[cfg(test)]
#[path = "trigger_tests.rs"]
mod tests;
