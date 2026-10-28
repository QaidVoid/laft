use laft_core::{Blake3Hash, Soname};
use object::Endianness;
use object::elf;
use object::read::elf::{Dyn, FileHeader, ProgramHeader, SectionHeader, SectionTable, Sym};

use crate::Error;
use crate::symbol::{self, Export, Import, SymbolVersion, VersionNeed};

/// The `e_machine` value of x86-64.
pub const EM_X86_64: u16 = elf::EM_X86_64.0;

/// The word size of an ELF file, from `EI_CLASS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// 32-bit (`ELFCLASS32`).
    Elf32,
    /// 64-bit (`ELFCLASS64`).
    Elf64,
}

/// What an ELF file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// An object file for the link editor (`ET_REL`).
    Relocatable,
    /// A program. This is `ET_EXEC`, or `ET_DYN` with the `DF_1_PIE` flag,
    /// which is how a position-independent executable is marked.
    Executable,
    /// A shared object (`ET_DYN` without `DF_1_PIE`).
    SharedObject,
    /// A core dump (`ET_CORE`).
    Core,
    /// Any other `e_type` value.
    Other(u16),
}

/// What Laft needs to know about one ELF file.
///
/// The dynamic fields come from the `SHT_DYNAMIC` section and the symbol
/// fields from `.dynsym` and the GNU symbol versioning sections. A file
/// without them, such as a static executable or an object file, has no
/// sonames, search paths, exports or imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elf {
    /// The word size.
    pub class: Class,
    /// The file type.
    pub kind: Kind,
    /// The `e_machine` value, for example [`EM_X86_64`].
    pub machine: u16,
    /// The program interpreter from `PT_INTERP`, for example
    /// `/lib/ld-musl-x86_64.so.1`.
    pub interpreter: Option<String>,
    /// The `DT_SONAME` of a shared object.
    pub soname: Option<Soname>,
    /// The `DT_NEEDED` entries, in file order.
    pub needed: Vec<Soname>,
    /// The `DT_RUNPATH` entries, split at colons, in file order. Entries are
    /// returned as written, so they may contain `$ORIGIN`.
    pub runpath: Vec<String>,
    /// The `DT_RPATH` entries, split at colons, in file order.
    pub rpath: Vec<String>,
    /// The GNU build id note.
    pub build_id: Option<Vec<u8>>,
    /// The exported dynamic symbols, sorted, without duplicates.
    pub exports: Vec<Export>,
    /// The imported dynamic symbols, sorted, without duplicates.
    pub imports: Vec<Import>,
}

impl Elf {
    /// Whether this is an executable that runs without a dynamic loader and
    /// loads no shared objects, which means it has neither `PT_INTERP` nor
    /// `DT_NEEDED`. Static position-independent executables count.
    pub fn is_static(&self) -> bool {
        self.kind == Kind::Executable && self.interpreter.is_none() && self.needed.is_empty()
    }

    /// The ABI digest of the exported symbols.
    ///
    /// Each export becomes one line. Functions, including `GNU_IFUNC`, are
    /// `F <name><ver>`, data objects `O <name><ver> <size>`, thread-local
    /// variables `T <name><ver> <size>` and untyped symbols `N <name><ver>`.
    /// `<ver>` is `@@<version>` for a default GNU symbol version,
    /// `@<version>` for a hidden one and empty for an unversioned symbol. In
    /// names and versions, `\`, newline, space and `@` are written as `\x5c`,
    /// `\x0a`, `\x20` and `\x40`, so different exports never give the same
    /// text. The lines are sorted bytewise, duplicates are removed, and they are joined
    /// with newlines with a final newline appended, so an object without
    /// exports hashes a single newline. The digest is the BLAKE3 hash of that
    /// text.
    ///
    /// Two builds of a library with the same digest export the same symbols
    /// at the same versions, with the same data sizes.
    pub fn abi_digest(&self) -> Blake3Hash {
        Blake3Hash::of(symbol::abi_text(&self.exports).as_bytes())
    }
}

