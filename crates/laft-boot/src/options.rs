//! Mount option strings turned into `mount(2)` flags and data.

use rustix::mount::MountFlags;

/// Splits the comma joined `options` into the flags `mount(2)` takes and the
/// data string for the filesystem.
///
/// Generic options such as `noatime` or `nosuid` become flags. `rw` and
/// `defaults` change nothing. Every other option is passed on to the
/// filesystem in the data string, in its original order.
pub(crate) fn parse(options: &str) -> (MountFlags, String) {
    let mut flags = MountFlags::empty();
    let mut data = Vec::new();
    for option in options.split(',').filter(|o| !o.is_empty()) {
        flags |= match option {
            "ro" => MountFlags::RDONLY,
            "rw" | "defaults" => MountFlags::empty(),
            "nosuid" => MountFlags::NOSUID,
            "nodev" => MountFlags::NODEV,
            "noexec" => MountFlags::NOEXEC,
            "sync" => MountFlags::SYNCHRONOUS,
            "dirsync" => MountFlags::DIRSYNC,
            "noatime" => MountFlags::NOATIME,
            "nodiratime" => MountFlags::NODIRATIME,
            "relatime" => MountFlags::RELATIME,
            "strictatime" => MountFlags::STRICTATIME,
            "lazytime" => MountFlags::LAZYTIME,
            other => {
                data.push(other);
                MountFlags::empty()
            }
        };
    }
    (flags, data.join(","))
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
