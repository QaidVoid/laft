use proptest::prelude::*;

use super::*;

fn header(path: &str, link: &str, size: u64) -> Header {
    Header {
        kind: Kind::File,
        path: path.into(),
        link: link.into(),
        mode: 0o444,
        uid: 31,
        gid: 65534,
        size,
    }
}

/// Reads back what `encode` wrote.
fn read_back(bytes: &[u8]) -> Header {
    assert_eq!(bytes.len() % BLOCK, 0);
    let first: &[u8; BLOCK] = bytes[..BLOCK].try_into().expect("one block");
    let mut header = decode(first).expect("header decodes");
    if header.kind == Kind::PaxHeader {
        let data = &bytes[BLOCK..BLOCK + header.size as usize];
        let pax = parse_pax(data).expect("pax header parses");
        let next = BLOCK + header.size as usize + padding(header.size);
        assert_eq!(bytes.len(), next + BLOCK);
        let block: &[u8; BLOCK] = bytes[next..].try_into().expect("one block");
        header = decode(block).expect("header decodes");
        header.path = pax.path.unwrap_or(header.path);
        header.link = pax.link.unwrap_or(header.link);
        header.size = pax.size.unwrap_or(header.size);
    } else {
        assert_eq!(bytes.len(), BLOCK);
    }
    header
}

fn encoded(header: &Header) -> Vec<u8> {
    let mut out = Vec::new();
    encode(header, &mut out);
    out
}

#[test]
fn short_headers_are_plain_ustar_with_fixed_fields() {
    let bytes = encoded(&header("usr/bin/rg", "", 5));
    assert_eq!(bytes.len(), BLOCK);
    assert_eq!(&bytes[100..108], b"0000444\0");
    assert_eq!(&bytes[136..148], b"00000000000\0", "mtime is zero");
    assert!(
        bytes[265..329].iter().all(|&b| b == 0),
        "owner names are empty"
    );
    assert_eq!(
        &bytes[329..345],
        b"0000000\x000000000\0",
        "device numbers are zero"
    );
    assert_eq!(read_back(&bytes), header("usr/bin/rg", "", 5));
}

#[test]
fn long_paths_links_and_sizes_use_only_pax_records() {
    let long = format!("usr/{}", "x".repeat(200));
    for h in [
        header(&long, "", 0),
        header("usr/l", &long, 0),
        header("usr/big", "", 1 << 40),
        header(&long[..104], &long[..101], 0),
    ] {
        let bytes = encoded(&h);
        assert_eq!(bytes[156], b'x');
        assert_eq!(read_back(&bytes), h);
    }
    let exactly_100 = &long[..100];
    assert_eq!(encoded(&header(exactly_100, exactly_100, 0)).len(), BLOCK);
}

#[test]
fn corrupt_or_foreign_headers_are_rejected() {
    let good = encoded(&header("usr/a", "", 0));
    let block = |f: &dyn Fn(&mut [u8; BLOCK])| {
        let mut b: [u8; BLOCK] = good[..].try_into().expect("one block");
        f(&mut b);
        b
    };
    let resum = |b: &mut [u8; BLOCK]| {
        let sum = checksum(b);
        b[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
    };
    assert!(
        decode(&block(&|b| b[0] = b'b')).is_err(),
        "checksum no longer matches"
    );
    let gnu = block(&|b| {
        b[257..265].copy_from_slice(b"ustar  \0");
        resum(b);
    });
    assert!(decode(&gnu).is_err(), "GNU headers are not POSIX");
    for flag in [b'3', b'4', b'6', b'7', b'g', b'L', b'K', 0] {
        let other = block(&|b| {
            b[156] = flag;
            resum(b);
        });
        assert!(decode(&other).is_err(), "type {flag}");
    }
    let base256 = block(&|b| {
        b[124] = 0x80;
        resum(b);
    });
    assert!(
        decode(&base256).is_err(),
        "base-256 sizes are a GNU extension"
    );
    let file_type_bits = block(&|b| {
        b[100..108].copy_from_slice(b"0100644\0");
        resum(b);
    });
    assert!(decode(&file_type_bits).is_err());
}

#[test]
fn ustar_prefix_is_joined_to_the_name() {
    let mut out = encoded(&header("rg", "", 0));
    out[345..352].copy_from_slice(b"usr/bin");
    let block: &mut [u8; BLOCK] = (&mut out[..]).try_into().expect("one block");
    let sum = checksum(block);
    block[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
    assert_eq!(decode(block).expect("valid header").path, b"usr/bin/rg");
}

#[test]
fn pax_records_other_than_path_linkpath_and_size_are_rejected() {
    let mut xattr = Vec::new();
    pax_record(&mut xattr, "SCHILY.xattr.security.capability", b"\x01");
    assert!(parse_pax(&xattr).is_err());
    let mut mtime = Vec::new();
    pax_record(&mut mtime, "mtime", b"1");
    assert!(parse_pax(&mtime).is_err());
    let mut twice = Vec::new();
    pax_record(&mut twice, "path", b"usr/a");
    pax_record(&mut twice, "path", b"usr/b");
    assert!(parse_pax(&twice).is_err());
    for malformed in [
        &b"5 path=a\n"[..],
        b"99 path=a\n",
        b"path=a\n",
        b"10 path=a\0\0",
        b"8 path\n",
    ] {
        assert!(parse_pax(malformed).is_err(), "{malformed:?}");
    }
}

proptest! {
    #[test]
    fn pax_record_lengths_count_themselves(value in proptest::collection::vec(any::<u8>(), 0..1200)) {
        let mut record = Vec::new();
        pax_record(&mut record, "path", &value);
        prop_assert_eq!(parse_pax(&record).expect("record parses").path, Some(value));
    }

    #[test]
    fn headers_round_trip(
        path in proptest::collection::vec(1u8.., 1..300),
        link in proptest::collection::vec(1u8.., 0..300),
        size in prop_oneof![0u64..1 << 20, (1u64 << 33) - 2..(1 << 34)],
        mode in 0u32..0o7777,
        uid in 0u32..0o7777777,
    ) {
        let h = Header { kind: Kind::Symlink, path, link, mode, uid, gid: uid, size };
        prop_assert_eq!(read_back(&encoded(&h)), h);
    }

    #[test]
    fn decoding_arbitrary_blocks_never_panics(bytes in proptest::collection::vec(any::<u8>(), BLOCK)) {
        let block: &[u8; BLOCK] = bytes[..].try_into().expect("one block");
        let _ = decode(block);
        let _ = parse_pax(&bytes);
    }
}
