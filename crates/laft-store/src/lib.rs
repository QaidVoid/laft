//! The Laft package store, views, generations, transactions and garbage
//! collection.
//!
//! Everything lives under a [`Root`], which is `/` on a running system:
//!
//! - The **store** (`laft/store`) holds one immutable entry per package,
//!   named by its content address. An entry is published with one rename and
//!   never written to afterwards.
//! - A **view** is a complete `usr` tree assembled from store entries with
//!   hard links. Declared views live in `laft/views/<name>/<version>`.
//! - A **generation** (`laft/gens/<N>`) is a numbered, immutable system
//!   state: the system view as `usr`, its configuration and `gen.json`.
//!   `laft/gens/default` names the generation the machine boots by default.
//!
//! Every change happens inside a [`Transaction`] under the root's exclusive
//! [`Lock`], and is recorded in a journal first, so that [`Lock::recover`]
//! can finish or roll back a transaction interrupted at any point, including
//! by a power loss. [`plan_gc`] decides what nothing references any more,
//! explains why, and deletes it.
//!
//! ```no_run
//! use std::time::SystemTime;
//!
//! use laft_store::{GcPolicy, Root, plan_gc};
//!
//! let root = Root::default();
//! let lock = root.lock("gc")?;
//! lock.recover()?;
//! let plan = plan_gc(&lock, &GcPolicy::default(), SystemTime::now())?;
//! print!("{plan}");
//! plan.execute(&lock)?;
//! # Ok::<(), laft_store::Error>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod fault;
mod fsx;
mod gc;
mod generation;
mod journal;
mod lock;
mod root;
mod store;
mod transaction;
mod view;

#[cfg(test)]
mod test_support;

pub use error::{Conflict, Error, Result};
pub use gc::{Decision, GcPlan, GcPolicy, Subject, plan_gc, remove_generations};
pub use generation::{
    CarriedModule, CarryOver, ConfigFile, ConfigRecord, EtcRecord, Generation, GenerationInput,
    IndexRecord, Init,
};
pub use journal::State;
pub use lock::{Lock, LockInfo};
pub use root::Root;
pub use store::{Difference, Store};
pub use transaction::{Outcome, Plan, Recovery, StagedGeneration, Transaction};
pub use view::{
    PackageInput, PackageRecord, Reason, TriggerRecord, TriggerStatus, ViewInput, ViewManifest,
};
