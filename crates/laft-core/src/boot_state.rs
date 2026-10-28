use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CanonicalJsonError, Timestamp};

/// The only version of the boot state that is read and written.
pub const BOOT_STATE_VERSION: u32 = 1;

/// How many events the boot state keeps, the most recent ones.
pub const MAX_BOOT_EVENTS: usize = 20;

/// The boot trial and fallback state, stored in `/laft/state/boot.json` as
/// canonical JSON.
///
/// Every field but `version` may be left out and then takes its empty
/// value. Unknown fields are refused.
///
/// ```
/// use laft_core::BootState;
///
/// let state = BootState::parse(br#"{"bad":[40],"good":[41],"version":1}"#)?;
/// assert!(state.bad.contains(&40));
/// assert_eq!(state.trial, None);
/// # Ok::<(), laft_core::BootStateError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BootState {
    /// Generations that used up their trial without completing a boot.
    pub bad: BTreeSet<u64>,
    /// Fallbacks that happened, oldest first.
    pub events: Vec<BootEvent>,
    /// Generations that completed a boot.
    pub good: BTreeSet<u64>,
    /// The generation that completed a boot most recently.
    pub last_good: Option<u64>,
    /// The boot default that is on trial, if any.
    pub trial: Option<BootTrial>,
    /// The format version, [`BOOT_STATE_VERSION`].
    pub version: u32,
}

impl Default for BootState {
    fn default() -> Self {
        Self {
            bad: BTreeSet::new(),
            events: Vec::new(),
            good: BTreeSet::new(),
            last_good: None,
            trial: None,
            version: BOOT_STATE_VERSION,
        }
    }
}

/// A generation on trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootTrial {
    /// The generation.
    #[serde(rename = "gen")]
    pub generation: u64,
    /// The boots it may still start before it counts as failed.
    pub tries_left: u32,
}

/// Something that happened to the boot default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootEvent {
    /// The generation that failed.
    pub from: u64,
    /// What happened.
    pub kind: BootEventKind,
    /// When it happened.
    pub time: Timestamp,
    /// The generation that replaced it.
    pub to: u64,
}

/// The kinds of [`BootEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BootEventKind {
    /// The boot default failed every try and was replaced.
    Fallback,
}

/// Why a boot state could not be read.
#[derive(Debug)]
pub enum BootStateError {
    /// The document is not a boot state of its version.
    Invalid(serde_json::Error),
    /// The document has a version this program does not know.
    UnsupportedVersion(u32),
}

impl fmt::Display for BootStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(e) => e.fmt(f),
            Self::UnsupportedVersion(version) => write!(f, "unsupported version {version}"),
        }
    }
}

impl std::error::Error for BootStateError {}

#[derive(Deserialize)]
struct Versioned {
    version: u32,
}

impl BootState {
    /// Reads the state from `bytes`. The version is read first, so a
    /// document of another version is reported as such whatever else it
    /// holds.
    ///
    /// # Errors
    ///
    /// Fails when the document has no version, has another version than
    /// [`BOOT_STATE_VERSION`], or is not a boot state.
    pub fn parse(bytes: &[u8]) -> Result<Self, BootStateError> {
        let versioned: Versioned =
            serde_json::from_slice(bytes).map_err(BootStateError::Invalid)?;
        if versioned.version != BOOT_STATE_VERSION {
            return Err(BootStateError::UnsupportedVersion(versioned.version));
        }
        serde_json::from_slice(bytes).map_err(BootStateError::Invalid)
    }

    /// The state as canonical JSON, the form it is stored in.
    ///
    /// # Errors
    ///
    /// Fails when a generation number is too large for canonical JSON.
    pub fn to_json(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        crate::to_canonical_json(self)
    }

    /// Forgets good and bad generations that are not in `existing`.
    pub fn prune(&mut self, existing: &BTreeSet<u64>) {
        self.good.retain(|n| existing.contains(n));
        self.bad.retain(|n| existing.contains(n));
    }

    /// Records `event` and forgets the oldest events beyond
    /// [`MAX_BOOT_EVENTS`].
    pub fn push_event(&mut self, event: BootEvent) {
        self.events.push(event);
        let excess = self.events.len().saturating_sub(MAX_BOOT_EVENTS);
        self.events.drain(..excess);
    }
}

#[cfg(test)]
#[path = "boot_state_tests.rs"]
mod tests;
