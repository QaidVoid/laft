//! A writer for uncompressed cpio archives in the `newc` format, which
//! the kernel unpacks as an initramfs.

/// What an archive entry is.
pub(crate) enum Kind {
    /// A directory.
    Dir,
    /// A regular file with this content.
    File(Vec<u8>),
    /// A symbolic link to this target.
    Symlink(String),
    /// A character device with this major and minor number.
    CharDevice(u32, u32),
}

/// An archive under construction. Entries keep the order they were added
/// in, so a directory must be added before what it contains.
#[derive(Default)]
pub(crate) struct Archive {
    bytes: Vec<u8>,
    next_inode: u32,
}

impl Archive {
    /// Adds `path`, relative to the archive root, owned by root with the
    /// permission bits `mode`.
    ///
    /// # Panics
    ///
    /// Panics if a file is 4 GiB or larger, which the newc format cannot
    /// record and no initramfs member reaches.
    pub(crate) fn add(&mut self, path: &str, mode: u32, kind: Kind) {
        let (format, data, rdev): (u32, &[u8], (u32, u32)) = match &kind {
            Kind::Dir => (0o040_000, &[], (0, 0)),
            Kind::File(bytes) => (0o100_000, bytes, (0, 0)),
            Kind::Symlink(target) => (0o120_000, target.as_bytes(), (0, 0)),
            Kind::CharDevice(major, minor) => (0o020_000, &[], (*major, *minor)),
        };
        self.next_inode += 1;
        let fields = [
            self.next_inode,
            format | mode,
            0,
            0,
            1,
            0,
            u32::try_from(data.len()).expect("an initramfs member is smaller than 4 GiB"),
            0,
            0,
            rdev.0,
            rdev.1,
            u32::try_from(path.len() + 1).expect("a member name is short"),
            0,
        ];
        self.bytes.extend_from_slice(b"070701");
        for field in fields {
            self.bytes
                .extend_from_slice(format!("{field:08x}").as_bytes());
        }
        self.bytes.extend_from_slice(path.as_bytes());
        self.bytes.push(0);
        self.pad();
        self.bytes.extend_from_slice(data);
        self.pad();
    }

    /// The archive with its trailer.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        self.add("TRAILER!!!", 0, Kind::File(Vec::new()));
        self.bytes
    }

    fn pad(&mut self) {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
    }
}
