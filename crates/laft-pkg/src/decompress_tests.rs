use std::io::Write;

use super::*;

fn frame(data: &[u8], checksum: bool) -> Vec<u8> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).expect("encoder");
    encoder.include_checksum(checksum).expect("checksum flag");
    encoder.write_all(data).expect("compression");
    encoder.finish().expect("compression")
}

fn decompress(input: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    Decompressor::new(input)?.read_to_end(&mut out)?;
    Ok(out)
}

#[test]
fn concatenated_frames_decompress_in_order() {
    let big: Vec<u8> = (0..300_000u32).flat_map(|i| i.to_le_bytes()).collect();
    let mut input = frame(b"first ", true);
    input.extend(frame(&big, true));
    input.extend(frame(b" last", true));
    let mut expected = b"first ".to_vec();
    expected.extend(&big);
    expected.extend(b" last");
    assert_eq!(decompress(&input).expect("valid stream"), expected);
}

#[test]
fn every_frame_needs_a_content_checksum() {
    assert!(decompress(&frame(b"data", false)).is_err());
    let mut input = frame(b"data", true);
    input.extend(frame(b"data", false));
    assert!(
        decompress(&input).is_err(),
        "the second frame has no checksum"
    );
}

#[test]
fn a_wrong_checksum_is_detected() {
    let mut input = frame(b"data", true);
    let last = input.len() - 1;
    input[last] ^= 1;
    assert!(decompress(&input).is_err());
}

#[test]
fn empty_truncated_skippable_and_trailing_input_is_rejected() {
    assert!(decompress(b"").is_err());
    let input = frame(b"some data that compresses", true);
    assert!(decompress(&input[..input.len() - 2]).is_err());
    let mut skippable = vec![0x50, 0x2a, 0x4d, 0x18, 0, 0, 0, 0];
    skippable.extend(&input);
    assert!(decompress(&skippable).is_err());
    let mut trailing = input.clone();
    trailing.extend(b"garbage");
    assert!(decompress(&trailing).is_err());
    let mut short = input;
    short.extend(&[0x28, 0xb5]);
    assert!(decompress(&short).is_err());
}

fn frame_with_window_log(log: u32) -> Vec<u8> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).expect("encoder");
    encoder.include_checksum(true).expect("checksum flag");
    encoder.window_log(log).expect("window log");
    encoder.write_all(&[7; 4096]).expect("compression");
    encoder.finish().expect("compression")
}

#[test]
fn windows_above_128_mib_are_rejected() {
    assert!(decompress(&frame_with_window_log(MAX_WINDOW_LOG)).is_ok());
    let err =
        decompress(&frame_with_window_log(MAX_WINDOW_LOG + 1)).expect_err("window is too large");
    assert!(err.to_string().contains("too much memory"), "{err}");
}
