//! Feeds arbitrary bytes to the ELF inspector and computes the ABI digest of
//! whatever it accepts.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(Some(elf)) = laft_elf::inspect(data) {
        let _ = elf.abi_digest();
        let _ = elf.is_static();
    }
});
