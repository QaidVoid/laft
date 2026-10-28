//! Inspects the committed ELF fixtures through the public API.

use std::fs;
use std::path::Path;

use laft_core::Blake3Hash;
use laft_elf::{
    Class, EM_X86_64, Elf, Export, ExportKind, Import, Kind, Library, SymbolVersion, VersionNeed,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn inspect(name: &str) -> Elf {
    laft_elf::inspect(&fixture(name))
        .expect("fixtures are well-formed")
        .expect("fixture is ELF")
}

fn export(name: &str, version: Option<(&str, bool)>, kind: ExportKind) -> Export {
    Export {
        name: name.to_owned(),
        version: version.map(|(name, hidden)| SymbolVersion {
            name: name.to_owned(),
            hidden,
        }),
        kind,
    }
}

#[test]
fn source_file_is_not_elf() {
    assert_eq!(laft_elf::inspect(&fixture("libversioned.c")), Ok(None));
}

#[test]
fn shared_library_reports_soname_rpath_and_versioned_exports() {
    let library = inspect("libversioned.so");
    assert_eq!(library.class, Class::Elf64);
    assert_eq!(library.machine, EM_X86_64);
    assert_eq!(library.kind, Kind::SharedObject);
    assert_eq!(
        library.soname.as_ref().map(|s| s.as_str()),
        Some("libversioned.so.1")
    );
    assert_eq!(library.rpath, ["/opt/legacy/lib"]);
    assert!(library.runpath.is_empty());
    assert!(library.needed.is_empty());
    assert_eq!(library.interpreter, None);
    assert!(!library.is_static());
    assert_eq!(library.build_id.as_ref().map(Vec::len), Some(20));
    assert!(library.imports.is_empty());

    let default = |version| Some((version, false));
    assert_eq!(
        library.exports,
        [
            export("VERS_1", default("VERS_1"), ExportKind::Object { size: 0 }),
            export("VERS_2", default("VERS_2"), ExportKind::Object { size: 0 }),
            export("add", Some(("VERS_1", true)), ExportKind::Function),
            export("add", default("VERS_2"), ExportKind::Function),
            export("counter", default("VERS_1"), ExportKind::Object { size: 4 }),
            export("indirect", default("VERS_2"), ExportKind::Function),
            export("marker", default("VERS_2"), ExportKind::NoType),
            export("protected_fn", default("VERS_2"), ExportKind::Function),
            export("tls_slot", default("VERS_2"), ExportKind::Tls { size: 8 }),
            export("unversioned_fn", None, ExportKind::Function),
            export("weak_hook", default("VERS_2"), ExportKind::Function),
        ]
    );
}

#[test]
fn abi_digest_matches_the_digest_computed_with_readelf_and_b3sum() {
    let expected = String::from_utf8(fixture("libversioned.abi.b3")).expect("b3sum prints hex");
    let expected = expected.trim();
    let mut bytes = [0u8; 32];
    for (byte, pair) in bytes.iter_mut().zip(expected.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).expect("hex is ASCII");
        *byte = u8::from_str_radix(pair, 16).expect("b3sum prints hex");
    }
    assert_eq!(expected.len(), 64);
    assert_eq!(
        inspect("libversioned.so").abi_digest(),
        Blake3Hash::from_bytes(bytes)
    );
}

#[test]
fn abi_digest_ignores_everything_but_exports() {
    let library = inspect("libversioned.so");
    let mut other = library.clone();
    other.soname = None;
    other.rpath.clear();
    other.build_id = None;
    assert_eq!(other.abi_digest(), library.abi_digest());
    other.exports.pop();
    assert_ne!(other.abi_digest(), library.abi_digest());
}

#[test]
fn executable_reports_interpreter_needed_runpath_and_imports() {
    let program = inspect("program");
    assert_eq!(program.kind, Kind::Executable);
    assert!(!program.is_static());
    assert_eq!(
        program.interpreter.as_deref(),
        Some("/lib64/ld-linux-x86-64.so.2")
    );
    let needed: Vec<&str> = program.needed.iter().map(|s| s.as_str()).collect();
    assert_eq!(needed, ["libversioned.so.1"]);
    assert_eq!(program.runpath, ["$ORIGIN/../lib", "/usr/lib"]);
    assert!(program.rpath.is_empty());
    assert_eq!(program.soname, None);
    assert!(program.exports.is_empty(), "{:?}", program.exports);

    let need = |version: &str| VersionNeed {
        name: version.to_owned(),
        library: Library::Soname("libversioned.so.1".parse().expect("valid soname")),
    };
    assert_eq!(
        program.imports,
        [
            Import {
                name: "add".into(),
                version: Some(need("VERS_2")),
                weak: false,
            },
            Import {
                name: "counter".into(),
                version: Some(need("VERS_1")),
                weak: false,
            },
            Import {
                name: "optional_hook".into(),
                version: None,
                weak: true,
            },
        ]
    );
}

#[test]
fn needed_paths_are_reported_rather_than_rejected() {
    let program = inspect("needs-path");
    let path = Library::Path("./libversioned.so".into());
    assert_eq!(program.needed, std::slice::from_ref(&path));
    assert!(!program.is_static());
    let add = program
        .imports
        .iter()
        .find(|import| import.name == "add")
        .expect("add is imported");
    assert_eq!(
        add.version,
        Some(VersionNeed {
            name: "VERS_2".into(),
            library: path,
        })
    );
}

#[test]
fn static_executables_are_static() {
    for name in ["static", "static-pie"] {
        let program = inspect(name);
        assert_eq!(program.kind, Kind::Executable, "{name}");
        assert!(program.is_static(), "{name}");
        assert_eq!(program.interpreter, None, "{name}");
        assert!(program.needed.is_empty(), "{name}");
        assert!(
            program.exports.is_empty() && program.imports.is_empty(),
            "{name}"
        );
    }
}

#[test]
fn truncated_library_is_an_error() {
    let library = fixture("libversioned.so");
    for len in [5, 63, 64, 1024, library.len() - 1] {
        assert!(
            laft_elf::inspect(&library[..len]).is_err(),
            "truncated to {len} bytes"
        );
    }
}
