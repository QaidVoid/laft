//! Who called this program and whether they may enter a view.

use crate::records::ViewRecord;

/// The real credentials of the caller, recorded before anything else runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Caller {
    /// The real uid.
    pub(crate) uid: u32,
    /// The real gid.
    pub(crate) gid: u32,
    /// The supplementary gids.
    pub(crate) groups: Vec<u32>,
}

/// Whether `caller` may enter the view `view` describes.
///
/// Root may enter every view. Anyone else needs their real uid in the
/// view's allowed uids, or their real gid or a supplementary gid in its
/// allowed gids. The kernel's supplementary list already reflects the group
/// memberships of the login, so the group database is not consulted.
pub(crate) fn may_enter(caller: &Caller, view: &ViewRecord) -> bool {
    caller.uid == 0
        || view.allowed_uids.contains(&caller.uid)
        || std::iter::once(&caller.gid)
            .chain(&caller.groups)
            .any(|gid| view.allowed_gids.contains(gid))
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
