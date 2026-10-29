use std::fmt;
use std::str::FromStr;

use crate::ParseError;

/// How a block device is named, in one of the forms of the first field of
/// an fstab entry.
///
/// The system configuration and the initramfs parse device names with this
/// type, so the configuration accepts exactly what the initramfs reads. The
/// initramfs resolves only some forms, which [`DeviceSpec::resolved_at_boot`]
/// tells.
///
/// ```
/// use laft_core::DeviceSpec;
///
/// let spec: DeviceSpec = "/dev/mapper/cryptroot".parse()?;
/// assert_eq!(spec.mapper_name(), Some("cryptroot"));
/// assert!(spec.resolved_at_boot());
/// let label: DeviceSpec = "PARTLABEL=swap".parse()?;
/// assert!(!label.resolved_at_boot());
/// assert!("swap".parse::<DeviceSpec>().is_err());
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceSpec {
    /// An absolute, normalized path, such as `/dev/nvme0n1p2` or
    /// `/dev/disk/by-id/nvme-x`.
    Path(String),
    /// `UUID=<uuid>`: the UUID in the filesystem's superblock.
    Uuid(String),
    /// `LABEL=<label>`: the label of the filesystem.
    Label(String),
    /// `PARTUUID=<uuid>`: the UUID of the partition.
    PartUuid(String),
    /// `PARTLABEL=<label>`: the label of the partition.
    PartLabel(String),
}

impl DeviceSpec {
    /// The kernel device name of a path `/dev/<name>` with a single
    /// component below `/dev/`, such as `nvme0n1p2`.
    pub fn kernel_name(&self) -> Option<&str> {
        let Self::Path(path) = self else {
            return None;
        };
        path.strip_prefix("/dev/")
            .filter(|name| !name.contains('/'))
    }

    /// The device-mapper name of a path `/dev/mapper/<name>`, which exists
    /// only once the device is set up, for example by unlocking LUKS.
    pub fn mapper_name(&self) -> Option<&str> {
        let Self::Path(path) = self else {
            return None;
        };
        path.strip_prefix("/dev/mapper/")
            .filter(|name| !name.contains('/'))
    }

    /// Whether the initramfs can find the device: by `UUID=`, by a kernel
    /// device name or by a device-mapper name.
    pub fn resolved_at_boot(&self) -> bool {
        matches!(self, Self::Uuid(_))
            || self.kernel_name().is_some()
            || self.mapper_name().is_some()
    }
}

impl FromStr for DeviceSpec {
    type Err = ParseError;

    /// Reads a device name. A path is absolute and normalized, without
    /// empty, `.` or `..` components. A tag value is not empty, and a UUID
    /// is printable ASCII. Neither holds whitespace or control characters,
    /// which an fstab entry cannot carry.
    fn from_str(text: &str) -> Result<Self, ParseError> {
        let error = |reason| ParseError::new("device", text, reason);
        if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(error(
                "a device name holds no whitespace or control characters",
            ));
        }
        if let Some((tag, value)) = text.split_once('=')
            && matches!(tag, "UUID" | "LABEL" | "PARTUUID" | "PARTLABEL")
        {
            if value.is_empty() {
                return Err(error("the tag has no value"));
            }
            if tag.ends_with("UUID") && !value.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(error("a UUID is printable ASCII"));
            }
            let value = value.to_owned();
            return Ok(match tag {
                "UUID" => Self::Uuid(value),
                "LABEL" => Self::Label(value),
                "PARTUUID" => Self::PartUuid(value),
                _ => Self::PartLabel(value),
            });
        }
        let Some(rest) = text.strip_prefix('/') else {
            return Err(error(
                "expected an absolute path, UUID=, LABEL=, PARTUUID= or PARTLABEL=",
            ));
        };
        if rest.split('/').any(|part| matches!(part, "" | "." | "..")) {
            return Err(error("a device path has no empty, `.` or `..` components"));
        }
        Ok(Self::Path(text.to_owned()))
    }
}

impl fmt::Display for DeviceSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => f.write_str(path),
            Self::Uuid(uuid) => write!(f, "UUID={uuid}"),
            Self::Label(label) => write!(f, "LABEL={label}"),
            Self::PartUuid(uuid) => write!(f, "PARTUUID={uuid}"),
            Self::PartLabel(label) => write!(f, "PARTLABEL={label}"),
        }
    }
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;
