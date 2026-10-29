//! Activation of a running system.

use std::collections::BTreeSet;

use laft_store::Lock;

use crate::activate::Activation;
use crate::env::Env;
use crate::failure::Failure;

/// Switches the running system to generation `number`.
pub(crate) fn activate(
    _env: &Env,
    _lock: &Lock,
    number: u64,
    _adopt: &BTreeSet<String>,
) -> Result<Activation, Failure> {
    Err(Failure::general(format!(
        "generation {number} cannot be activated on the running system by this version"
    ))
    .hint("use --boot and reboot into it"))
}
