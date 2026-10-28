//! Choosing the kernel modules of an initramfs from a kernel's module
//! index, in the order they must be loaded.

use std::collections::{BTreeMap, BTreeSet};

/// The module directories every storage, bus and keyboard driver lives in,
/// all of which an initramfs that is not host only carries.
pub(crate) const GENERIC_DIRS: [&str; 11] = [
    "kernel/drivers/ata/",
    "kernel/drivers/nvme/",
    "kernel/drivers/scsi/",
    "kernel/drivers/mmc/",
    "kernel/drivers/md/",
    "kernel/drivers/block/",
    "kernel/drivers/virtio/",
    "kernel/drivers/usb/storage/",
    "kernel/drivers/usb/host/",
    "kernel/drivers/hid/",
    "kernel/drivers/input/keyboard/",
];

/// The module directories of the ciphers LUKS may need.
pub(crate) const CRYPTO_DIRS: [&str; 2] = ["kernel/crypto/", "kernel/arch/x86/crypto/"];

/// The modules for typing a passphrase at the console.
pub(crate) const KEYBOARD: [&str; 8] = [
    "hid_generic",
    "usbhid",
    "xhci_pci",
    "xhci_hcd",
    "ehci_pci",
    "ehci_hcd",
    "atkbd",
    "i8042",
];

/// The module index of one kernel release, read from the files `depmod`
/// writes and the kernel build ships.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModuleIndex {
    /// The path of every module relative to the release directory, by name.
    paths: BTreeMap<String, String>,
    /// The modules each module needs, by name.
    deps: BTreeMap<String, Vec<String>>,
    /// The modules built into the kernel image.
    builtin: BTreeSet<String>,
    /// The modules to load before each module, by name.
    softdeps: BTreeMap<String, Vec<String>>,
    /// The build order of each module, which breaks ties in the load order.
    order: BTreeMap<String, usize>,
}

/// The name of the module at `path`: its file name without the `.ko` and
/// compression suffixes, with `-` replaced by `_` as `modprobe` does.
pub(crate) fn module_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = [".ko.zst", ".ko.xz", ".ko.gz", ".ko"]
        .iter()
        .find_map(|suffix| file.strip_suffix(suffix))
        .unwrap_or(file);
    normalize(stem)
}

/// `name` with `-` replaced by `_`.
pub(crate) fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

impl ModuleIndex {
    /// Parses `modules.dep`, `modules.builtin`, `modules.softdep` and
    /// `modules.order`.
    ///
    /// # Errors
    ///
    /// Fails with the line when `modules.dep` has a line without a colon.
    pub(crate) fn parse(
        dep: &str,
        builtin: &str,
        softdep: &str,
        order: &str,
    ) -> Result<Self, String> {
        let mut index = Self::default();
        for line in dep.lines().filter(|l| !l.trim().is_empty()) {
            let (module, needs) = line
                .split_once(':')
                .ok_or_else(|| format!("modules.dep: malformed line {line:?}"))?;
            let name = module_name(module.trim());
            index.paths.insert(name.clone(), module.trim().to_owned());
            index
                .deps
                .insert(name, needs.split_whitespace().map(module_name).collect());
        }
        index.builtin = builtin
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(module_name)
            .collect();
        for line in softdep.lines() {
            let mut words = line.split_whitespace();
            if words.next() != Some("softdep") {
                continue;
            }
            let Some(module) = words.next() else { continue };
            let mut pre = false;
            for word in words {
                match word {
                    "pre:" => pre = true,
                    "post:" => pre = false,
                    _ if pre => index
                        .softdeps
                        .entry(normalize(module))
                        .or_default()
                        .push(normalize(word)),
                    _ => {}
                }
            }
        }
        for (position, line) in order
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .enumerate()
        {
            index.order.entry(module_name(line)).or_insert(position);
        }
        Ok(index)
    }

    /// The path of module `name` relative to the release directory, if the
    /// kernel has it as a loadable module.
    pub(crate) fn path(&self, name: &str) -> Option<&str> {
        self.paths.get(name).map(String::as_str)
    }

    /// Whether module `name` is built into the kernel image.
    pub(crate) fn is_builtin(&self, name: &str) -> bool {
        self.builtin.contains(name)
    }

    /// The loadable modules to put in an initramfs, in load order, for the
    /// requested module names `names` and every module below one of the
    /// directories `dirs`.
    ///
    /// Requested modules that are built in or missing are left out. Soft
    /// dependencies to load first and hard dependencies join the set, and
    /// every module comes after the modules it needs. Ties are broken by
    /// the kernel's build order, then by name.
    pub(crate) fn select(&self, names: &BTreeSet<String>, dirs: &[&str]) -> Vec<String> {
        let loadable =
            |name: &String| self.paths.contains_key(name) && !self.builtin.contains(name);
        let mut selected: BTreeSet<String> = names
            .iter()
            .map(|n| normalize(n))
            .filter(loadable)
            .collect();
        for (name, path) in &self.paths {
            if dirs.iter().any(|dir| path.starts_with(dir)) && loadable(name) {
                selected.insert(name.clone());
            }
        }
        let mut pending: Vec<String> = selected.iter().cloned().collect();
        while let Some(name) = pending.pop() {
            for needed in self.prerequisites(&name) {
                if loadable(needed) && selected.insert(needed.clone()) {
                    pending.push(needed.clone());
                }
            }
        }

        let rank = |name: &String| {
            (
                self.order.get(name).copied().unwrap_or(usize::MAX),
                name.clone(),
            )
        };
        let mut waiting: BTreeMap<&String, BTreeSet<&String>> = selected
            .iter()
            .map(|name| {
                let needs = self
                    .prerequisites(name)
                    .filter(|n| selected.contains(*n) && *n != name)
                    .collect();
                (name, needs)
            })
            .collect();
        let mut ordered = Vec::with_capacity(selected.len());
        while !waiting.is_empty() {
            let ready = waiting
                .iter()
                .filter(|(_, needs)| needs.is_empty())
                .map(|(name, _)| *name)
                .min_by_key(|name| rank(name));
            // A dependency cycle cannot load in any order. The modules left
            // are appended in rank order, and loading reports the failure.
            let Some(next) =
                ready.or_else(|| waiting.keys().copied().min_by_key(|name| rank(name)))
            else {
                break;
            };
            waiting.remove(next);
            for needs in waiting.values_mut() {
                needs.remove(next);
            }
            ordered.push(next.clone());
        }
        ordered
    }

    /// The modules `name` needs loaded first: its soft dependencies to load
    /// before it and its hard dependencies.
    fn prerequisites<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a String> + 'a {
        let soft = self.softdeps.get(name).into_iter().flatten();
        let hard = self.deps.get(name).into_iter().flatten();
        soft.chain(hard)
    }
}

/// The values of `key=` in the `.modinfo` section `modinfo`, which holds
/// NUL separated `key=value` strings.
pub(crate) fn modinfo_values<'a>(modinfo: &'a [u8], key: &str) -> Vec<&'a str> {
    modinfo
        .split(|&b| b == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .filter_map(|entry| entry.strip_prefix(key)?.strip_prefix('='))
        .collect()
}

#[cfg(test)]
#[path = "modules_tests.rs"]
mod tests;
