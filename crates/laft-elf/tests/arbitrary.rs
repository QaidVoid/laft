//! Arbitrary and corrupted input must be rejected or accepted, never panic.

use proptest::prelude::*;

const LIBRARY: &[u8] = include_bytes!("fixtures/libversioned.so");
const PROGRAM: &[u8] = include_bytes!("fixtures/program");

fn inspect_fully(data: &[u8]) {
    if let Ok(Some(elf)) = laft_elf::inspect(data) {
        let _ = elf.abi_digest();
        let _ = elf.is_static();
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn arbitrary_bytes_never_panic(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        inspect_fully(&data);
    }

    #[test]
    fn arbitrary_bytes_after_the_magic_never_panic(
        class in 0u8..4,
        rest in prop::collection::vec(any::<u8>(), 0..4096),
    ) {
        let mut data = b"\x7fELF".to_vec();
        data.push(class);
        data.extend(rest);
        inspect_fully(&data);
    }

    #[test]
    fn corrupted_fixtures_never_panic(
        program in any::<bool>(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..16),
    ) {
        let mut data = if program { PROGRAM } else { LIBRARY }.to_vec();
        for (index, byte) in edits {
            let at = index.index(data.len());
            data[at] = byte;
        }
        inspect_fully(&data);
    }
}
