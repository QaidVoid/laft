use laft_core::Soname;
use object::elf::{self, SymbolBind, SymbolType, SymbolVisibility};

/// A symbol that a dynamic object defines in `.dynsym` for other objects to
/// bind to.
///
/// Only symbols that count towards the ABI digest are exports: defined
/// symbols with `GLOBAL`, `WEAK` or `GNU_UNIQUE` binding, `DEFAULT` or
/// `PROTECTED` visibility, and type `FUNC`, `GNU_IFUNC`, `OBJECT`, `TLS` or
/// `NOTYPE`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Export {
    /// The symbol name, without a version suffix.
    pub name: String,
    /// The version the symbol is defined at, or `None` when it is
    /// unversioned.
    pub version: Option<SymbolVersion>,
    /// What the symbol refers to.
    pub kind: ExportKind,
}

/// The GNU symbol version an export is defined at, from `.gnu.version` and
/// `.gnu.version_d`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolVersion {
    /// The version name, for example `GLIBC_2.34`.
    pub name: String,
    /// Whether the version is hidden. A hidden version (`name@VERSION`) only
    /// satisfies references to that exact version. The default version
    /// (`name@@VERSION`) also satisfies new links against the bare name.
    pub hidden: bool,
}

/// The kind of an exported symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExportKind {
    /// A function, including a `GNU_IFUNC` resolved at load time.
    Function,
    /// A data object.
    Object {
        /// The size of the object in bytes.
        size: u64,
    },
    /// A thread-local variable.
    Tls {
        /// The size of the variable in bytes.
        size: u64,
    },
    /// A symbol without a type, such as a label defined in assembly.
    NoType,
}

/// A symbol that a dynamic object expects another object to define.
///
/// Imports are the undefined `GLOBAL` and `WEAK` symbols of `.dynsym`, and
/// the defined ones whose version is a requirement on another library. The
/// latter are data objects that an executable copied out of a library at
/// link time and still needs that library to provide.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Import {
    /// The symbol name, without a version suffix.
    pub name: String,
    /// The version required, or `None` when the reference is unversioned.
    pub version: Option<VersionNeed>,
    /// Whether the reference is weak, so the object still loads when no
    /// library defines the symbol.
    pub weak: bool,
}

/// A GNU symbol version requirement, from `.gnu.version_r`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VersionNeed {
    /// The version name, for example `GLIBC_2.34`.
    pub name: String,
    /// The `DT_NEEDED` library the version must come from.
    pub library: Soname,
}

impl Export {
    fn abi_line(&self) -> String {
        let name = match &self.version {
            None => self.name.clone(),
            Some(version) if version.hidden => format!("{}@{}", self.name, version.name),
            Some(version) => format!("{}@@{}", self.name, version.name),
        };
        match self.kind {
            ExportKind::Function => format!("F {name}"),
            ExportKind::Object { size } => format!("O {name} {size}"),
            ExportKind::Tls { size } => format!("T {name} {size}"),
            ExportKind::NoType => format!("N {name}"),
        }
    }
}

/// Classifies a defined dynamic symbol, or returns `None` when it is not an
/// export.
pub(crate) fn export_kind(
    bind: SymbolBind,
    kind: SymbolType,
    visibility: SymbolVisibility,
    size: u64,
) -> Option<ExportKind> {
    if !matches!(bind, elf::STB_GLOBAL | elf::STB_WEAK | elf::STB_GNU_UNIQUE)
        || !matches!(visibility, elf::STV_DEFAULT | elf::STV_PROTECTED)
    {
        return None;
    }
    match kind {
        elf::STT_FUNC | elf::STT_GNU_IFUNC => Some(ExportKind::Function),
        elf::STT_OBJECT => Some(ExportKind::Object { size }),
        elf::STT_TLS => Some(ExportKind::Tls { size }),
        elf::STT_NOTYPE => Some(ExportKind::NoType),
        _ => None,
    }
}

/// The text the ABI digest hashes: one line per export, sorted bytewise and
/// without duplicates.
///
/// The lines are joined with newlines and a final newline is appended, so an
/// object without exports yields a single newline.
pub(crate) fn abi_text(exports: &[Export]) -> String {
    let mut lines: Vec<String> = exports.iter().map(Export::abi_line).collect();
    lines.sort_unstable();
    lines.dedup();
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
#[path = "symbol_tests.rs"]
mod tests;
