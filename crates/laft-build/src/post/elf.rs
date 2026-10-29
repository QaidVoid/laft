//! The steps that inspect ELF files: search paths, library placement and
//! ABI digests.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use laft_core::Soname;
use laft_elf::{EM_X86_64, Elf, Kind, Library};
use laft_pkg::Abi;

use crate::error::{BuildError, Warning, io};
use crate::fsx::{self, Kind as PathKind};

/// Every ELF file of a payload, by payload path, as inspected before
/// stripping.
pub type ElfFiles = BTreeMap<String, Elf>;

/// Reads the file `path` when it starts with the ELF magic.
pub(crate) fn read_elf_bytes(path: &Path) -> Result<Option<Vec<u8>>, BuildError> {
    use std::io::Read;
    let mut magic = [0u8; 4];
    let mut file = fs::File::open(path).map_err(io(path))?;
    let n = file.read(&mut magic).map_err(io(path))?;
    if n < 4 || magic != *b"\x7fELF" {
        return Ok(None);
    }
    fs::read(path).map(Some).map_err(io(path))
}

/// Inspects the file `path` when it is ELF.
pub(crate) fn read_elf(path: &Path) -> Result<Option<Elf>, BuildError> {
    let Some(data) = read_elf_bytes(path)? else {
        return Ok(None);
    };
    Ok(laft_elf::inspect(&data).ok().flatten())
}

/// Whether the payload path `rel` lies directly in `usr/lib/`.
fn in_usr_lib(rel: &str) -> bool {
    fsx::parent(rel) == "usr/lib"
}

/// Whether a search path entry leads into the build or output directory.
fn leaks(entry: &str) -> bool {
    ["/build", "/out"]
        .iter()
        .any(|p| entry == *p || entry.starts_with(&format!("{p}/")))
}

/// P6: inspects every ELF file of the payload `out` before it is stripped.
///
/// Search paths are rewritten first: an entry of only `/usr/lib` is
/// removed and `DT_RPATH` becomes `DT_RUNPATH`. Then a search path into
/// `/build` or `/out` is `B107`, another entry that does not start with
/// `$ORIGIN` is warning `W111`, and a machine other than x86-64 is `B108`,
/// except for firmware under `usr/lib/firmware/`. A file that starts with
/// the ELF magic but cannot be read as ELF is skipped with warning `W115`.
///
/// # Errors
///
/// Fails with `B107` or `B108`, and when the payload cannot be read or
/// changed.
pub fn scan_elf(out: &Path, warnings: &mut Vec<Warning>) -> Result<ElfFiles, BuildError> {
    let mut files = ElfFiles::new();
    for (rel, kind) in fsx::walk(out)? {
        if kind != PathKind::File {
            continue;
        }
        let path = out.join(&rel);
        let Some(mut data) = read_elf_bytes(&path)? else {
            continue;
        };
        let elf = laft_elf::normalize_search_paths(&mut data).and_then(|changed| {
            let elf = laft_elf::inspect(&data)?;
            Ok((changed, elf))
        });
        let (changed, elf) = match elf {
            Ok((changed, Some(elf))) => (changed, elf),
            Ok((_, None)) => continue,
            Err(e) => {
                warnings.push(Warning::new("W115", format!("{rel} is skipped: {e}")));
                continue;
            }
        };
        if changed {
            fsx::replace_file(&path, &data)?;
        }
        if elf.machine != EM_X86_64 && !rel.starts_with("usr/lib/firmware/") {
            return Err(BuildError::problem(
                "B108",
                format!("{rel} is built for machine {}, not x86-64", elf.machine),
            ));
        }
        for entry in elf.runpath.iter().chain(&elf.rpath) {
            if leaks(entry) {
                return Err(BuildError::problem(
                    "B107",
                    format!("{rel} searches libraries in {entry}, a directory of the build"),
                ));
            }
            if !entry.starts_with("$ORIGIN") && !entry.starts_with("${ORIGIN}") {
                warnings.push(Warning::new(
                    "W111",
                    format!(
                        "{rel} searches libraries in {entry}, which is not relative to $ORIGIN"
                    ),
                ));
            }
        }
        files.insert(rel, elf);
    }
    Ok(files)
}