/// Inspects the bytes of a file.
///
/// Returns `Ok(None)` when the file does not start with the ELF magic, since
/// most files in a package are not ELF.
///
/// # Errors
///
/// Fails when the file starts with the ELF magic but is malformed. That
/// includes truncated tables, out-of-range offsets and string indexes, an
/// unknown class or byte order, a `PT_DYNAMIC` segment without a dynamic
/// section, a repeated `PT_INTERP`, `DT_SONAME`, `DT_RUNPATH` or `DT_RPATH`, a
/// symbol version table that does not match `.dynsym`, and names or paths that
/// are not UTF-8. Also fails when a `DT_NEEDED`, `DT_SONAME` or version
/// requirement names a library by something other than a soname, such as a
/// path, because such a library cannot be expressed as a `so:` dependency.
///
/// # Examples
///
/// ```
/// let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/program");
/// let elf = laft_elf::inspect(&std::fs::read(path)?)?.expect("the file is ELF");
/// assert_eq!(elf.machine, laft_elf::EM_X86_64);
/// assert_eq!(elf.needed[0].as_str(), "libversioned.so.1");
/// assert!(!elf.is_static());
///
/// assert_eq!(laft_elf::inspect(b"#!/bin/sh\n")?, None);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn inspect(data: &[u8]) -> Result<Option<Elf>, Error> {
    if !data.starts_with(&elf::ELFMAG) {
        return Ok(None);
    }
    let class = data.get(4).copied().map(elf::FileClass);
    if class == Some(elf::ELFCLASS32) {
        parse::<elf::FileHeader32<Endianness>>(data, Class::Elf32).map(Some)
    } else if class == Some(elf::ELFCLASS64) {
        parse::<elf::FileHeader64<Endianness>>(data, Class::Elf64).map(Some)
    } else {
        Err(Error::new("unknown ELF class"))
    }
}

fn parse<H: FileHeader<Endian = Endianness>>(data: &[u8], class: Class) -> Result<Elf, Error> {
    let header = H::parse(data).map_err(Error::object)?;
    let endian = header.endian().map_err(Error::object)?;
    let sections = header.sections(endian, data).map_err(Error::object)?;

    let mut interpreter = None;
    let mut has_dynamic_segment = false;
    for segment in header
        .program_headers(endian, data)
        .map_err(Error::object)?
    {
        if let Some(path) = segment.interpreter(endian, data).map_err(Error::object)? {
            set_once(
                &mut interpreter,
                utf8(path, "PT_INTERP")?.to_owned(),
                "PT_INTERP",
            )?;
        }
        has_dynamic_segment |= segment.p_type(endian) == elf::PT_DYNAMIC;
    }

    let dynamic = read_dynamic(&sections, endian, data)?;
    if has_dynamic_segment && dynamic.is_none() {
        return Err(Error::new("PT_DYNAMIC segment without a dynamic section"));
    }
    let dynamic = dynamic.unwrap_or_default();

    let kind = match header.e_type(endian) {
        elf::ET_REL => Kind::Relocatable,
        elf::ET_EXEC => Kind::Executable,
        elf::ET_DYN if dynamic.pie => Kind::Executable,
        elf::ET_DYN => Kind::SharedObject,
        elf::ET_CORE => Kind::Core,
        other => Kind::Other(other.0),
    };
    let (exports, imports) = read_symbols(&sections, endian, data)?;

    Ok(Elf {
        class,
        kind,
        machine: header.e_machine(endian).0,
        interpreter,
        soname: dynamic.soname,
        needed: dynamic.needed,
        runpath: dynamic.runpath.unwrap_or_default(),
        rpath: dynamic.rpath.unwrap_or_default(),
        build_id: read_build_id(&sections, endian, data)?,
        exports,
        imports,
    })
}

#[derive(Default)]
struct Dynamic {
    soname: Option<Soname>,
    needed: Vec<Soname>,
    runpath: Option<Vec<String>>,
    rpath: Option<Vec<String>>,
    pie: bool,
}

fn read_dynamic<H: FileHeader<Endian = Endianness>>(
    sections: &SectionTable<'_, H>,
    endian: Endianness,
    data: &[u8],
) -> Result<Option<Dynamic>, Error> {
    let Some((entries, strtab)) = sections.dynamic(endian, data).map_err(Error::object)? else {
        return Ok(None);
    };
    let strings = sections
        .strings(endian, data, strtab)
        .map_err(Error::object)?;
    let string = |entry: &H::Dyn, tag: &str| {
        let bytes = entry.string(endian, strings).map_err(Error::object)?;
        utf8(bytes, tag)
    };

    let mut dynamic = Dynamic::default();
    for entry in entries {
        match entry.tag(endian) {
            elf::DT_NULL => break,
            elf::DT_NEEDED => {
                let needed = soname(string(entry, "DT_NEEDED")?, "DT_NEEDED")?;
                dynamic.needed.push(needed);
            }
            elf::DT_SONAME => {
                let name = soname(string(entry, "DT_SONAME")?, "DT_SONAME")?;
                set_once(&mut dynamic.soname, name, "DT_SONAME")?;
            }
            elf::DT_RUNPATH => {
                let path = search_path(string(entry, "DT_RUNPATH")?);
                set_once(&mut dynamic.runpath, path, "DT_RUNPATH")?;
            }
            elf::DT_RPATH => {
                let path = search_path(string(entry, "DT_RPATH")?);
                set_once(&mut dynamic.rpath, path, "DT_RPATH")?;
            }
            elf::DT_FLAGS_1 => dynamic.pie = entry.val(endian) & elf::DF_1_PIE.0 != 0,
            _ => {}
        }
    }
    Ok(Some(dynamic))
}

