//! Dropping from root back to the caller for good.
//!
//! The supplementary groups go first, because changing them needs
//! `CAP_SETGID`. The gid goes next, because changing it after the uid would
//! no longer be allowed. The uid goes last, and with it every capability.
//! Real, effective and saved ids are all set, so nothing is left to switch
//! back to. Afterwards the result is read back from the kernel, and any
//! difference from the caller's ids, or any way back to root, aborts.
//!
//! `no_new_privs` is deliberately not set, because the privileged programs
//! bound into the view, such as `doas`, must keep working there.

use rustix::io::Errno;
use rustix::process::{Gid, Uid};

use crate::Failure;
use crate::access::Caller;

/// One credential change of the drop.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// Set the supplementary groups.
    Groups(Vec<u32>),
    /// Set the real, effective and saved gid.
    Gid(u32),
    /// Set the real, effective and saved uid.
    Uid(u32),
}

/// The credential changes that return to `caller`, in the order they must
/// be made.
pub(crate) fn plan(caller: &Caller) -> [Change; 3] {
    [
        Change::Groups(caller.groups.clone()),
        Change::Gid(caller.gid),
        Change::Uid(caller.uid),
    ]
}

/// The credentials read back from the kernel after the drop.
#[derive(Debug, Clone)]
pub(crate) struct Observed {
    /// The real, effective and saved uid.
    pub(crate) uids: [u32; 3],
    /// The real, effective and saved gid.
    pub(crate) gids: [u32; 3],
    /// The supplementary gids.
    pub(crate) groups: Vec<u32>,
    /// What `setuid(0)` returned.
    pub(crate) setuid_root: Result<(), Errno>,
    /// What `setgid(0)` returned.
    pub(crate) setgid_root: Result<(), Errno>,
    /// Whether the effective and permitted capability sets are empty.
    pub(crate) no_capabilities: bool,
}

/// Whether `observed` shows exactly the credentials of `caller` with no way
/// back to root.
///
/// Every id slot must hold the caller's id and the supplementary groups
/// must be the caller's. A caller other than root must also be refused
/// `setuid(0)` with `EPERM` and hold no capabilities, and, unless its gid is
/// 0, be refused `setgid(0)` with `EPERM` too.
pub(crate) fn verify(caller: &Caller, observed: &Observed) -> bool {
    let mut expected = caller.groups.clone();
    expected.sort_unstable();
    expected.dedup();
    let mut groups = observed.groups.clone();
    groups.sort_unstable();
    groups.dedup();

    let ids =
        observed.uids == [caller.uid; 3] && observed.gids == [caller.gid; 3] && groups == expected;
    if caller.uid == 0 {
        return ids;
    }
    ids && observed.setuid_root == Err(Errno::PERM)
        && (caller.gid == 0 || observed.setgid_root == Err(Errno::PERM))
        && observed.no_capabilities
}

/// Drops to `caller` and verifies the result.
///
/// The process must be single-threaded, because the changes apply to the
/// calling thread only.
///
/// # Errors
///
/// Fails when a change is refused or the verification does not pass. The
/// caller must then exit without running anything.
pub(crate) fn drop_to(caller: &Caller) -> Result<(), Failure> {
    let failed = || Failure::error("privilege drop failed");
    for change in plan(caller) {
        let result = match change {
            Change::Groups(groups) => {
                let groups: Vec<Gid> = groups.into_iter().map(Gid::from_raw).collect();
                rustix::thread::set_thread_groups(&groups)
            }
            Change::Gid(gid) => {
                let gid = Gid::from_raw(gid);
                rustix::thread::set_thread_res_gid(gid, gid, gid)
            }
            Change::Uid(uid) => {
                let uid = Uid::from_raw(uid);
                rustix::thread::set_thread_res_uid(uid, uid, uid)
            }
        };
        result.map_err(|_| failed())?;
    }

    let capabilities = rustix::thread::capabilities(None).map_err(|_| failed())?;
    let observed = Observed {
        uids: laft_sandbox::user_ids().map_err(|_| failed())?,
        gids: laft_sandbox::group_ids().map_err(|_| failed())?,
        groups: rustix::process::getgroups()
            .map_err(|_| failed())?
            .into_iter()
            .map(Gid::as_raw)
            .collect(),
        setuid_root: probe(caller.uid, || rustix::thread::set_thread_uid(Uid::ROOT)),
        setgid_root: probe(caller.uid, || rustix::thread::set_thread_gid(Gid::ROOT)),
        no_capabilities: capabilities.effective.is_empty() && capabilities.permitted.is_empty(),
    };
    if verify(caller, &observed) {
        Ok(())
    } else {
        Err(failed())
    }
}

/// Tries to regain root with `attempt`, unless the caller is root anyway.
fn probe(uid: u32, attempt: impl FnOnce() -> rustix::io::Result<()>) -> Result<(), Errno> {
    if uid == 0 { Ok(()) } else { attempt() }
}

#[cfg(test)]
#[path = "privileges_tests.rs"]
mod tests;
