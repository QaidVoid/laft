//! Naming boot devices and finding them as the kernel discovers them.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use laft_core::{DeviceSpec, ParseError};

use crate::probe::{PROBE_LEN, Probe, probe};

/// Reads a boot device name from `laft-boot.json`.
///
/// # Errors
///
/// Fails for a malformed name and for a form the initramfs cannot resolve:
/// anything but `UUID=<uuid>`, `/dev/<name>` and `/dev/mapper/<name>`.
pub(crate) fn boot_spec(text: &str) -> Result<DeviceSpec, String> {
    let spec: DeviceSpec = text.parse().map_err(|e: ParseError| e.to_string())?;
    if !spec.resolved_at_boot() {
        return Err(format!(
            "device {text:?} is not UUID=..., /dev/<name> or /dev/mapper/<name>"
        ));
    }
    Ok(spec)
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

    /// The device node of `spec` if it is present now. Only the forms of
    /// [`boot_spec`] are ever found.
    pub(crate) fn find(&self, spec: &DeviceSpec) -> Option<PathBuf> {
        if let Some(name) = spec.mapper_name() {
            return self.block_node(&self.dev.join("mapper").join(name));
        }
        if let Some(name) = spec.kernel_name() {
            return self.block_node(&self.dev.join(name));
        }
        let DeviceSpec::Uuid(uuid) = spec else {
            return None;
        };
        self.seen.iter().find_map(|(name, found)| {
            let found = found.as_ref()?;
            found
                .uuid
                .eq_ignore_ascii_case(uuid)
                .then(|| self.dev.join(name))
        })
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
