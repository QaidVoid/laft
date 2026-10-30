//! The installer: an initramfs that boots the test kernel directly, mounts
//! the empty filesystems of the disk image at `/mnt` and `/mnt/boot`, and
//! runs `laft hardware --write` and `laft install` against them, as an
//! administrator installs from a rescue system.
//!
//! The install runs in a virtual machine because `laft install` needs the
//! EFI system partition mounted as FAT, which an unprivileged user on the
//! host cannot mount.

use std::fs;
use std::path::{Path, PathBuf};

use laft_pkg::Reader;

use crate::cpio::{Archive, Kind};
use crate::packages::package_file;
use crate::system::System;
use crate::{Paths, Result, fail};

/// The line the installer prints when it is done, followed by the exit
/// status of `laft install`.
pub(crate) const DONE: &str = "laft-vmtest: install finished with status ";

/// The installer's `/init`.
const INIT: &str = r#"#!/bin/busybox sh
/bin/busybox --install -s /bin
export PATH=/bin:/usr/bin HOME=/root NO_COLOR=1
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mount -t tmpfs tmpfs /tmp
mount -t tmpfs tmpfs /run
for module in $(cat /modules); do
  insmod "/lib/modules/$module" || echo "laft-vmtest: cannot load $module"
done
tries=0
while [ ! -b /dev/vda2 ] && [ "$tries" -lt 100 ]; do
  sleep 0.1
  tries=$((tries + 1))
done
mkdir -p /dev/disk/by-uuid
for device in /dev/vda1 /dev/vda2; do
  uuid=$(blkid "$device" | sed -n 's/.* UUID="\([^"]*\)".*/\1/p')
  [ -n "$uuid" ] && ln -s "$device" "/dev/disk/by-uuid/$uuid"
done
run_install() {
  mkdir -p /mnt &&
  mount -t ext4 /dev/vda2 /mnt &&
  mkdir -p /mnt/boot &&
  mount -t vfat /dev/vda1 /mnt/boot &&
  mkdir -p /mnt/etc/laft &&
  cp /system.stav /mnt/etc/laft/system.stav &&
  laft --root /mnt hardware --write &&
  cat /mnt/etc/laft/hardware.stav &&
  laft --root /mnt --yes install
}
run_install
status=$?
umount /mnt/boot
umount /mnt
sync
echo "laft-vmtest: install finished with status $status"
poweroff -f
"#;

/// Writes the installer's kernel and initramfs into the work directory and
/// returns their paths. The initramfs carries busybox and the modules from
/// the packages, `laft`, the published repository at `/repo` and the
/// configuration of `system` with `key` as the repository key. The kernel
/// is the one the installed system boots. Only the quoted value
/// `"@REPO_KEY@"` is replaced, so the comments of the configuration stay
/// as they are.
///
/// # Errors
///
/// Fails when a package cannot be read or a file cannot be written.
pub(crate) fn prepare(paths: &Paths, key: &str, system: &System) -> Result<(PathBuf, PathBuf)> {
    let unpacked = paths.work.join("unpacked");
    let _ = fs::remove_dir_all(&unpacked);
    let busybox = unpack(&paths.build_out(), "busybox", &unpacked.join("busybox"))?;
    let linux = unpack(&system.kernel_dir(paths), "linux", &unpacked.join("linux"))?;
    let release = linux.join("usr/lib/modules").join(system.release());

    let mut archive = Archive::default();
    for dir in [
        "bin",
        "dev",
        "etc",
        "lib",
        "lib/modules",
        "mnt",
        "proc",
        "root",
        "run",
        "sys",
        "tmp",
        "usr",
        "usr/bin",
    ] {
        archive.add(dir, 0o755, Kind::Dir);
    }
    archive.add("dev/console", 0o600, Kind::CharDevice(5, 1));
    archive.add("init", 0o755, Kind::File(INIT.as_bytes().to_vec()));
    archive.add(
        "bin/busybox",
        0o755,
        Kind::File(read(&busybox.join("usr/bin/busybox"))?),
    );
    archive.add(
        "usr/bin/laft",
        0o755,
        Kind::File(read(&paths.bin.join("laft"))?),
    );
    let mut list = String::new();
    for module in system.installer_modules() {
        let name = module.rsplit('/').next().unwrap_or(module);
        archive.add(
            &format!("lib/modules/{name}"),
            0o644,
            Kind::File(read(&release.join("kernel").join(module))?),
        );
        list.push_str(name);
        list.push('\n');
    }
    archive.add("modules", 0o644, Kind::File(list.into_bytes()));
    let config = fs::read_to_string(paths.source.join(system.config()))
        .map_err(|e| fail(format!("{}: {e}", system.config())))?
        .replace("\"@REPO_KEY@\"", &format!("\"{key}\""));
    archive.add("system.stav", 0o644, Kind::File(config.into_bytes()));
    add_tree(&mut archive, &paths.www(), "repo")?;

    let initrd = paths.work.join("installer.cpio");
    fs::write(&initrd, archive.finish()).map_err(|e| fail(format!("{}: {e}", initrd.display())))?;
    let kernel = paths.work.join("vmlinuz");
    let _ = fs::remove_file(&kernel);
    fs::copy(release.join("vmlinuz"), &kernel)
        .map_err(|e| fail(format!("{}: {e}", kernel.display())))?;
    fs::remove_dir_all(&unpacked).map_err(|e| fail(format!("{}: {e}", unpacked.display())))?;
    Ok((kernel, initrd))
}

/// Unpacks the one package named `name` of `dir` into `to`, with the
/// reader the store uses, and returns `to`.
fn unpack(dir: &Path, name: &str, to: &Path) -> Result<PathBuf> {
    let file = package_file(dir, name)?;
    let reader = fs::File::open(&file)
        .map_err(|e| e.to_string())
        .and_then(|f| Reader::new(f).map_err(|e| e.to_string()))
        .map_err(|e| fail(format!("{}: {e}", file.display())))?;
    fs::create_dir_all(to).map_err(|e| fail(format!("{}: {e}", to.display())))?;
    reader
        .unpack(to)
        .map_err(|e| fail(format!("unpacking {}: {e}", file.display())))?;
    Ok(to.to_path_buf())
}

/// Adds the directory `dir` and everything below it as `inside`.
fn add_tree(archive: &mut Archive, dir: &Path, inside: &str) -> Result<()> {
    archive.add(inside, 0o755, Kind::Dir);
    let mut items: Vec<_> = fs::read_dir(dir)
        .map_err(|e| fail(format!("{}: {e}", dir.display())))?
        .flatten()
        .collect();
    items.sort_by_key(fs::DirEntry::file_name);
    for item in items {
        let path = item.path();
        let name = format!("{inside}/{}", item.file_name().to_string_lossy());
        let kind = item
            .file_type()
            .map_err(|e| fail(format!("{}: {e}", path.display())))?;
        if kind.is_dir() {
            add_tree(archive, &path, &name)?;
        } else if kind.is_symlink() {
            let target =
                fs::read_link(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
            archive.add(
                &name,
                0o777,
                Kind::Symlink(target.to_string_lossy().into_owned()),
            );
        } else {
            archive.add(&name, 0o644, Kind::File(read(&path)?));
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| fail(format!("{}: {e}", path.display())))
}
