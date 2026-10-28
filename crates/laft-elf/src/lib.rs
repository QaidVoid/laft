//! ELF inspection for Laft packages.
//!
//! The builder inspects every file of a package payload with [`inspect`]. It
//! derives `so:` dependencies from the `DT_NEEDED` entries and `so:` provides
//! from `DT_SONAME`, checks search paths, the interpreter and the target
//! machine, and records each shared library's ABI digest. The exports and
//! imports, with their GNU symbol versions, let it decide whether a rebuilt
//! library still satisfies the objects linked against it.
//!
//! ```
//! let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/libversioned.so");
//! let library = laft_elf::inspect(&std::fs::read(path)?)?.expect("the file is ELF");
//! assert_eq!(library.soname.as_ref().map(|s| s.as_str()), Some("libversioned.so.1"));
//! assert!(library.abi_digest().to_string().starts_with("b3:"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod inspect;
mod symbol;

pub use error::Error;
pub use inspect::{Class, EM_X86_64, Elf, Kind, inspect};
pub use symbol::{Export, ExportKind, Import, SymbolVersion, VersionNeed};
