//! The Laft service model and its runit backend.
//!
//! Services are declared once, in the system configuration, as [`Service`]
//! values. [`ServiceSet::new`] validates them, adds the generated gettys and
//! the implicit dependency on the early boot graph, and resolves every
//! enabled service into a [`Spec`]: the complete description of how to run
//! it. [`runit::render`] turns a service set into the files a generation
//! needs to boot and supervise it with runit.

#![forbid(unsafe_code)]

mod error;
mod graph;
mod model;
pub mod reconcile;
pub mod runit;
mod spec;
mod standard;
mod validate;

#[cfg(test)]
mod test_support;

pub use error::Error;
pub use model::{
    Dir, Getty, Init, Limits, Log, LogRotation, OnChange, Ready, Restart, Service, ServiceConfig,
    ServiceKind, Signal,
};
pub use spec::{SPEC_VERSION, Spec, SpecDir, SpecLimits, SpecLog, SpecReady};
pub use standard::{
    ACTIVATE, BOOT_COMPLETE, EARLY, GETTY_PREFIX, TURNSTILED, getty_name, getty_services,
    standard_services,
};
pub use validate::{ConfigError, ServiceSet, Warning};
