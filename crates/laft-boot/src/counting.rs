//! Boot counting and the automatic fallback to the last good generation.
//!
//! A new boot default is on trial until one of its boots completes. Each boot
//! of it consumes one try. When none are left, it is marked bad and the
//! machine falls back to the last generation that booted completely.

use std::collections::BTreeSet;

use laft_core::{BootEvent, BootEventKind, BootState, BootTrial, Timestamp};
use serde::Deserialize;

use crate::generation::Generation;

/// What boot counting does for one boot, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The steps, carried out in order.
    pub(crate) steps: Vec<Step>,
    /// How the boot goes on once every step is done.
    pub(crate) then: Then,
}

/// One step of a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Points `/laft/gens/default` at generation `to`. A failure is logged
    /// and ends the plan early: no later step is carried out and
    /// `otherwise` boots.
    SetDefault {
        /// The generation the link names.
        to: u64,
        /// The generation that boots when the link cannot be changed.
        otherwise: Generation,
    },
    /// Makes the default boot menu entry the one of generation `to`. A
    /// failure is logged and ends the plan early: no later step is carried
    /// out and `otherwise` boots.
    SetBootEntry {
        /// The generation the menu starts by default.
        to: u64,
        /// The generation that boots when the menu cannot be changed.
        otherwise: Generation,
    },
    /// Writes the state. A failure is logged and the plan goes on.
    Save(BootState),
    /// Logs a warning.
    Warn(String),
    /// Logs an error.
    Error(String),
}

/// How the boot goes on after a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Then {
    /// The generation boots now.
    Boot(Generation),
    /// The machine restarts, so that the boot loader starts another kernel.
    Reboot,
}

/// Counts a boot of `booting` against its trial and plans what follows.
///
/// Nothing changes when `notries` is set, when boot counting is disabled
/// with `tries` 0, or when no trial is affected. A boot of the generation
/// on trial consumes one try. When none is left, the generation is marked
/// bad and the last good generation, read with `read`, replaces it: the
/// default link and the boot menu are pointed at it, and only then is the
/// state saved that ends the trial. A boot that stops before the save, or
/// cannot change the default link or the boot menu, therefore leaves the
/// trial used up, and the next boot falls back again. A boot of the last good generation while
/// the trial is used up finishes such a fallback. The new generation boots
/// in place when it has the same kernel and initramfs as `booting`, and
/// through a reboot otherwise. `now` stamps the fallback event.
pub(crate) fn decide(
    mut state: BootState,
    booting: &Generation,
    tries: u32,
    notries: bool,
    read: impl Fn(u64) -> Result<Generation, String>,
    now: Timestamp,
) -> Plan {
    let boot = |steps| Plan {
        steps,
        then: Then::Boot(booting.clone()),
    };
    let number = booting.number;
    let Some(trial) = state.trial else {
        return boot(Vec::new());
    };
    if notries || tries == 0 {
        return boot(Vec::new());
    }
    if trial.generation == number && trial.tries_left > 0 {
        state.trial = Some(BootTrial {
            tries_left: trial.tries_left - 1,
            ..trial
        });
        return boot(vec![Step::Save(state)]);
    }
    let failed = trial.generation;
    let finishing = failed != number && state.last_good == Some(number);
    if trial.tries_left > 0 || (failed != number && !finishing) {
        return boot(Vec::new());
    }
    state.bad.insert(failed);
    state.trial = None;
    let target = if finishing {
        Ok(booting.clone())
    } else {
        match state.last_good.filter(|&t| t != failed) {
            None => Err("there is no known good generation".to_owned()),
            Some(t) => read(t)
                .map_err(|why| format!("the last good generation {t} cannot be booted ({why})")),
        }
    };
    let target = match target {
        Ok(target) => target,
        Err(why) => {
            return boot(vec![
                Step::Error(format!(
                    "generation {failed} failed {tries} boots and {why}; booting it anyway"
                )),
                Step::Save(state),
            ]);
        }
    };
    let to = target.number;
    state.push_event(BootEvent {
        from: failed,
        kind: BootEventKind::Fallback,
        time: now,
        to,
    });
    let in_place = target.kernel_id == booting.kernel_id && target.initramfs == booting.initramfs;
    let (otherwise, then, how) = if in_place {
        (target.clone(), Then::Boot(target), "continuing with")
    } else {
        (booting.clone(), Then::Reboot, "rebooting into")
    };
    Plan {
        steps: vec![
            Step::SetDefault {
                to,
                otherwise: otherwise.clone(),
            },
            Step::SetBootEntry { to, otherwise },
            Step::Save(state),
            Step::Warn(format!(
                "generation {failed} failed {tries} boots; {how} generation {to}"
            )),
        ],
        then,
    }
}

