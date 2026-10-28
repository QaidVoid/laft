//! Namespaces and mounts for Laft.
//!
//! Three kinds of processes are isolated with this crate:
//!
//! - The builder runs every build phase unprivileged in a [`Sandbox`]: new
//!   user, mount, PID, IPC, UTS and network namespaces, a root that holds
//!   only the build view as `/usr` plus fixed writable `/build` and `/out`
//!   directories, no capabilities and `no_new_privs`. The phase that fetches
//!   sources keeps the host network with [`Network::Host`], which also binds
//!   the host's `/etc/resolv.conf`, `/etc/hosts` and `/etc/nsswitch.conf`
//!   read-only into an otherwise empty `/etc`, so that names resolve. An
//!   isolated sandbox has no `/etc` at all.
//! - Assembly runs every fixed program, such as a trigger that computes a
//!   cache from the tree being assembled, in a [`FixedSandbox`]: the tree
//!   as `/usr` with only named directories writable, a given configuration
//!   as `/etc`, and the program running as [`NOBODY`].
//! - The setuid helper that enters a non-system view calls [`enter_view`],
//!   which replaces the system generation's `usr` with the view in a private
//!   mount namespace and binds the system's privileged files on top. Before
//!   that it neutralizes the state its caller left behind with
//!   [`close_inherited_on_exec`] and [`reset_signal_dispositions`], and
//!   after dropping privileges it checks them with [`user_ids`] and
//!   [`group_ids`].
//!
//! Every mount uses the descriptor based mount API (`open_tree`,
//! `move_mount`, `fsmount` and `mount_setattr`), so Linux 5.12 or newer is
//! required.
//!
//! This crate may use `unsafe` code. It is confined to forking, `unshare`,
//! `mount_setattr`, `close_range`, signal resets, `kill`, `_exit`, reading
//! the real, effective and saved ids, and the hook that sets up a sandbox between `fork` and `exec`, each with the
//! reason it is sound next to it.

mod enter;
mod error;
mod fixed;
mod process;
mod sandbox;
mod sys;

#[cfg(test)]
mod test_support;

pub use enter::enter_view;
pub use error::Error;
pub use fixed::{FixedSandbox, NOBODY};
pub use process::{close_inherited_on_exec, group_ids, reset_signal_dispositions, user_ids};
pub use sandbox::{Command, HOSTNAME, MAX_OPEN_FILES, Network, Sandbox};
