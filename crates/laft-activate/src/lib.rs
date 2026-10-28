//! The static `/etc` tree of a Laft generation, the account databases and
//! the activation of `/etc` on a live system.
//!
//! The configuration reaches this crate as plain values ([`EtcConfig`]).
//! [`declare`] resolves the configured accounts into [`Declared`] accounts
//! with ids that stay stable across generations, and [`render`] builds the
//! static `/etc` tree of a generation from packages, derived files and
//! declared files. At activation,
//! [`apply_accounts`] merges a generation's accounts into `/etc/passwd`,
//! `/etc/group`, `/etc/shadow` and `/etc/gshadow`.
//!
//! The live `/etc` is a real directory. Managed files are symlinks into
//! `/etc/static`, which follows the active generation, so switching
//! generations switches their content in one step. [`reconcile_etc`]
//! creates and removes those symlinks and writes copy-mode files, and never
//! overwrites what Laft did not create unless it is adopted.
//! [`reset_etc`] restores paths the user replaced, and [`activate_boot`]
//! makes `/etc` and the accounts match the booted generation.
//!
//! Every function works below a [`laft_store::Root`], so an installer or a
//! test can point it at any directory.

#![forbid(unsafe_code)]

mod accounts;
mod boot;
mod config;
mod error;
mod etc;
mod format;
mod fsx;
mod render;
mod state;

#[cfg(test)]
mod test_support;

pub use accounts::{
    AccountsReport, Allocations, Declared, DeclaredGroup, DeclaredUser, Password, apply_accounts,
    declare,
};
pub use boot::{BootActivation, activate_boot};
pub use config::{EtcConfig, File, Group, Kernel, User, Users};
pub use error::{ConfigError, Conflict, ConflictKind, Error, Result, Warning};
pub use etc::{EtcReport, OnConflict, check_etc, reconcile_etc, reset_etc};
pub use render::{EtcTree, Node, render};
