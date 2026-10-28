//! `laft-boot.json`, the boot configuration stored in the initramfs.
//!
//! It holds nothing specific to one generation, so generations with the same
//! boot configuration share one initramfs.

use laft_core::LuksOption;
use serde::Deserialize;

use crate::device::DeviceSpec;

/// The only version of `laft-boot.json` this program reads.
const VERSION: u32 = 1;

/// The boot configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    /// The format version, 1.
    pub(crate) version: u32,
    /// The EFI system partition, mounted only to fall back to another
    /// kernel.
    pub(crate) esp: Filesystem,
    /// A separate filesystem for `/laft`, if it is not on the root.
    pub(crate) laft: Option<Filesystem>,
    /// The LUKS devices to unlock, in order.
    pub(crate) luks: Vec<Luks>,
    /// The paths of the kernel modules to load, dependencies first.
    pub(crate) modules: Vec<String>,
    /// The kernel release the modules belong to.
    pub(crate) release: String,
    /// The root filesystem.
    pub(crate) root: Filesystem,
    /// The seconds to wait for boot devices.
    pub(crate) root_wait_secs: u64,
    /// Whether the initramfs has the emergency shell.
    pub(crate) shell: bool,
    /// The boot attempts before falling back. 0 disables boot counting.
    pub(crate) tries: u32,
}

/// A filesystem to mount.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Filesystem {
    /// The device, `UUID=...`, `/dev/<name>` or `/dev/mapper/<name>`.
    pub(crate) device: String,
    /// The path of the fsck program in the initramfs, if the filesystem is
    /// checked.
    #[serde(default)]
    pub(crate) fsck: Option<String>,
    /// The comma joined mount options.
    pub(crate) options: String,
    /// The filesystem type, for example `ext4`.
    #[serde(rename = "type")]
    pub(crate) fstype: String,
}

/// A LUKS device to unlock.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Luks {
    /// The encrypted device.
    pub(crate) device: String,
    /// The device-mapper name, so the unlocked device is
    /// `/dev/mapper/<name>`.
    pub(crate) name: String,
    /// The options for the unlocked device.
    pub(crate) options: Vec<LuksOption>,
}

#[derive(Deserialize)]
struct Versioned {
    version: u32,
}

impl Config {
    /// The devices to wait for before unlocking: every LUKS device, in the
    /// order of [`Self::luks`], then the root and `/laft` filesystems unless
    /// they are unlocked devices themselves. The first `luks.len()` specs
    /// are those of the LUKS devices.
    ///
    /// # Errors
    ///
    /// Fails when a device name is malformed.
    pub(crate) fn unlock_specs(&self) -> Result<Vec<DeviceSpec>, String> {
        let mut specs = Vec::new();
        for luks in &self.luks {
            specs.push(DeviceSpec::parse(&luks.device)?);
        }
        for fs in [Some(&self.root), self.laft.as_ref()].into_iter().flatten() {
            match DeviceSpec::parse(&fs.device)? {
                DeviceSpec::Mapper(_) => {}
                spec => specs.push(spec),
            }
        }
        Ok(specs)
    }

    /// Reads `laft-boot.json` from `bytes` for the running kernel `release`.
    ///
    /// # Errors
    ///
    /// Fails when the document is not a version 1 configuration, when it was
    /// made for another kernel release, or when a device or LUKS name is
    /// malformed.
    pub(crate) fn parse(bytes: &[u8], release: &str) -> Result<Self, String> {
        let versioned: Versioned = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if versioned.version != VERSION {
            return Err(format!("unsupported version {}", versioned.version));
        }
        let config: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if config.release != release {
            return Err(format!(
                "it is for kernel {} but kernel {release} is running",
                config.release
            ));
        }
        for luks in &config.luks {
            DeviceSpec::parse(&luks.device)?;
            if DeviceSpec::parse(&format!("/dev/mapper/{}", luks.name)).is_err() {
                return Err(format!("invalid LUKS name {:?}", luks.name));
            }
        }
        for fs in [Some(&config.root), config.laft.as_ref(), Some(&config.esp)]
            .into_iter()
            .flatten()
        {
            DeviceSpec::parse(&fs.device)?;
        }
        Ok(config)
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
