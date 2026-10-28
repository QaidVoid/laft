//! Boot trials: the write side of the boot state in
//! `laft/state/boot.json`, which `laft-boot` reads to count the boots of a
//! new default generation and to fall back when it never completes one.
//!
//! [`install_boot`](crate::install_boot) starts a trial for a new boot
//! default with [`start_trial`]. Once the system has booted completely,
//! [`mark_booted_good`] ends it. Every write replaces the file atomically.

use std::fs;
use std::io;

use laft_core::{BootEvent, BootState, BootTrial};
use laft_store::{Lock, Root};

use crate::error::{Result, corrupt, io};
use crate::{boot, fsx, state};

/// The name of the boot state in `laft/state`.
const BOOT_STATE: &str = "boot.json";

/// Reads the boot state of `root`, or an empty one when there is none.
fn load(root: &Root) -> Result<BootState> {
    let path = state::state_file(root, BOOT_STATE);
    match fs::read(&path) {
        Ok(bytes) => BootState::parse(&bytes).map_err(|e| corrupt(&path, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BootState::default()),
        Err(e) => Err(io(&path)(e)),
    }
}

/// Writes `state` as canonical JSON, replacing the old file atomically.
fn save(root: &Root, state: &BootState) -> Result<()> {
    let path = state::state_file(root, BOOT_STATE);
    let dir = fsx::parent(&path);
    fs::create_dir_all(dir).map_err(io(dir))?;
    let bytes = state
        .to_json()
        .map_err(|e| corrupt(&path, format!("not canonical JSON: {e}")))?;
    fsx::write_atomic(&path, &bytes, 0o644, (0, 0))
}

/// Puts `generation`, the new boot default, on trial with `tries` boots,
/// and returns the trial that is in effect.
///
/// A trial of the same generation is kept as it is, so installing the
/// boot files again never gives back the tries it used. A generation that
/// completed a boot before needs no trial, and `tries` 0 disables boot
/// counting, so both end any trial instead.
///
/// # Errors
///
/// Fails when the boot state cannot be read or written. An unreadable
/// state is left as it is.
pub fn start_trial(root: &Root, generation: u64, tries: u32) -> Result<Option<BootTrial>> {
    let mut state = load(root)?;
    let trial = if tries == 0 || state.good.contains(&generation) {
        None
    } else {
        match state.trial {
            Some(trial) if trial.generation == generation => Some(trial),
            _ => Some(BootTrial {
                generation,
                tries_left: tries,
            }),
        }
    };
    if state.trial != trial {
        state.trial = trial;
        save(root, &state)?;
    }
    Ok(trial)
}

/// Records that `generation` completed a boot: it becomes the last good
/// generation, is no longer counted as bad, and its trial ends. A trial of
/// another generation stays. Returns the new state.
///
/// # Errors
///
/// Fails when the boot state cannot be read or written. An unreadable
/// state is left as it is.
pub fn mark_good(lock: &Lock, generation: u64) -> Result<BootState> {
    let root = lock.root();
    let mut state = load(root)?;
    state.good.insert(generation);
    state.bad.remove(&generation);
    state.last_good = Some(generation);
    if state.trial.is_some_and(|t| t.generation == generation) {
        state.trial = None;
    }
    save(root, &state)?;
    Ok(state)
}

/// Marks the generation that `run/laft/booted` names as good, as
/// [`mark_good`] does, and returns its number. This is what runs once the
/// system has booted completely.
///
/// ```no_run
/// use laft_activate::mark_booted_good;
/// use laft_store::Root;
///
/// let lock = Root::default().lock("boot-complete")?;
/// let generation = mark_booted_good(&lock)?;
/// println!("generation {generation} booted completely");
/// # Ok::<(), laft_activate::Error>(())
/// ```
///
/// # Errors
///
/// Fails when `run/laft/booted` does not name a generation, and as
/// [`mark_good`] does.
pub fn mark_booted_good(lock: &Lock) -> Result<u64> {
    let generation = boot::booted(lock.root())?;
    mark_good(lock, generation)?;
    Ok(generation)
}

/// Records `event` in the boot state, which keeps only the most recent
/// events.
///
/// # Errors
///
/// Fails when the boot state cannot be read or written.
pub fn record_event(lock: &Lock, event: BootEvent) -> Result<()> {
    let root = lock.root();
    let mut state = load(root)?;
    state.push_event(event);
    save(root, &state)
}

#[cfg(test)]
#[path = "trial_tests.rs"]
mod tests;
