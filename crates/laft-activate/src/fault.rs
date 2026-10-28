//! A hook that lets tests interrupt assembly, account activation and boot
//! installation at a named point. Outside tests it does nothing.

use crate::error::Result;

/// Every point at which a test can interrupt assembly, account activation
/// or boot installation, in the order they are passed.
#[cfg(test)]
pub(crate) const POINTS: &[&str] = &[
    "accounts.databases",
    "assemble.staged",
    "assemble.etc",
    "assemble.triggers",
    "assemble.privileged",
    "assemble.boot",
    "esp.files",
    "esp.config",
];

#[cfg(test)]
thread_local! {
    static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
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

/// Makes the next calls on this thread fail at `name`, or at no point for
/// `None`.
#[cfg(test)]
pub(crate) fn fail_at(name: Option<&'static str>) {
    FAIL_AT.set(name);
}
