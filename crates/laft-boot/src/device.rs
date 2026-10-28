//! Naming boot devices and finding them as the kernel discovers them.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::probe::{PROBE_LEN, Probe, probe};

/// How a boot device is named in `laft-boot.json`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DeviceSpec {
    /// `/dev/<name>`, a kernel device name such as `nvme0n1p2`.
    Kernel(String),
    /// `/dev/mapper/<name>`, which exists only once LUKS is unlocked.
    Mapper(String),
    /// `UUID=<uuid>`, matched against the UUID in the device's superblock.
    Uuid(String),
}

impl DeviceSpec {
    /// Reads a device name.
    ///
    /// # Errors
    ///
    /// Fails for anything but `UUID=<uuid>`, `/dev/<name>` and
    /// `/dev/mapper/<name>` with a single path component as the name.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let single = |name: &str| !name.is_empty() && !name.contains('/') && name != "..";
        if let Some(uuid) = text.strip_prefix("UUID=") {
            if !uuid.is_empty() && uuid.bytes().all(|b| b.is_ascii_graphic()) {
                return Ok(Self::Uuid(uuid.to_owned()));
            }
        } else if let Some(name) = text.strip_prefix("/dev/mapper/") {
            if single(name) {
                return Ok(Self::Mapper(name.to_owned()));
            }
        } else if let Some(name) = text.strip_prefix("/dev/")
            && single(name)
        {
            return Ok(Self::Kernel(name.to_owned()));
        }
        Err(format!(
            "device {text:?} is not UUID=..., /dev/<name> or /dev/mapper/<name>"
        ))
    }
}

impl fmt::Display for DeviceSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kernel(name) => write!(f, "/dev/{name}"),
            Self::Mapper(name) => write!(f, "/dev/mapper/{name}"),
            Self::Uuid(uuid) => write!(f, "UUID={uuid}"),
        }
    }
}

/// The block devices the kernel has announced, with what each holds.
///
/// Devices are listed from a sysfs block class directory and opened in a
/// device directory, `/sys/class/block` and `/dev` on a real boot. A device
/// is probed once, the first time it is seen with a non-zero size, so that
/// polling stays cheap.
#[derive(Debug)]
pub(crate) struct Devices {
    sys_block: PathBuf,
    dev: PathBuf,
    seen: BTreeMap<String, Option<Probe>>,
}

impl Devices {
    /// The devices listed in `sys_block` with nodes in `dev`.
    pub(crate) fn new(sys_block: impl Into<PathBuf>, dev: impl Into<PathBuf>) -> Self {
        Self {
            sys_block: sys_block.into(),
            dev: dev.into(),
            seen: BTreeMap::new(),
        }
    }

    /// Probes the devices that appeared since the last refresh.
    pub(crate) fn refresh(&mut self) {
        let Ok(entries) = fs::read_dir(&self.sys_block) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if self.seen.contains_key(&name) || !has_size(&entry.path()) {
                continue;
            }
            let mut head = Vec::with_capacity(PROBE_LEN);
            let read = File::open(self.dev.join(&name))
                .and_then(|f| f.take(PROBE_LEN as u64).read_to_end(&mut head));
            if read.is_ok() {
                self.seen.insert(name, probe(&head));
            }
        }
    }

    /// The device node of `spec` if it is present now.
    pub(crate) fn find(&self, spec: &DeviceSpec) -> Option<PathBuf> {
        match spec {
            DeviceSpec::Kernel(name) => self.block_node(&self.dev.join(name)),
            DeviceSpec::Mapper(name) => self.block_node(&self.dev.join("mapper").join(name)),
            DeviceSpec::Uuid(uuid) => self.seen.iter().find_map(|(name, found)| {
                let found = found.as_ref()?;
                found
                    .uuid
                    .eq_ignore_ascii_case(uuid)
                    .then(|| self.dev.join(name))
            }),
        }
    }

    fn block_node(&self, path: &Path) -> Option<PathBuf> {
        let kind = fs::metadata(path).ok()?.file_type();
        kind.is_block_device().then(|| path.to_owned())
    }

    /// Waits until every device in `specs` is present, polling every
    /// `interval` for at most `timeout`, and returns their nodes in the
    /// order of `specs`.
    ///
    /// # Errors
    ///
    /// When the time is up, fails with the first missing device and a
    /// listing of every device seen with its type and UUID.
    pub(crate) fn wait(
        &mut self,
        specs: &[DeviceSpec],
        timeout: Duration,
        interval: Duration,
    ) -> Result<Vec<PathBuf>, String> {
        let start = Instant::now();
        loop {
            self.refresh();
            let found: Vec<_> = specs.iter().map(|s| self.find(s)).collect();
            if let Some(missing) = found.iter().position(Option::is_none) {
                if start.elapsed() >= timeout {
                    return Err(format!(
                        "timed out after {}s waiting for {}\n{}",
                        timeout.as_secs(),
                        specs[missing],
                        self.listing()
                    ));
                }
                thread::sleep(interval);
                continue;
            }
            return Ok(found.into_iter().flatten().collect());
        }
    }

    /// Every device seen, one per line, with its type and UUID when known.
    pub(crate) fn listing(&self) -> String {
        let mut out = String::from("block devices seen:");
        if self.seen.is_empty() {
            out.push_str(" none");
        }
        for (name, found) in &self.seen {
            match found {
                Some(p) => out.push_str(&format!("\n  {name:<12} {:<6} {}", p.kind, p.uuid)),
                None => out.push_str(&format!("\n  {name:<12} (no filesystem)")),
            }
        }
        out
    }
}

/// Whether the sysfs entry `entry` reports a non-zero size.
fn has_size(entry: &Path) -> bool {
    fs::read_to_string(entry.join("size"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .is_some_and(|size| size > 0)
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;
