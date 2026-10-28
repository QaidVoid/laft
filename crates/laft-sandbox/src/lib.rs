//! Namespaces and mounts for Laft.
//!
//! The builder runs every build phase unprivileged in a [`Sandbox`]: new
//! user, mount, PID, IPC, UTS and network namespaces, a root that holds only
//! the build view as `/usr` plus fixed writable `/build` and `/out`
//! directories, no capabilities and `no_new_privs`. The phase that fetches
//! sources keeps the host network with [`Network::Host`].
//!
//! Every mount uses the descriptor based mount API (`open_tree`,
//! `move_mount`, `fsmount` and `mount_setattr`), so Linux 5.12 or newer is
//! required.
//!
//! This crate may use `unsafe` code. It is confined to forking, `unshare`,
//! `mount_setattr`, `close_range`, signal resets and the hook that sets up a
//! sandbox between `fork` and `exec`, each with the reason it is sound next
//! to it.

mod error;
mod sandbox;
mod sys;

#[cfg(test)]
mod test_support;

pub use error::Error;
pub use sandbox::{Command, HOSTNAME, MAX_OPEN_FILES, Network, Sandbox};
