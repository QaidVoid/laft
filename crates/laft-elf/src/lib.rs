//! ELF inspection for Laft packages.
//!
//! The builder inspects every file of a package payload with [`inspect`]. It
//! derives `so:` dependencies from the `DT_NEEDED` entries and `so:` provides
//! from `DT_SONAME`, checks search paths, the interpreter and the target
//! machine, and records each shared library's ABI digest. The exports and
//! imports, with their GNU symbol versions, let it decide whether a rebuilt
//! library still satisfies the objects linked against it. A `DT_NEEDED`
//! entry that is a path rather than a soname is reported as a
//! [`Library::Path`], and the builder decides whether to accept it.
//! [`normalize_search_paths`] rewrites the search paths of a payload file
//! in place so that they follow Laft's rules.
//!
//! Payload files must keep their section header table. The dynamic entries,
//! the dynamic symbols and their versions are read from the `SHT_DYNAMIC`,
//! `SHT_DYNSYM` and GNU versioning sections, so a dynamic object whose
//! section headers were stripped is rejected rather than reported with an
//! empty ABI.
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
mod rewrite;
mod symbol;

pub use error::Error;
pub use inspect::{Class, EM_X86_64, Elf, Kind, Library, inspect};
pub use rewrite::normalize_search_paths;
pub use symbol::{Export, ExportKind, Import, SymbolVersion, VersionNeed};
