use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, chown, fchown, symlink};
use std::path::Path;

use laft_core::Blake3Hash;

use crate::decompress::Decompressor;
use crate::manifest::PRIVILEGE_BITS;
use crate::tar::{self, Header, Kind};
use crate::write::{MANIFEST_PATH, META_PATH};
use crate::{Entry, Error, Identity, Manifest, Meta};

/// The largest metadata member, in bytes.
const MAX_META_LEN: u64 = 1024 * 1024;
/// The largest manifest member, in bytes.
const MAX_MANIFEST_LEN: u64 = 64 * 1024 * 1024;

/// A streaming reader of package files that validates everything it reads.
///
/// [`Reader::new`] reads only the metadata and the manifest, which come
/// first, so listing a package's files needs only the beginning of the file.
/// [`Reader::unpack`] then streams the payload into a directory, checking
/// every member against the next manifest entry.
///
/// ```no_run
/// # fn main() -> Result<(), laft_pkg::Error> {
/// let file = std::fs::File::open("ripgrep-14.1.1-r1.x86_64-v3.lpkg")?;
/// let reader = laft_pkg::Reader::new(file)?;
/// for entry in reader.manifest().entries() {
///     println!("{}", entry.path());
/// }
/// # Ok(())
/// # }
/// ```
pub struct Reader<R> {
    archive: Archive<R>,
    meta: Meta,
    meta_bytes: Vec<u8>,
    manifest: Manifest,
    manifest_bytes: Vec<u8>,
}

impl<R: Read> Reader<R> {
    /// Reads and validates the metadata and manifest members of the package
    /// file `input`.
    ///
    /// The metadata must be the first member, at most 1 MiB of valid
    /// canonical JSON. The manifest must be the second, at most 64 MiB of
    /// valid canonical JSON. The manifest must name the metadata by its
    /// BLAKE3 hash, and both must agree on the identity and installed size.
    ///
    /// # Errors
    ///
    /// Fails when the stream is not a valid zstd stream or tar archive, or
    /// when either member is missing, too large or invalid.
    pub fn new(input: R) -> Result<Self, Error> {
        let mut archive = Archive {
            stream: Decompressor::new(input)?,
        };
        let meta_bytes = archive.document(META_PATH, MAX_META_LEN)?;
        let meta = Meta::from_json(&meta_bytes).map_err(|e| e.at(META_PATH))?;
        let identity = meta.identity();
        let manifest_bytes = archive
            .document(MANIFEST_PATH, MAX_MANIFEST_LEN)
            .map_err(|e| e.in_package(&identity))?;
        let manifest = Manifest::from_json(&manifest_bytes)
            .map_err(|e| e.at(MANIFEST_PATH).in_package(&identity))?;
        manifest.check_meta(&meta, &meta_bytes)?;
        Ok(Self {
            archive,
            meta,
            meta_bytes,
            manifest,
            manifest_bytes,
        })
    }

    /// The package metadata.
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// The exact bytes of the metadata member.
    pub fn meta_bytes(&self) -> &[u8] {
        &self.meta_bytes
    }

    /// The payload manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The exact bytes of the manifest member.
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }

    /// Unpacks the payload into `dir`, which must be an existing empty
    /// directory, and validates the rest of the package file.
    ///
    /// Each member must match the next manifest entry in path, type, mode,
    /// owner, group, size and symlink target, and carry no setuid or setgid
    /// bits. A hard link must point to an earlier file with the same content,
    /// mode, owner and group. File content is hashed while it is written and
    /// must match the manifest. After the last entry the archive must end
    /// with the end-of-archive marker, and only zero bytes may follow it.
    ///
    /// Files get their manifest mode. Directories are created writable by
    /// their owner and keep that mode, so that the caller can add the
    /// metadata and choose the final directory modes. Entries owned by an
    /// account other than root are changed to the registry ids, which needs
    /// the privilege to change ownership. Everything else is owned by the
    /// caller. The metadata and manifest are not written. They are available
    /// from [`Self::meta_bytes`] and [`Self::manifest_bytes`].
    ///
    /// On error, `dir` holds a partial payload, which the caller removes.
    ///
    /// # Errors
    ///
    /// Fails when `dir` is not an empty directory, when the package does not
    /// match its manifest, or when reading or writing fails. The error names
    /// the package and the path.
    pub fn unpack(mut self, dir: &Path) -> Result<(), Error> {
        let package = self.manifest.package().clone();
        let mut contents = fs::read_dir(dir).map_err(|e| Error::from(e).in_package(&package))?;
        if contents.next().is_some() {
            return Err(Error::invalid(format!(
                "cannot unpack into {}, which is not empty",
                dir.display()
            ))
            .in_package(&package));
        }
        let mut files: HashMap<&str, FileInfo> = HashMap::new();
        for entry in self.manifest.entries() {
            let path = entry.path();
            let member = self
                .archive
                .next()
                .map_err(|e| e.at(path).in_package(&package))?
                .ok_or_else(|| {
                    Error::invalid("the archive ends before this manifest entry")
                        .at(path)
                        .in_package(&package)
                })?;
            unpack_entry(&mut self.archive, dir, entry, &member, &mut files)
                .map_err(|e| e.at(path).in_package(&package))?;
        }
        self.archive.finish(&package)
    }
}

