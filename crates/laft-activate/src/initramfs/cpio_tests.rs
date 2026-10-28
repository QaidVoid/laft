use super::*;

/// One entry read back from an archive: name, mode, nlink, ino, rdev and
/// data.
type Read = (String, u32, u32, u32, (u32, u32), Vec<u8>);

fn field(bytes: &[u8], index: usize) -> u32 {
    let start = 6 + index * 8;
    u32::from_str_radix(std::str::from_utf8(&bytes[start..start + 8]).unwrap(), 16).unwrap()
}

fn align(offset: usize) -> usize {
    offset.div_ceil(4) * 4
}

/// Parses `bytes` back, checking the magic and the alignment of every
/// header, up to and including the trailer.
fn parse(bytes: &[u8]) -> Vec<Read> {
    let mut entries = Vec::new();
    let mut at = 0;
    loop {
        assert_eq!(at % 4, 0);
        let header = &bytes[at..];
        assert_eq!(&header[..6], b"070701");
        let size = field(header, 6) as usize;
        let name_len = field(header, 11) as usize;
        let name = std::str::from_utf8(&header[110..110 + name_len - 1])
            .unwrap()
            .to_owned();
        assert_eq!(header[110 + name_len - 1], 0);
        let data_at = align(at + 110 + name_len);
        let data = bytes[data_at..data_at + size].to_vec();
        entries.push((
            name.clone(),
            field(header, 1),
            field(header, 4),
            field(header, 0),
            (field(header, 9), field(header, 10)),
            data,
        ));
        at = align(data_at + size);
        if name == "TRAILER!!!" {
            assert_eq!(at, bytes.len());
            return entries;
        }
    }
}

#[test]
fn entries_are_sorted_with_parents_and_fixed_metadata() {
    let mut archive = Archive::default();
    archive.add(
        "/lib/modules/6.1/kernel/a.ko",
        0o644,
        Kind::File(b"module".to_vec()),
    );
    archive.add("/init", 0o755, Kind::File(b"init!".to_vec()));
    archive.add("/bin/sh", 0o777, Kind::Symlink("busybox".into()));
    archive.add("/dev/console", 0o600, Kind::CharDevice(5, 1));
    archive.add("/proc", 0o755, Kind::Dir);
    let bytes = archive.to_bytes();
    let entries = parse(&bytes);
    let names: Vec<&str> = entries.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(
        names,
        [
            "bin",
            "bin/sh",
            "dev",
            "dev/console",
            "init",
            "lib",
            "lib/modules",
            "lib/modules/6.1",
            "lib/modules/6.1/kernel",
            "lib/modules/6.1/kernel/a.ko",
            "proc",
            "TRAILER!!!",
        ]
    );
    for (index, (name, mode, nlink, ino, rdev, data)) in entries.iter().enumerate() {
        match name.as_str() {
            "TRAILER!!!" => assert_eq!(*ino, 0),
            _ => assert_eq!(*ino as usize, index + 1),
        }
        match name.as_str() {
            "bin/sh" => assert_eq!((*mode, data.as_slice()), (0o120777, &b"busybox"[..])),
            "dev/console" => assert_eq!((*mode, *rdev), (0o020600, (5, 1))),
            "init" => assert_eq!(
                (*mode, *nlink, data.as_slice()),
                (0o100755, 1, &b"init!"[..])
            ),
            "lib" | "proc" => assert_eq!((*mode, *nlink), (0o040755, 2)),
            _ => {}
        }
    }
    let mtime_and_owner = |at: usize| {
        (
            field(&bytes[at..], 2),
            field(&bytes[at..], 3),
            field(&bytes[at..], 5),
        )
    };
    assert_eq!(mtime_and_owner(0), (0, 0, 0));
}

#[test]
fn the_same_entries_give_the_same_bytes() {
    let build = |order: &[(&str, &[u8])]| {
        let mut archive = Archive::default();
        for (path, data) in order {
            archive.add(path, 0o644, Kind::File(data.to_vec()));
        }
        archive.to_bytes()
    };
    let a = build(&[("a/x", b"1"), ("b", b"22"), ("a/y", b"333")]);
    let b = build(&[("a/y", b"333"), ("b", b"22"), ("a/x", b"1")]);
    assert_eq!(a, b);
}

#[test]
fn an_empty_archive_holds_only_the_trailer() {
    let entries = parse(&Archive::default().to_bytes());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "TRAILER!!!");
}
