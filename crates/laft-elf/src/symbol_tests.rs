use super::*;

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
fn abi_lines_mark_default_and_hidden_versions() {
    let exports = [
        export("add", Some(("VERS_2", false)), ExportKind::Function),
        export("add", Some(("VERS_1", true)), ExportKind::Function),
        export("plain", None, ExportKind::Function),
    ];
    assert_eq!(abi_text(&exports), "F add@@VERS_2\nF add@VERS_1\nF plain\n");
}

#[test]
fn abi_lines_carry_sizes_of_data_only() {
    let exports = [
        export("counter", None, ExportKind::Object { size: 4 }),
        export("slot", Some(("V", false)), ExportKind::Tls { size: 8 }),
        export("label", None, ExportKind::NoType),
    ];
    assert_eq!(abi_text(&exports), "N label\nO counter 4\nT slot@@V 8\n");
}

#[test]
fn abi_lines_sort_bytewise_and_drop_duplicates() {
    let exports = [
        export("b", None, ExportKind::Function),
        export("B", None, ExportKind::Function),
        export("b", None, ExportKind::Function),
        export("_a", None, ExportKind::Function),
    ];
    assert_eq!(abi_text(&exports), "F B\nF _a\nF b\n");
}

#[test]
fn no_exports_hash_a_single_newline() {
    assert_eq!(abi_text(&[]), "\n");
}

#[test]
fn names_cannot_forge_lines_or_versions() {
    let function = |name| export(name, None, ExportKind::Function);
    let distinct_pairs = [
        (vec![function("a\nF b")], vec![function("a"), function("b")]),
        (
            vec![function("add@@VERS_2")],
            vec![export("add", Some(("VERS_2", false)), ExportKind::Function)],
        ),
        (
            vec![export("add", Some(("V@1", true)), ExportKind::Function)],
            vec![export("add@V", Some(("1", true)), ExportKind::Function)],
        ),
        (vec![function("a\\x40b")], vec![function("a@b")]),
    ];
    for (left, right) in distinct_pairs {
        assert_ne!(abi_text(&left), abi_text(&right), "{left:?} and {right:?}");
    }
}

#[test]
fn special_characters_are_written_as_hex_escapes() {
    let exports = [export(
        "a b\\c\nd@e",
        Some(("V 1", false)),
        ExportKind::Function,
    )];
    assert_eq!(abi_text(&exports), "F a\\x20b\\x5cc\\x0ad\\x40e@@V\\x201\n");
}

#[test]
fn exports_need_global_binding_and_default_or_protected_visibility() {
    let kind = |bind, visibility| export_kind(bind, elf::STT_FUNC, visibility, 0);
    for bind in [elf::STB_GLOBAL, elf::STB_WEAK, elf::STB_GNU_UNIQUE] {
        assert_eq!(kind(bind, elf::STV_DEFAULT), Some(ExportKind::Function));
        assert_eq!(kind(bind, elf::STV_PROTECTED), Some(ExportKind::Function));
        assert_eq!(kind(bind, elf::STV_HIDDEN), None);
        assert_eq!(kind(bind, elf::STV_INTERNAL), None);
    }
    assert_eq!(kind(elf::STB_LOCAL, elf::STV_DEFAULT), None);
}

#[test]
fn exports_keep_code_data_and_untyped_symbols_only() {
    let kind = |kind| export_kind(elf::STB_GLOBAL, kind, elf::STV_DEFAULT, 16);
    assert_eq!(kind(elf::STT_FUNC), Some(ExportKind::Function));
    assert_eq!(kind(elf::STT_GNU_IFUNC), Some(ExportKind::Function));
    assert_eq!(kind(elf::STT_OBJECT), Some(ExportKind::Object { size: 16 }));
    assert_eq!(kind(elf::STT_TLS), Some(ExportKind::Tls { size: 16 }));
    assert_eq!(kind(elf::STT_NOTYPE), Some(ExportKind::NoType));
    assert_eq!(kind(elf::STT_SECTION), None);
    assert_eq!(kind(elf::STT_FILE), None);
    assert_eq!(kind(elf::STT_COMMON), None);
}
