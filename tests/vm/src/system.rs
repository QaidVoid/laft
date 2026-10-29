//! The two systems the test can install: the minimal one built from the
//! recipes in `tests/vm/recipes`, and the base system that the bootstrap of
//! the recipe repository built.

use std::path::{Path, PathBuf};

use crate::Paths;

/// What the installed system is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum System {
    /// busybox, runit, a prebuilt kernel and Limine, built from the recipes
    /// in `tests/vm/recipes`.
    Minimal,
    /// The base system that the bootstrap built, from the package files in
    /// this directory, such as the output directory of its build root.
    SelfBuilt(PathBuf),
}

/// The packages of the self-built system that the repository publishes:
/// those the configuration names and their runtime dependencies.
pub(crate) const SELF_BUILT_PACKAGES: &[&str] = &[
    "acl",
    "attr",
    "bash",
    "bzip2",
    "coreutils",
    "dash",
    "file",
    "gcc-libs",
    "glibc",
    "gmp",
    "grep",
    "kbd",
    "kmod",
    "libcap",
    "libxcrypt",
    "limine",
    "linux",
    "lz4",
    "ncurses",
    "openssl",
    "procps-ng",
    "readline",
    "runit",
    "sed",
    "shadow",
    "udev",
    "util-linux",
    "xz",
    "zlib",
    "zstd",
];

impl System {
    /// The configuration file in `tests/vm` that the installer uses.
    pub(crate) fn config(&self) -> &'static str {
        match self {
            Self::Minimal => "system.stav",
            Self::SelfBuilt(_) => "system-self-built.stav",
        }
    }

    /// The directory that holds the kernel package.
    pub(crate) fn kernel_dir(&self, paths: &Paths) -> PathBuf {
        match self {
            Self::Minimal => paths.build_out(),
            Self::SelfBuilt(dir) => dir.clone(),
        }
    }

    /// The release of the kernel, the directory below `usr/lib/modules`.
    pub(crate) fn release(&self) -> &'static str {
        match self {
            Self::Minimal => "7.2.13_1",
            Self::SelfBuilt(_) => "7.2.13-laft1",
        }
    }

    /// The modules the installer loads, in dependency order, below
    /// `usr/lib/modules/<release>/kernel`. The self-built kernel has the
    /// virtio disk, ext4 and FAT drivers built in.
    pub(crate) fn installer_modules(&self) -> &'static [&'static str] {
        match self {
            Self::Minimal => &[
                "drivers/virtio/virtio_ring.ko",
                "drivers/virtio/virtio.ko",
                "drivers/virtio/virtio_pci_legacy_dev.ko",
                "drivers/virtio/virtio_pci_modern_dev.ko",
                "drivers/virtio/virtio_pci.ko",
                "drivers/block/virtio_blk.ko",
                "lib/crc/crc16.ko",
                "fs/mbcache.ko",
                "fs/jbd2/jbd2.ko",
                "fs/ext4/ext4.ko",
                "fs/fat/fat.ko",
                "fs/fat/vfat.ko",
                "fs/nls/nls_cp437.ko",
                "fs/nls/nls_iso8859-1.ko",
            ],
            Self::SelfBuilt(_) => &[
                "fs/fat/vfat.ko",
                "fs/nls/nls_cp437.ko",
                "fs/nls/nls_iso8859-1.ko",
            ],
        }
    }

    /// The directory of self-built package files, if this is that system.
    pub(crate) fn self_built(&self) -> Option<&Path> {
        match self {
            Self::Minimal => None,
            Self::SelfBuilt(dir) => Some(dir),
        }
    }
}
