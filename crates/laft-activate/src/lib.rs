//! Assembly of Laft generations and their activation: the static `/etc`
//! tree, the account databases, triggers, privileged copies, the initramfs
//! and the boot files.
//!
//! [`assemble`] builds a complete generation inside a store transaction
//! from resolved packages and the configuration as plain values: the
//! system view with the outputs of its triggers and its privileged copies,
//! `/etc`, the services, the accounts and the initramfs with its boot
//! record. [`install_boot`] copies the kernels and initramfs files of every
//! generation to the EFI system partition and renders the boot menu.
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
mod assemble;
mod boot;
mod config;
mod error;
mod esp;
mod etc;
mod fault;
mod format;
mod fsx;
mod initramfs;
mod privileged;
mod render;
mod state;
mod tree;
mod trigger;

#[cfg(test)]
mod test_support;

pub use accounts::{
    AccountsReport, Allocations, Declared, DeclaredGroup, DeclaredUser, Password, apply_accounts,
    declare,
};
pub use assemble::{Assembled, Assembly, assemble, recover};
pub use boot::{BootActivation, activate_boot};
pub use config::{
    Boot, BootConfig, EtcConfig, File, Fs, Group, Initrd, Kernel, Loader, Luks, Microcode,
    Privileged, Security, User, Users,
};
pub use error::{ConfigError, Conflict, ConflictKind, Error, Result, Warning};
pub use esp::{BootInstall, check_esp, install_boot};
pub use etc::{EtcReport, OnConflict, check_etc, reconcile_etc, reset_etc};
pub use initramfs::{BootRecord, Host};
pub use privileged::{PRIVILEGED_FILE, PrivilegedFile, PrivilegedRecord, ViewPrivileges};
pub use render::{EtcTree, Node, render};
