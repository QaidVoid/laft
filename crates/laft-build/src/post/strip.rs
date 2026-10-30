//! The step that strips ELF files and splits off their debug info.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use laft_elf::Kind;

use crate::BuildOptions;
use crate::error::{BuildError, io};
use crate::exec::Executor;
use crate::fsx::{self, Kind as PathKind};
use crate::post::ElfFiles;

/// Where split debug info goes, below the payload root.
pub const DEBUG_DIR: &str = "usr/lib/debug";

/// The payload path of the debug file for the build id `id`:
/// `usr/lib/debug/.build-id/<xx>/<rest>.debug`.
pub fn debug_path(id: &[u8]) -> Option<String> {
    let hex = fsx::hex(id);
    (hex.len() > 2).then(|| format!("{DEBUG_DIR}/.build-id/{}/{}.debug", &hex[..2], &hex[2..]))
}

/// Strips the ELF files of the payload `out` when the recipe's `strip`
/// is set, running `objcopy` and `strip` through `exec`.
///
/// With `debug_split`, every executable and shared object with a build id
/// first has its debug info copied to [`debug_path`] and gets a
/// `.gnu_debuglink` to it. Executables are then stripped with
/// `--strip-all`, shared objects with `--strip-unneeded`, and object files
/// and static archives with `--strip-debug`. Archive members get a zero
/// time stamp, owner and group and the same mode, whatever the default of
/// the binutils at hand, so that archives are reproducible. Files under
/// `usr/lib/modules/` and `usr/lib/debug/` are left alone, and each file is
/// handled once even when the payload links it under several names.
///
/// # Errors
///
/// Fails with `B040` when a tool fails, and when the payload cannot be
/// read or changed.
pub fn strip(
    out: &Path,
    elves: &ElfFiles,
    options: &BuildOptions,
    exec: &Executor,
) -> Result<(), BuildError> {
    if !options.strip {
        return Ok(());
    }
    let mut seen = BTreeSet::new();
    for (rel, kind) in fsx::walk(out)? {
        if kind != PathKind::File
            || rel.starts_with("usr/lib/modules/")
            || rel.starts_with(&format!("{DEBUG_DIR}/"))
        {
            continue;
        }
        let path = out.join(&rel);
        let metadata = fs::symlink_metadata(&path).map_err(io(&path))?;
        if !seen.insert((metadata.dev(), metadata.ino())) {
            continue;
        }
        let (flag, archive) = match elves.get(&rel).map(|e| e.kind) {
            Some(Kind::Executable) => ("--strip-all", false),
            Some(Kind::SharedObject) => ("--strip-unneeded", false),
            Some(Kind::Relocatable) => ("--strip-debug", false),
            Some(_) => continue,
            None if is_archive(&path)? => ("--strip-debug", true),
            None => continue,
        };
        let mode = metadata.permissions().mode() & 0o7777;
        fsx::set_mode(&path, mode | 0o200)?;
        let stripped = (|| {
            let file = exec.out_path(&rel);
            let split = options.debug_split && flag != "--strip-debug";
            if let Some(debug) = elves
                .get(&rel)
                .and_then(|e| e.build_id.as_deref())
                .and_then(debug_path)
                .filter(|_| split)
            {
                let host = out.join(&debug);
                if let Some(dir) = host.parent() {
                    fs::create_dir_all(dir).map_err(io(dir))?;
                }
                let target = exec.out_path(&debug);
                exec.tool(
                    "strip",
                    "objcopy",
                    &["--only-keep-debug".into(), file.clone(), target.clone()],
                )?;
                exec.tool(
                    "strip",
                    "objcopy",
                    &[format!("--add-gnu-debuglink={target}"), file.clone()],
                )?;
                fsx::set_mode(&host, 0o444)?;
            }
            let mut args = vec![flag.to_owned()];
            if archive {
                args.push("--enable-deterministic-archives".to_owned());
            }
            args.push(file);
            exec.tool("strip", "strip", &args)
        })();
        fsx::set_mode(&path, mode)?;
        stripped?;
    }
    Ok(())
}

/// Whether the file `path` is an `ar` archive.
fn is_archive(path: &Path) -> Result<bool, BuildError> {
    let mut magic = [0u8; 8];
    let mut file = fs::File::open(path).map_err(io(path))?;
    let n = file.read(&mut magic).map_err(io(path))?;
    Ok(n == 8 && magic == *b"!<arch>\n")
}

#[cfg(test)]
#[path = "strip_tests.rs"]
mod tests;
