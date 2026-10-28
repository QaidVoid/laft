//! Feeds arbitrary bytes to the package reader and unpacks whatever it
//! accepts into a temporary directory.
//!
//! The first input byte picks the layer to fuzz. When it is even, the rest of
//! the input is the package file itself, which exercises the zstd framing.
//! When it is odd, the rest is compressed first, so that mutations reach the
//! tar and JSON parsing behind the compression.

#![no_main]

use std::io::Write;

use libfuzzer_sys::fuzz_target;

fn compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1)
        .expect("a level 1 encoder can always be created");
    encoder
        .include_checksum(true)
        .expect("the checksum flag is a valid parameter");
    encoder
        .write_all(data)
        .expect("compressing into memory cannot fail");
    encoder.finish().expect("compressing into memory cannot fail")
}

fuzz_target!(|data: &[u8]| {
    let Some((&layer, rest)) = data.split_first() else {
        return;
    };
    let compressed;
    let input = if layer % 2 == 0 {
        rest
    } else {
        compressed = compress(rest);
        &compressed
    };
    let Ok(reader) = laft_pkg::Reader::new(input) else {
        return;
    };
    let dir = tempfile::tempdir().expect("a temporary directory can be created");
    let _ = reader.unpack(dir.path());
});
