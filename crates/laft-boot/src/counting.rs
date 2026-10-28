//! Boot counting and the automatic fallback to the last good generation.
//!
//! A new boot default is on trial until one of its boots completes. Each boot
//! of it consumes one try. When none are left, it is marked bad and the
//! machine falls back to the last generation that booted completely.

use laft_core::{BootEvent, BootEventKind, BootState, Timestamp};
use serde::Deserialize;

/// What boot counting decided for one boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// The generation is not on trial. Nothing is written.
    Unchanged,
    /// One try was consumed. The new state is written and the generation
    /// boots.
    Counted(BootState),
    /// The trial is over but no good generation can replace it. The new
    /// state is written and the generation boots anyway.
    Exhausted(BootState),
    /// The trial is over and generation `to` replaces it as the boot
    /// default. The new state is written.
    Fallback {
        /// The state to write.
        state: BootState,
        /// The last good generation, which boots instead.
        to: u64,
    },
}

/// Counts a boot of `generation` against its trial.
///
/// Nothing changes when `notries` is set, when boot counting is disabled
/// with `tries` 0, or when `generation` is not the one on trial. `valid`
/// tells whether a generation can still be booted, and `now` stamps a
/// fallback event.
pub(crate) fn decide(
    mut state: BootState,
    generation: u64,
    tries: u32,
    notries: bool,
    valid: impl Fn(u64) -> bool,
    now: Timestamp,
) -> Decision {
    let Some(trial) = state.trial.as_mut() else {
        return Decision::Unchanged;
    };
    if notries || tries == 0 || trial.generation != generation {
        return Decision::Unchanged;
    }
    if trial.tries_left > 0 {
        trial.tries_left -= 1;
        return Decision::Counted(state);
    }
    state.bad.insert(generation);
    state.trial = None;
    match state.last_good.filter(|&t| t != generation && valid(t)) {
        None => Decision::Exhausted(state),
        Some(to) => {
            state.push_event(BootEvent {
                from: generation,
                kind: BootEventKind::Fallback,
                time: now,
                to,
            });
            Decision::Fallback { state, to }
        }
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
