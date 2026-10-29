//! Reading generations and choosing the one to boot.
//!
//! Only the few fields of `gen.json` and `boot/boot.json` that booting needs
//! are read. Every other field is ignored.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use serde::Deserialize;

/// What booting needs to know about a valid generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    /// The generation number.
    pub number: u64,
    /// The service backend, for example `runit`.
    pub backend: String,
    /// The command line of PID 1.
    pub exec: Vec<String>,
    /// The store id of the kernel package.
    pub kernel_id: String,
    /// The kernel release, for example `6.17.5-laft1`.
    pub kernel_release: String,
    /// The hash naming the generation's initramfs.
    pub initramfs: String,
}

#[derive(Deserialize)]
struct GenJson {
    format: u32,
    generation: u64,
    init: InitJson,
}

#[derive(Deserialize)]
struct InitJson {
    backend: String,
    exec: Vec<String>,
}

#[derive(Deserialize)]
struct BootJson {
    version: u32,
    initramfs: String,
    kernel_id: String,
    kernel_release: String,
}

/// Reads generation `number` from the generations directory `gens`.
///
/// A generation is valid when its `gen.json` is a format 1 manifest of that
/// number with a backend name and a PID 1 command, its `usr/bin` is a
/// directory, and its `boot/boot.json` is a version 1 boot record.
///
/// # Errors
///
/// Fails with the reason when the generation is not valid.
pub fn read(gens: &Path, number: u64) -> Result<Generation, String> {
    let dir = gens.join(number.to_string());
    let gen_json: GenJson = read_json(&dir.join("gen.json"))?;
    if gen_json.format != 1 || gen_json.generation != number {
        return Err(format!(
            "{}/gen.json has an unsupported format or another number",
            dir.display()
        ));
    }
    let backend = gen_json.init.backend;
    if backend.is_empty() || !backend.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(format!("generation {number} has an invalid init backend"));
    }
    if gen_json
        .init
        .exec
        .first()
        .is_none_or(|p| !p.starts_with('/'))
    {
        return Err(format!("generation {number} has no absolute PID 1 command"));
    }
    if !fs::metadata(dir.join("usr/bin")).is_ok_and(|m| m.is_dir()) {
        return Err(format!("{}/usr/bin is not a directory", dir.display()));
    }
    let boot: BootJson = read_json(&dir.join("boot/boot.json"))?;
    if boot.version != 1 {
        return Err(format!(
            "{}/boot/boot.json has an unsupported version",
            dir.display()
        ));
    }
    Ok(Generation {
        number,
        backend,
        exec: gen_json.init.exec,
        kernel_id: boot.kernel_id,
        kernel_release: boot.kernel_release,
        initramfs: boot.initramfs,
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{} is invalid: {e}", path.display()))
}

/// The numbers of the generations in `gens`, the entries named by a
/// number. Other entries, such as the `default` link, are skipped.
///
/// # Errors
///
/// Fails when `gens` cannot be listed completely.
pub fn numbers(gens: &Path) -> io::Result<BTreeSet<u64>> {
    let mut numbers = BTreeSet::new();
    for entry in fs::read_dir(gens)? {
        if let Some(number) = entry?.file_name().to_str().and_then(|n| n.parse().ok()) {
            numbers.insert(number);
        }
    }
    Ok(numbers)
}

/// The generation the symlink `link` names, or `None` when it is missing.
///
/// # Errors
///
/// Fails when the link exists but does not name a generation number.
pub fn read_link(link: &Path) -> Result<Option<u64>, String> {
    let target = match fs::read_link(link) {
        Ok(target) => target,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", link.display())),
    };
    target
        .to_str()
        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|t| t.parse().ok())
        .map(Some)
        .ok_or_else(|| format!("{} does not name a generation", link.display()))
}

/// The generation to boot, with warnings to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The chosen generation.
    pub generation: Generation,
    /// Things worth telling the user about the choice.
    pub warnings: Vec<String>,
}

/// Chooses the generation to boot.
///
/// The `requested` generation from the command line wins, and the `default`
/// generation stands in when none was requested. When the requested one is
/// not valid, the default is used instead if it is valid and built for the
/// `running` kernel release. `read` reads and validates a generation.
///
/// # Errors
///
/// Fails when no valid generation can be chosen.
pub fn select(
    requested: Option<u64>,
    default: Option<u64>,
    running: &str,
    read: impl Fn(u64) -> Result<Generation, String>,
) -> Result<Selection, String> {
    let Some(wanted) = requested.or(default) else {
        return Err(
            "no laft.gen= on the kernel command line and no /laft/gens/default link".to_owned(),
        );
    };
    let mut warnings = Vec::new();
    match read(wanted) {
        Ok(generation) => {
            if generation.kernel_release != running {
                warnings.push(format!(
                    "generation {wanted} is built for kernel {} but kernel {running} is running",
                    generation.kernel_release
                ));
            }
            Ok(Selection {
                generation,
                warnings,
            })
        }
        Err(why) => {
            let fallback = default
                .filter(|&d| d != wanted)
                .and_then(|d| read(d).ok())
                .filter(|g| g.kernel_release == running);
            match fallback {
                Some(generation) => {
                    warnings.push(format!(
                        "generation {wanted} cannot be booted ({why}); booting the default \
                         generation {} instead",
                        generation.number
                    ));
                    Ok(Selection {
                        generation,
                        warnings,
                    })
                }
                None => Err(format!("generation {wanted} cannot be booted: {why}")),
            }
        }
    }
}

#[cfg(test)]
#[path = "generation_tests.rs"]
mod tests;
