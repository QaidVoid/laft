//! Identifiers and formats shared by every Laft component.
//!
//! Package names, versions, variants, package references, dependencies,
//! hashes, store identifiers, timestamps, the static system accounts,
//! canonical JSON, file capabilities, the trigger table, device names, the
//! LUKS options of the boot and the boot counting state live here, so that
//! every tool parses and prints them the same way.

#![forbid(unsafe_code)]

mod account;
mod base32;
mod boot_state;
mod caps;
mod cjson;
mod dep;
mod device;
mod error;
mod hash;
mod luks;
mod name;
mod pkgref;
mod store;
mod text_serde;
mod time;
mod trigger;
mod variant;
mod version;

pub use account::{system_gid, system_uid};
pub use boot_state::{
    BOOT_STATE_VERSION, BootEvent, BootEventKind, BootState, BootStateError, BootTrial,
    MAX_BOOT_EVENTS,
};
pub use caps::FileCapabilities;
pub use cjson::{CanonicalJsonError, to_canonical_json};
pub use dep::{Constraint, Constraints, Dep, Op, PcName, Provide, Soname, VersionReq};
pub use device::DeviceSpec;
pub use error::ParseError;
pub use hash::{Blake3Hash, Sha256Hash};
pub use luks::LuksOption;
pub use name::Name;
pub use pkgref::PkgRef;
pub use store::{STORE_DIR, StoreId};
pub use time::Timestamp;
pub use trigger::{Instances, TRIGGER_TABLE_VERSION, TRIGGERS, Trigger, glob_matches, trigger};
pub use variant::Variant;
pub use version::{PackageVersion, Version, VersionPrefix};

/// Items that exported macros refer to. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use serde;
}