/// What a hard link must agree with.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileInfo {
    hash: Blake3Hash,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
}

fn unpack_entry<'m, R: Read>(
    archive: &mut Archive<R>,
    dir: &Path,
    entry: &'m Entry,
    member: &Header,
    files: &mut HashMap<&'m str, FileInfo>,
) -> Result<(), Error> {
    let path = entry.path();
    if member.path != path.as_bytes() {
        return Err(Error::mismatch(
            "member path",
            path,
            String::from_utf8_lossy(&member.path),
        ));
    }
    let (uid, gid) = entry.ids()?;
    if member.uid != uid {
        return Err(Error::mismatch("owner", uid, member.uid));
    }
    if member.gid != gid {
        return Err(Error::mismatch("group", gid, member.gid));
    }
    let target = dir.join(path);
    let expect_kind = |expected: &str| {
        Error::mismatch(
            "member type",
            expected,
            format!("{:?}", member.kind).to_lowercase(),
        )
    };
    match entry {
        Entry::Dir { mode, .. } => {
            if member.kind != Kind::Dir {
                return Err(expect_kind("dir"));
            }
            check_mode(*mode, member.mode)?;
            fs::create_dir(&target)?;
            if (uid, gid) != (0, 0) {
                chown(&target, Some(uid), Some(gid))?;
            }
        }
        Entry::Symlink { target: link, .. } => {
            if member.kind != Kind::Symlink {
                return Err(expect_kind("symlink"));
            }
            if member.link != link.as_bytes() {
                return Err(Error::mismatch(
                    "symlink target",
                    link,
                    String::from_utf8_lossy(&member.link),
                ));
            }
            symlink(link, &target)?;
        }
        Entry::File {
            mode, size, hash, ..
        } => {
            check_mode(*mode, member.mode)?;
            let info = FileInfo {
                hash: *hash,
                size: *size,
                mode: *mode,
                uid,
                gid,
            };
            match member.kind {
                Kind::File => {
                    if member.size != *size {
                        return Err(Error::mismatch("size", size, member.size));
                    }
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&target)?;
                    let (copied, actual) =
                        crate::copy_hashed(&mut archive.stream, &mut file, *size)?;
                    if copied < *size {
                        return Err(Error::invalid("the archive ends inside this file"));
                    }
                    if actual != *hash {
                        return Err(Error::mismatch("content hash", hash, actual));
                    }
                    archive.skip_padding(*size)?;
                    if (uid, gid) != (0, 0) {
                        fchown(&file, Some(uid), Some(gid))?;
                    }
                    file.set_permissions(fs::Permissions::from_mode(*mode))?;
                }
                Kind::HardLink => {
                    let link = std::str::from_utf8(&member.link)
                        .ok()
                        .filter(|link| files.get(link) == Some(&info))
                        .ok_or_else(|| {
                            Error::invalid(format!(
                                "the hard link to `{}` does not point to an earlier file with the same content, mode, owner and group",
                                String::from_utf8_lossy(&member.link)
                            ))
                        })?;
                    fs::hard_link(dir.join(link), &target)?;
                }
                _ => return Err(expect_kind("file")),
            }
            files.insert(path, info);
        }
    }
    Ok(())
}

fn check_mode(expected: u32, actual: u32) -> Result<(), Error> {
    if expected != actual {
        return Err(Error::mismatch(
            "mode",
            format!("{expected:#o}"),
            format!("{actual:#o}"),
        ));
    }
    Ok(())
}

