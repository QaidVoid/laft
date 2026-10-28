//! Activation at boot.

use std::collections::BTreeSet;
use std::fs;

use laft_store::{Lock, Recovery, Root};

use crate::error::{Error, Result, io};
use crate::etc::{EtcReport, OnConflict, reconcile_etc};
use crate::{AccountsReport, apply_accounts};

/// What activation at boot found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootActivation {
    /// The interrupted transactions that were finished or rolled back.
    pub recoveries: Vec<Recovery>,
    /// The booted generation.
    pub generation: u64,
    /// What reconciling `/etc` changed.
    pub etc: EtcReport,
    /// What writing the account databases changed.
    pub accounts: AccountsReport,
}

/// Makes `/etc` and the account databases match the booted generation,
/// before any service starts.
///
/// They can disagree after a crash during activation or after booting an
/// older generation from the boot menu. Interrupted transactions are
/// finished or rolled back first. Then `/etc` is reconciled with the
/// generation that `run/laft/booted` names, leaving conflicting paths alone
/// with a warning, because nobody can resolve them during boot. Finally
/// the generation's accounts are written, which removes accounts that only
/// a newer generation declared.
///
/// ```no_run
/// use laft_activate::activate_boot;
/// use laft_store::Root;
///
/// let lock = Root::default().lock("activate")?;
/// let activation = activate_boot(&lock)?;
/// for warning in &activation.etc.warnings {
///     eprintln!("warning: {warning}");
/// }
/// # Ok::<(), laft_activate::Error>(())
/// ```
///
/// # Errors
///
/// Fails when recovery fails, when `run/laft/booted` does not name a
/// generation, and as [`reconcile_etc`] and [`apply_accounts`] do.
pub fn activate_boot(lock: &Lock) -> Result<BootActivation> {
    let recoveries = lock.recover()?;
    let generation = booted(lock.root())?;
    let etc = reconcile_etc(lock, generation, &BTreeSet::new(), OnConflict::Skip)?;
    let accounts = apply_accounts(lock, generation)?;
    Ok(BootActivation {
        recoveries,
        generation,
        etc,
        accounts,
    })
}

/// The generation that `run/laft/booted` points at, such as `mounts/41`.
fn booted(root: &Root) -> Result<u64> {
    let link = root.path().join("run/laft/booted");
    let target = fs::read_link(&link).map_err(io(&link))?;
    target
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse().ok())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{} does not name a generation: {}",
                link.display(),
                target.display()
            ))
        })
}

#[cfg(test)]
#[path = "boot_tests.rs"]
mod tests;
