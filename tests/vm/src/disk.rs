//! The disk image: a GPT disk with an empty EFI system partition and an
//! empty ext4 root, made without root privileges.

use std::fs::{self, File};
use std::path::Path;
use std::process::Command;

use crate::{Result, fail, run, run_with_input};

/// The size of the EFI system partition, in MiB.
const ESP_MIB: u64 = 128;

/// The size of the root partition, in MiB.
const ROOT_MIB: u64 = 768;

/// Where the first partition starts, in MiB.
const FIRST_MIB: u64 = 1;

/// The sector size of the image.
const SECTOR: u64 = 512;

/// Creates the disk image `disk`, with partition 1 an EFI system partition
/// holding an empty FAT32 filesystem and partition 2 an empty ext4
/// filesystem. The filesystems are made in files next to `disk`, written
/// into the image and removed.
///
/// # Errors
///
/// Fails when a tool fails or a file cannot be written.
pub(crate) fn create(disk: &Path) -> Result<()> {
    let dir = disk.parent().unwrap_or(Path::new("."));
    let total_mib = FIRST_MIB + ESP_MIB + ROOT_MIB + 1;
    let _ = fs::remove_file(disk);
    File::create(disk)
        .and_then(|f| f.set_len(total_mib << 20))
        .map_err(|e| fail(format!("{}: {e}", disk.display())))?;
    let mib = (1 << 20) / SECTOR;
    let script = format!(
        "label: gpt\n\
         start={}, size={}, type=U, name=esp\n\
         start={}, size={}, type=L, name=root\n",
        FIRST_MIB * mib,
        ESP_MIB * mib,
        (FIRST_MIB + ESP_MIB) * mib,
        ROOT_MIB * mib,
    );
    run_with_input(
        Command::new("sfdisk").arg("--quiet").arg(disk),
        script.as_bytes(),
    )?;

    let esp = dir.join("esp.img");
    let root = dir.join("root.img");
    let _ = fs::remove_file(&esp);
    run(Command::new("mkfs.vfat")
        .args(["-F", "32", "-n", "ESP", "-C"])
        .arg(&esp)
        .arg((ESP_MIB * 1024).to_string()))?;
    File::create(&root)
        .and_then(|f| f.set_len(ROOT_MIB << 20))
        .map_err(|e| fail(format!("{}: {e}", root.display())))?;
    run(Command::new("mkfs.ext4")
        .args(["-q", "-F", "-L", "laft-root"])
        .arg(&root))?;
    for (image, at) in [(&esp, FIRST_MIB), (&root, FIRST_MIB + ESP_MIB)] {
        run(Command::new("dd")
            .arg(format!("if={}", image.display()))
            .arg(format!("of={}", disk.display()))
            .args(["bs=1M", "conv=notrunc,sparse", "status=none"])
            .arg(format!("seek={at}")))?;
        fs::remove_file(image).map_err(|e| fail(format!("{}: {e}", image.display())))?;
    }
    Ok(())
}
