//! The static `/etc` tree of a Laft generation, the account databases and
//! the activation of `/etc` on a live system.
//!
//! The configuration reaches this crate as plain values ([`EtcConfig`]).
//! [`declare`] resolves the configured accounts into [`Declared`] accounts
//! with ids that stay stable across generations. At activation,
//! [`apply_accounts`] merges a generation's accounts into `/etc/passwd`,
//! `/etc/group`, `/etc/shadow` and `/etc/gshadow`.

#![forbid(unsafe_code)]

mod accounts;
mod config;
mod error;
mod fsx;
mod state;

#[cfg(test)]
mod test_support;

pub use accounts::{
    AccountsReport, Allocations, Declared, DeclaredGroup, DeclaredUser, Password, apply_accounts,
    declare,
};
pub use config::{EtcConfig, File, Group, Kernel, User, Users};
pub use error::{ConfigError, Conflict, ConflictKind, Error, Result, Warning};