/// The tar layer of a package file.
struct Archive<R> {
    stream: Decompressor<R>,
}

impl<R: Read> Archive<R> {
    fn block(&mut self) -> Result<[u8; tar::BLOCK], Error> {
        let mut block = [0; tar::BLOCK];
        self.stream.read_exact(&mut block).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                Error::invalid("the archive ends without the end-of-archive marker")
            } else {
                Error::from(e)
            }
        })?;
        Ok(block)
    }

    fn skip_padding(&mut self, size: u64) -> Result<(), Error> {
        let mut padding = [0; tar::BLOCK];
        self.stream
            .read_exact(&mut padding[..tar::padding(size)])
            .map_err(|_| Error::invalid("the archive ends inside the padding of a member"))
    }

    /// Reads the next member header, applying a pax extended header if one
    /// comes first. Returns `None` at the end-of-archive marker.
    fn next(&mut self) -> Result<Option<Header>, Error> {
        let block = self.block()?;
        if block.iter().all(|&b| b == 0) {
            if self.block()?.iter().any(|&b| b != 0) {
                return Err(Error::invalid(
                    "a zero block is not followed by a second one",
                ));
            }
            return Ok(None);
        }
        let mut header = tar::decode(&block).map_err(Error::invalid)?;
        if header.kind == Kind::PaxHeader {
            if header.size > tar::MAX_PAX_LEN {
                return Err(Error::invalid("a pax extended header is too large"));
            }
            let mut data = vec![0; header.size as usize];
            self.stream
                .read_exact(&mut data)
                .map_err(|_| Error::invalid("the archive ends inside a pax extended header"))?;
            self.skip_padding(header.size)?;
            let pax = tar::parse_pax(&data).map_err(Error::invalid)?;
            let block = self.block()?;
            header = tar::decode(&block).map_err(Error::invalid)?;
            if header.kind == Kind::PaxHeader {
                return Err(Error::invalid(
                    "a pax extended header is followed by another one",
                ));
            }
            header.path = pax.path.unwrap_or(header.path);
            header.link = pax.link.unwrap_or(header.link);
            header.size = pax.size.unwrap_or(header.size);
        }
        let path = String::from_utf8_lossy(&header.path).into_owned();
        if header.mode & PRIVILEGE_BITS != 0 {
            return Err(Error::invalid("the member has setuid or setgid bits").at(&path));
        }
        if header.kind != Kind::File && header.size != 0 {
            return Err(Error::invalid("only regular file members have content").at(&path));
        }
        Ok(Some(header))
    }

    /// Reads a metadata or manifest member.
    fn document(&mut self, path: &str, max_len: u64) -> Result<Vec<u8>, Error> {
        let header = self
            .next()?
            .ok_or_else(|| Error::invalid(format!("the archive ends before `{path}`")))?;
        if header.kind != Kind::File || header.path != path.as_bytes() {
            return Err(Error::invalid(format!(
                "the member `{}` comes where `{path}` belongs",
                String::from_utf8_lossy(&header.path)
            )));
        }
        if header.size > max_len {
            return Err(Error::invalid(format!(
                "`{path}` is larger than {max_len} bytes"
            )));
        }
        let mut bytes = vec![0; header.size as usize];
        self.stream
            .read_exact(&mut bytes)
            .map_err(|_| Error::invalid(format!("the archive ends inside `{path}`")))?;
        self.skip_padding(header.size)?;
        Ok(bytes)
    }

    /// Checks that the archive ends here and that the rest of the stream is
    /// zero bytes.
    fn finish(&mut self, package: &Identity) -> Result<(), Error> {
        if let Some(member) = self.next().map_err(|e| e.in_package(package))? {
            return Err(
                Error::invalid("the archive has a member the manifest does not list")
                    .at(&String::from_utf8_lossy(&member.path))
                    .in_package(package),
            );
        }
        let mut buf = [0; 8192];
        loop {
            let n = self
                .stream
                .read(&mut buf)
                .map_err(|e| Error::from(e).in_package(package))?;
            if n == 0 {
                return Ok(());
            }
            if buf[..n].iter().any(|&b| b != 0) {
                return Err(
                    Error::invalid("data follows the end-of-archive marker").in_package(package)
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
