use super::*;

/// An ELF64 header with one program header of type `kind`.
fn elf(kind: u32) -> Vec<u8> {
    let mut out = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
    out.resize(64, 0);
    out[32..40].copy_from_slice(&64u64.to_le_bytes());
    out[54..56].copy_from_slice(&56u16.to_le_bytes());
    out[56..58].copy_from_slice(&1u16.to_le_bytes());
    let mut header = vec![0u8; 56];
    header[..4].copy_from_slice(&kind.to_le_bytes());
    out.extend(header);
    out
}

#[test]
fn an_interpreter_header_marks_a_dynamic_executable() {
    assert_eq!(has_interpreter(&elf(3)), Some(true));
    assert_eq!(has_interpreter(&elf(1)), Some(false));
    assert_eq!(has_interpreter(b"#!/bin/sh\n"), None);
}

#[test]
fn program_headers_beyond_the_address_space_are_not_an_interpreter() {
    let mut elf = vec![0u8; 64];
    elf[..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2;
    elf[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());
    elf[56..58].copy_from_slice(&3u16.to_le_bytes());
    assert_eq!(has_interpreter(&elf), None);
    elf[32..40].copy_from_slice(&(usize::MAX as u64 - 60).to_le_bytes());
    assert_eq!(has_interpreter(&elf), None);
}
