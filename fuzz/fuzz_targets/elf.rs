//! Feeds arbitrary bytes to the ELF inspector and computes the ABI digest of
//! whatever it accepts, then rewrites the search paths of a copy and
//! inspects the result again.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(Some(elf)) = laft_elf::inspect(data) {
        let _ = elf.abi_digest();
        let _ = elf.is_static();
    }
    let mut copy = data.to_vec();
    if laft_elf::normalize_search_paths(&mut copy).unwrap_or(false) {
        let _ = laft_elf::inspect(&copy);
    }
});