#[derive(Deserialize)]
struct Entries {
    version: u32,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "gen")]
    generation: u64,
    index: u64,
}

/// The boot menu index of `generation` according to the menu index file
/// `entries.json` in `bytes`, or `None` when the menu has no entry for it.
///
/// # Errors
///
/// Fails when the document is not a version 1 menu index.
pub(crate) fn menu_index(bytes: &[u8], generation: u64) -> Result<Option<u64>, String> {
    let entries: Entries = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if entries.version != 1 {
        return Err(format!("unsupported version {}", entries.version));
    }
    Ok(entries
        .entries
        .iter()
        .find(|e| e.generation == generation)
        .map(|e| e.index))
}

/// The generation each top-level entry of the boot loader configuration
/// `conf` boots, in menu order: the number in a `/Laft <generation>` title,
/// or `None` for an entry with another title. Nested entries, whose titles
/// start with `//`, are not positions of the top-level menu.
fn menu_generations(conf: &str) -> Vec<Option<u64>> {
    conf.lines()
        .filter(|line| line.starts_with('/') && !line.starts_with("//"))
        .map(|title| {
            let number = title.strip_prefix("/Laft ")?.split(' ').next()?;
            let digits = !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
            if digits { number.parse().ok() } else { None }
        })
        .collect()
}

/// `state` pruned to the generations in `existing`, ready to be saved,
/// with a warning when they could not be listed. A failed listing leaves
/// the state unpruned, so that a read error does not erase the boot
/// history.
pub(crate) fn prune_listed(
    mut state: BootState,
    existing: Result<BTreeSet<u64>, String>,
) -> (BootState, Option<String>) {
    match existing {
        Ok(existing) => {
            state.prune(&existing);
            (state, None)
        }
        Err(e) => (
            state,
            Some(format!(
                "cannot list the generations, so the boot state is not pruned: {e}"
            )),
        ),
    }
}

/// The 1-based position of the entry of `generation` in the boot loader
/// configuration `conf`. `index` from `entries.json` is used when the entry
/// there belongs to `generation`. Otherwise the index is stale and the
/// entry is looked up by its title. `None` when no entry boots it.
pub(crate) fn menu_position(conf: &str, index: Option<u64>, generation: u64) -> Option<u64> {
    let generations = menu_generations(conf);
    let at = |position: u64| {
        let slot = usize::try_from(position.checked_sub(1)?).ok()?;
        generations.get(slot).copied().flatten()
    };
    if let Some(index) = index.filter(|&i| at(i) == Some(generation)) {
        return Some(index);
    }
    let slot = generations.iter().position(|g| *g == Some(generation))?;
    u64::try_from(slot + 1).ok()
}

/// The boot loader configuration `conf` with its default entry set to
/// `index`, or `None` when it has no `default_entry: <n>` line to replace.
/// Every other line is kept byte for byte.
pub(crate) fn set_default_entry(conf: &str, index: u64) -> Option<String> {
    let mut replaced = false;
    let mut out = String::with_capacity(conf.len());
    for line in conf.split_inclusive('\n') {
        let (body, end) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        let is_default = body
            .strip_prefix("default_entry: ")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if is_default {
            out.push_str(&format!("default_entry: {index}{end}"));
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    replaced.then_some(out)
}

#[cfg(test)]
#[path = "counting_tests.rs"]
mod tests;