fn read_symbols<H: FileHeader<Endian = Endianness>>(
    sections: &SectionTable<'_, H>,
    endian: Endianness,
    data: &[u8],
) -> Result<(Vec<Export>, Vec<Import>), Error> {
    let symbols = sections
        .symbols(endian, data, elf::SHT_DYNSYM)
        .map_err(Error::object)?;
    if let Some((versyms, _)) = sections.gnu_versym(endian, data).map_err(Error::object)?
        && versyms.len() != symbols.len()
    {
        return Err(Error::new(
            "the symbol version table does not match the dynamic symbol table",
        ));
    }
    let versions = sections.versions(endian, data).map_err(Error::object)?;

    let mut exports = Vec::new();
    let mut imports = Vec::new();
    for (index, sym) in symbols.enumerate().skip(1) {
        let bind = sym.st_bind();
        let defined = sym.st_shndx(endian) != elf::SHN_UNDEF;
        let name = utf8(
            sym.name(endian, symbols.strings()).map_err(Error::object)?,
            "symbol name",
        )?;
        let version = match &versions {
            Some(table) => {
                let index = table.version_index(endian, index);
                table
                    .version(index.index())
                    .map_err(Error::object)?
                    .map(|version| (version, index.is_hidden()))
            }
            None => None,
        };

        let weak = bind == elf::STB_WEAK;
        let referenced = matches!(bind, elf::STB_GLOBAL | elf::STB_WEAK);
        if let Some((version, _)) = version
            && let Some(file) = version.file()
        {
            if referenced {
                imports.push(Import {
                    name: name.to_owned(),
                    version: Some(VersionNeed {
                        name: utf8(version.name(), "symbol version")?.to_owned(),
                        library: soname(utf8(file, "version library")?, "version library")?,
                    }),
                    weak,
                });
            }
        } else if defined {
            let size = sym.st_size(endian).into();
            let Some(kind) = symbol::export_kind(bind, sym.st_type(), sym.st_visibility(), size)
            else {
                continue;
            };
            let version = match version {
                Some((version, hidden)) => Some(SymbolVersion {
                    name: utf8(version.name(), "symbol version")?.to_owned(),
                    hidden,
                }),
                None => None,
            };
            exports.push(Export {
                name: name.to_owned(),
                version,
                kind,
            });
        } else if version.is_some() {
            return Err(Error::new(format!(
                "undefined symbol {name:?} has a version definition"
            )));
        } else if referenced && !name.is_empty() {
            imports.push(Import {
                name: name.to_owned(),
                version: None,
                weak,
            });
        }
    }
    exports.sort_unstable();
    exports.dedup();
    imports.sort_unstable();
    imports.dedup();
    Ok((exports, imports))
}

fn read_build_id<H: FileHeader<Endian = Endianness>>(
    sections: &SectionTable<'_, H>,
    endian: Endianness,
    data: &[u8],
) -> Result<Option<Vec<u8>>, Error> {
    for section in sections.iter() {
        let Some(mut notes) = section.notes(endian, data).map_err(Error::object)? else {
            continue;
        };
        while let Some(note) = notes.next().map_err(Error::object)? {
            if note.name() == elf::ELF_NOTE_GNU && note.n_type(endian) == elf::NT_GNU_BUILD_ID {
                return Ok(Some(note.desc().to_vec()));
            }
        }
    }
    Ok(None)
}

fn set_once<T>(slot: &mut Option<T>, value: T, what: &str) -> Result<(), Error> {
    if slot.replace(value).is_some() {
        return Err(Error::new(format!("more than one {what} entry")));
    }
    Ok(())
}

fn utf8<'a>(bytes: &'a [u8], what: &str) -> Result<&'a str, Error> {
    std::str::from_utf8(bytes).map_err(|_| Error::new(format!("{what} is not UTF-8")))
}

fn soname(name: &str, what: &str) -> Result<Soname, Error> {
    name.parse()
        .map_err(|error| Error::new(format!("{what} is not a soname: {error}")))
}

fn search_path(path: &str) -> Vec<String> {
    path.split(':').map(str::to_owned).collect()
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod tests;
