//! File operations of the boot: crash safe replacement of state files and
//! links, the check of the new root's layout, and emptying the initramfs.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, symlink};
use std::path::Path;

/// Replaces `path` with `bytes` so that a crash leaves either the old or
/// the new content: a temporary file next to it is written, synced and
/// renamed into place, then the directory is synced.
///
/// # Errors
///
/// Fails when any of those steps fails.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let mut tmp_name = path.file_name().unwrap_or_default().to_owned();
    tmp_name.push(".laft-boot.tmp");
    let tmp = dir.join(tmp_name);
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    File::open(dir)?.sync_all()
}

/// Points the symlink `link` at `target` with a single rename of a new link
/// created at `tmp`, which must be on the same filesystem, then syncs the
/// directory of `link`.
///
/// # Errors
///
/// Fails when the link cannot be created, renamed or synced.
pub(crate) fn replace_symlink(link: &Path, target: &str, tmp: &Path) -> io::Result<()> {
    let _ = fs::remove_file(tmp);
    symlink(target, tmp)?;
    fs::rename(tmp, link)?;
    File::open(link.parent().unwrap_or(Path::new("/")))?.sync_all()
}

/// The top-level symlinks every Laft root has, with their targets.
pub(crate) const TOP_LEVEL_LINKS: [(&str, &str); 5] = [
    ("usr", "/run/laft/system/usr"),
    ("bin", "usr/bin"),
    ("sbin", "usr/bin"),
    ("lib", "usr/lib"),
    ("lib64", "usr/lib"),
];

/// Checks that the root at `root` has the top-level symlinks that make
/// `/usr` follow the system generation.
///
/// # Errors
///
/// Fails with every link that is missing or points elsewhere.
pub(crate) fn check_layout(root: &Path) -> Result<(), String> {
    let wrong: Vec<_> = TOP_LEVEL_LINKS
        .iter()
        .filter(|(name, target)| {
            fs::read_link(root.join(name)).map_or(true, |t| t != Path::new(target))
        })
        .map(|(name, target)| format!("/{name} -> {target}"))
        .collect();
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the root filesystem is missing Laft's top-level symlinks ({}); see \"Rescue\" in \
             the Laft documentation",
            wrong.join(", ")
        ))
    }
}

/// Deletes everything below `root` that lies on the same filesystem as
/// `root`, except `keep` and what is below it. Directories are removed
/// bottom up. Entries that cannot be removed are skipped.
///
/// This frees the memory the initramfs occupies before the switch to the
/// new root.
pub(crate) fn clear(root: &Path, keep: &Path) {
    if let Ok(meta) = fs::symlink_metadata(root) {
        clear_dir(root, keep, meta.dev());
    }
}

fn clear_dir(dir: &Path, keep: &Path, dev: u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if path == keep || meta.dev() != dev {
            continue;
        }
        if meta.is_dir() {
            clear_dir(&path, keep, dev);
            let _ = fs::remove_dir(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
