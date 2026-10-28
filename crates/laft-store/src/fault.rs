//! Hooks that let tests interrupt a transaction at a named point and lower
//! the hard link limit. Outside tests they do nothing.

use crate::error::Result;

/// Every point at which a test can interrupt a transaction, in the order a
/// transaction passes them.
#[cfg(test)]
pub(crate) const POINTS: &[&str] = &[
    "begin.counters",
    "begin.journal",
    "add.storing",
    "add.staged",
    "add.recorded",
    "add.published",
    "store.synced",
    "store.stored",
    "assemble.started",
    "view.staged",
    "view.published",
    "gen.staged",
    "gen.published",
    "assembled",
    "default.linked",
    "complete",
];

#[cfg(test)]
thread_local! {
    static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static LINK_MAX: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Fails with [`crate::Error::Injected`] when a test asked to interrupt at
/// `name`.
#[cfg(test)]
pub(crate) fn point(name: &'static str) -> Result<()> {
    assert!(POINTS.contains(&name), "fault point {name} is not listed");
    if FAIL_AT.get() == Some(name) {
        return Err(crate::Error::Injected(name));
    }
    Ok(())
}

/// Does nothing outside tests.
#[cfg(not(test))]
pub(crate) fn point(_name: &'static str) -> Result<()> {
    Ok(())
}

/// Makes the next transactions on this thread fail at `name`, or at no
/// point for `None`.
#[cfg(test)]
pub(crate) fn fail_at(name: Option<&'static str>) {
    FAIL_AT.set(name);
}

/// Replaces the filesystem's hard link limit on this thread.
#[cfg(test)]
pub(crate) fn set_link_max(limit: Option<u64>) {
    LINK_MAX.set(limit);
}

/// The hard link limit a test set, if any.
#[cfg(test)]
pub(crate) fn link_max() -> Option<u64> {
    LINK_MAX.get()
}

/// No test overrides the hard link limit outside tests.
#[cfg(not(test))]
pub(crate) fn link_max() -> Option<u64> {
    None
}