/// P6, rule 1: makes every shared object directly in `usr/lib/` reachable
/// by its soname, because views have no linker cache.
///
/// For a library with `DT_SONAME` `S`, `usr/lib/S` must resolve to it. A
/// missing link is created as a relative symlink. A link that resolves to
/// something else is `B112`.
///
/// # Errors
///
/// Fails with `B112`, and when the payload cannot be changed.
pub fn soname_links(out: &Path, elves: &ElfFiles) -> Result<(), BuildError> {
    for (rel, elf) in elves {
        let Some(soname) = &elf.soname else {
            continue;
        };
        if elf.kind != Kind::SharedObject || !in_usr_lib(rel) {
            continue;
        }
        let link = format!("usr/lib/{soname}");
        if link == *rel {
            continue;
        }
        let path = out.join(&link);
        if fs::symlink_metadata(&path).is_err() {
            symlink(fsx::file_name(rel), &path).map_err(io(&path))?;
            continue;
        }
        let target = fsx::resolve(out, &link);
        if target.as_deref() != Some(rel.as_str()) {
            let shown = target.unwrap_or_else(|| "nothing in the payload".to_owned());
            return Err(BuildError::problem(
                "B112",
                format!("{link} points to {shown} but the SONAME of {rel} is {soname}"),
            ));
        }
    }
    Ok(())
}

/// The names under which the payload's shared objects can be loaded: their
/// sonames and file names, directly in `usr/lib/` and, by directory,
/// everywhere else.
pub(crate) struct Provided {
    pub(crate) usr_lib: BTreeSet<String>,
    pub(crate) private: BTreeMap<String, BTreeSet<String>>,
}

pub(crate) fn provided(elves: &ElfFiles) -> Provided {
    let mut usr_lib = BTreeSet::new();
    let mut private: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (rel, elf) in elves {
        if elf.kind != Kind::SharedObject {
            continue;
        }
        let names = elf
            .soname
            .iter()
            .map(|s| s.to_string())
            .chain([fsx::file_name(rel).to_owned()]);
        for name in names {
            if in_usr_lib(rel) {
                usr_lib.insert(name);
            } else {
                private
                    .entry(name)
                    .or_default()
                    .insert(fsx::parent(rel).to_owned());
            }
        }
    }
    Provided { usr_lib, private }
}

/// P6, rule 2: every object that needs a library which the payload has only
/// outside `usr/lib/` must reach it through a `DT_RUNPATH` entry, which
/// expands `$ORIGIN` to the object's own directory. Otherwise the build
/// fails with `B113`.
///
/// # Errors
///
/// Fails with `B113`.
pub fn check_private_libraries(elves: &ElfFiles) -> Result<(), BuildError> {
    let provided = provided(elves);
    for (rel, elf) in elves {
        let origin = format!("/{}", fsx::parent(rel));
        let reachable: BTreeSet<String> = elf
            .runpath
            .iter()
            .map(|e| fsx::normalize(&e.replace("${ORIGIN}", &origin).replace("$ORIGIN", &origin)))
            .collect();
        for need in &elf.needed {
            let Library::Soname(need) = need else {
                continue;
            };
            let name = need.to_string();
            if provided.usr_lib.contains(&name) {
                continue;
            }
            let Some(dirs) = provided.private.get(&name) else {
                continue;
            };
            if !dirs.iter().any(|d| reachable.contains(&format!("/{d}"))) {
                let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
                return Err(BuildError::problem(
                    "B113",
                    format!(
                        "{rel} needs {name}, which is only in {}. Add RUNPATH $ORIGIN",
                        dirs.join(", ")
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// P6, rule 3: warns with `W114` about every needed library that neither
/// the payload nor a package of the build view provides. `view` holds the
/// sonames the build view provides, and is `None` when they are unknown,
/// which skips the check.
pub fn check_needs(elves: &ElfFiles, view: Option<&BTreeSet<Soname>>, warnings: &mut Vec<Warning>) {
    let Some(view) = view else {
        return;
    };
    let provided = provided(elves);
    for (rel, elf) in elves {
        for need in &elf.needed {
            let Library::Soname(soname) = need else {
                continue;
            };
            let name = soname.to_string();
            if !provided.usr_lib.contains(&name)
                && !provided.private.contains_key(&name)
                && !view.contains(soname)
            {
                warnings.push(Warning::new(
                    "W114",
                    format!("{rel} needs {name}, which no package of the build view provides"),
                ));
            }
        }
    }
}

/// P8: the ABI digest of every shared object directly in `usr/lib/` that
/// has a `DT_SONAME`, by payload path.
pub fn abi_digests(elves: &ElfFiles) -> BTreeMap<String, Abi> {
    elves
        .iter()
        .filter(|(rel, elf)| elf.kind == Kind::SharedObject && in_usr_lib(rel))
        .filter_map(|(rel, elf)| {
            let soname = elf.soname.clone()?;
            Some((
                rel.clone(),
                Abi {
                    soname,
                    digest: elf.abi_digest(),
                },
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "elf_tests.rs"]
mod tests;
