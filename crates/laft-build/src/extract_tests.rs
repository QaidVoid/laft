use std::io::Write;
use std::os::unix::fs::MetadataExt;

use laft_core::Sha256Hash;

use super::*;

/// An entry of a test archive. Paths are written into the header as they
/// are, so that archives can hold what a careful writer would refuse.
enum E<'a> {
    File(&'a str, u32, &'a [u8]),
    Dir(&'a str),
    Symlink(&'a str, &'a str),
    Link(&'a str, &'a str),
    Fifo(&'a str),
}

fn tar(entries: &[E<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        let (path, kind, mode, data, link): (&str, tar::EntryType, u32, &[u8], &str) = match entry {
            E::File(p, m, d) => (p, tar::EntryType::Regular, *m, d, ""),
            E::Dir(p) => (p, tar::EntryType::Directory, 0o755, b"", ""),
            E::Symlink(p, l) => (p, tar::EntryType::Symlink, 0o777, b"", l),
            E::Link(p, l) => (p, tar::EntryType::Link, 0o644, b"", l),
            E::Fifo(p) => (p, tar::EntryType::Fifo, 0o644, b"", ""),
        };
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_size(data.len() as u64);
        header.set_mtime(0);
        let gnu = header.as_gnu_mut().expect("a GNU header");
        gnu.name[..path.len()].copy_from_slice(path.as_bytes());
        gnu.linkname[..link.len()].copy_from_slice(link.as_bytes());
        header.set_cksum();
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

fn source(file: &str, dest: &str, strip: Option<u32>) -> Source {
    Source {
        label: "main".into(),
        fetch: Fetch::Url {
            urls: vec![format!("https://example.org/{file}")],
            sha256: Sha256Hash::from_bytes([0; 32]),
            file: file.into(),
            extract: Format::of(file).is_some(),
        },
        dest: dest.into(),
        strip_components: strip,
    }
}

struct Tree {
    dir: laft_test::TempDir,
}

impl Tree {
    fn new() -> Self {
        let dir = laft_test::tempdir();
        for d in ["src", "scratch", "files"] {
            fs::create_dir(dir.path().join(d)).expect("mkdir");
        }
        Self { dir }
    }

    fn unpack(&self, source: &Source, bytes: &[u8]) -> Result<Vec<String>, SourceError> {
        let name = match &source.fetch {
            Fetch::Url { file, .. } => file.clone(),
        };
        let file = self.dir.path().join("files").join(name);
        fs::write(&file, bytes).expect("write");
        unpack(
            source,
            &file,
            &self.dir.path().join("src"),
            &self.dir.path().join("scratch"),
        )
    }

    /// Every path below `src`, with `/` after directories and `-> target`
    /// after links.
    fn listing(&self) -> Vec<String> {
        let root = self.dir.path().join("src");
        let mut out = Vec::new();
        let mut todo = vec![root.clone()];
        while let Some(dir) = todo.pop() {
            for entry in fs::read_dir(&dir).expect("readable") {
                let path = entry.expect("readable").path();
                let rel = path
                    .strip_prefix(&root)
                    .expect("below")
                    .display()
                    .to_string();
                let meta = fs::symlink_metadata(&path).expect("readable");
                if meta.file_type().is_symlink() {
                    let target = fs::read_link(&path).expect("a link");
                    out.push(format!("{rel} -> {}", target.display()));
                } else if meta.is_dir() {
                    out.push(format!("{rel}/"));
                    todo.push(path);
                } else {
                    out.push(rel);
                }
            }
        }
        out.sort();
        out
    }

    fn mode(&self, rel: &str) -> u32 {
        let path = self.dir.path().join("src").join(rel);
        fs::symlink_metadata(path).expect("exists").mode() & 0o7777
    }
}

fn sample() -> Vec<u8> {
    tar(&[
        E::Dir("foo-1.0/"),
        E::File("foo-1.0/README", 0o600, b"read me"),
        E::File("foo-1.0/bin/run", 0o700, b"#!/bin/sh\n"),
        E::Symlink("foo-1.0/bin/start", "run"),
        E::Link("foo-1.0/README.copy", "foo-1.0/README"),
    ])
}

#[test]
fn a_single_top_directory_is_stripped_and_modes_are_normalized() {
    let tree = Tree::new();
    let warnings = tree
        .unpack(&source("foo-1.0.tar", "", None), &sample())
        .expect("a safe archive");
    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(
        tree.listing(),
        [
            "README",
            "README.copy",
            "bin/",
            "bin/run",
            "bin/start -> run"
        ]
    );
    assert_eq!(tree.mode("README"), 0o644);
    assert_eq!(tree.mode("bin/run"), 0o744);
    assert_eq!(tree.mode("bin"), 0o755);
    assert_eq!(
        fs::read(tree.dir.path().join("src/README.copy")).expect("linked"),
        b"read me"
    );
    assert_eq!(
        fs::read_dir(tree.dir.path().join("scratch"))
            .expect("readable")
            .count(),
        0
    );
}

#[test]
fn explicit_strip_counts_and_destinations_are_honored() {
    let tree = Tree::new();
    tree.unpack(&source("foo-1.0.tar", "", Some(0)), &sample())
        .expect("a safe archive");
    assert!(tree.listing().contains(&"foo-1.0/bin/run".to_owned()));

    let tree = Tree::new();
    tree.unpack(&source("foo-1.0.tar", "docs", Some(2)), &sample())
        .expect("a safe archive");
    assert_eq!(tree.listing(), ["docs/", "docs/run", "docs/start -> run"]);

    let tree = Tree::new();
    let two_tops = tar(&[E::File("a", 0o644, b"a"), E::File("b/c", 0o644, b"c")]);
    tree.unpack(&source("x.tar", "", None), &two_tops)
        .expect("a safe archive");
    assert_eq!(tree.listing(), ["a", "b/", "b/c"]);
}

#[test]
fn files_that_are_not_extracted_are_copied() {
    let tree = Tree::new();
    tree.unpack(&source("foo.patch", "extra", None), b"diff")
        .expect("copied");
    assert_eq!(tree.listing(), ["extra/", "extra/foo.patch"]);
    assert_eq!(tree.mode("extra/foo.patch"), 0o644);
}

#[test]
fn copied_files_are_never_written_through_a_link() {
    let tree = Tree::new();
    tree.unpack(
        &source("one.tar", "", Some(0)),
        &tar(&[
            E::Dir("real"),
            E::Symlink("extra", "real"),
            E::Symlink("real/foo.patch", "x"),
        ]),
    )
    .expect("stays inside");
    let error = tree
        .unpack(&source("foo.patch", "extra", None), b"diff")
        .expect_err("the destination is a link");
    assert_eq!(error.code(), "B021");
    assert_eq!(
        tree.listing(),
        ["extra -> real", "real/", "real/foo.patch -> x"]
    );

    tree.unpack(&source("foo.patch", "real", None), b"diff")
        .expect("the link at the file name is replaced");
    assert_eq!(tree.listing(), ["extra -> real", "real/", "real/foo.patch"]);
    assert!(!tree.dir.path().join("src/real/x").exists());
}

#[test]
fn entries_that_leave_the_destination_are_refused() {
    let cases: [(&[E<'_>], &str); 7] = [
        (
            &[E::File("../evil", 0o644, b"x")],
            "archive entry ../evil escapes the destination",
        ),
        (
            &[E::File("a/../../evil", 0o644, b"x")],
            "archive entry a/../../evil escapes the destination",
        ),
        (
            &[E::File("/etc/passwd", 0o644, b"x")],
            "archive entry /etc/passwd has an absolute path",
        ),
        (
            &[E::Symlink("a/l", "../../etc")],
            "archive entry a/l links to ../../etc, outside the destination",
        ),
        (
            &[E::Symlink("l", "/etc")],
            "archive entry l links to /etc, outside the destination",
        ),
        (
            &[E::Symlink("d", "."), E::File("d/x", 0o644, b"x")],
            "archive entry d/x would be written through a symbolic link",
        ),
        (
            &[E::Link("x", "elsewhere")],
            "archive entry x is a hard link to elsewhere, which is not an earlier file of the archive",
        ),
    ];
    for (entries, message) in cases {
        let tree = Tree::new();
        let error = tree
            .unpack(&source("evil.tar", "", Some(0)), &tar(entries))
            .expect_err(message);
        assert_eq!(error.code(), "B021");
        assert!(error.to_string().ends_with(message), "{error}");
        assert_eq!(tree.listing(), Vec::<String>::new(), "{message}");
    }

    let tree = Tree::new();
    let out_of_top = tar(&[
        E::File("foo-1.0/a", 0o644, b"a"),
        E::Symlink("foo-1.0/l", "../foo-1.0/a"),
    ]);
    let error = tree
        .unpack(&source("foo.tar", "", None), &out_of_top)
        .expect_err("the link would leave the stripped directory");
    assert_eq!(error.code(), "B021");
}

#[test]
fn device_nodes_and_fifos_are_skipped_with_a_warning() {
    let tree = Tree::new();
    let warnings = tree
        .unpack(
            &source("x.tar", "", Some(0)),
            &tar(&[E::Fifo("pipe"), E::File("a", 0o644, b"a")]),
        )
        .expect("skipped");
    assert_eq!(warnings, ["skipped pipe, which is a Fifo entry"]);
    assert_eq!(tree.listing(), ["a"]);
}

#[test]
fn every_archive_format_extracts_the_same_tree() {
    let plain = sample();
    let gz = {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(&plain).expect("compress");
        e.finish().expect("compress")
    };
    let bz2 = {
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        e.write_all(&plain).expect("compress");
        e.finish().expect("compress")
    };
    let xz = {
        let mut e = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::default())
            .expect("an encoder");
        e.write_all(&plain).expect("compress");
        e.finish().expect("compress")
    };
    let lz = {
        let mut e = lzma_rust2::LzipWriter::new(Vec::new(), lzma_rust2::LzipOptions::default());
        e.write_all(&plain).expect("compress");
        e.finish().expect("compress")
    };
    let zst = zstd::encode_all(plain.as_slice(), 1).expect("compress");
    let zip = {
        let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let options = |mode| {
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(mode)
        };
        w.add_directory("foo-1.0/", options(0o755)).expect("zip");
        w.start_file("foo-1.0/README", options(0o600)).expect("zip");
        w.write_all(b"read me").expect("zip");
        w.start_file("foo-1.0/README.copy", options(0o600))
            .expect("zip");
        w.write_all(b"read me").expect("zip");
        w.start_file("foo-1.0/bin/run", options(0o700))
            .expect("zip");
        w.write_all(b"#!/bin/sh\n").expect("zip");
        w.add_symlink("foo-1.0/bin/start", "run", options(0o777))
            .expect("zip");
        w.finish().expect("zip").into_inner()
    };
    for (file, bytes) in [
        ("foo.tar.gz", gz.clone()),
        ("foo.tgz", gz),
        ("foo.tar.bz2", bz2),
        ("foo.tar.xz", xz.clone()),
        ("foo.txz", xz),
        ("foo.tar.lz", lz),
        ("foo.tar.zst", zst),
        ("foo.zip", zip),
    ] {
        let tree = Tree::new();
        tree.unpack(&source(file, "", None), &bytes).expect(file);
        assert_eq!(
            tree.listing(),
            [
                "README",
                "README.copy",
                "bin/",
                "bin/run",
                "bin/start -> run"
            ],
            "{file}"
        );
        assert_eq!(tree.mode("bin/run"), 0o744, "{file}");
    }
    let tree = Tree::new();
    let error = tree
        .unpack(&source("foo.tar.gz", "", None), b"not gzip")
        .expect_err("garbage");
    assert_eq!(error.code(), "B024");
}

#[test]
fn the_source_label_never_names_a_path() {
    let tree = Tree::new();
    let victim = tree.dir.path().join("victim");
    fs::create_dir(&victim).expect("mkdir");
    fs::write(victim.join("keep"), b"keep").expect("write");
    let mut source = source("x.tar", "", Some(0));
    source.label = "/../../victim".into();
    tree.unpack(&source, &tar(&[E::File("a", 0o644, b"a")]))
        .expect("a safe archive");
    assert_eq!(tree.listing(), ["a"]);
    assert_eq!(fs::read(victim.join("keep")).expect("kept"), b"keep");
    assert_eq!(
        fs::read_dir(tree.dir.path().join("scratch"))
            .expect("readable")
            .count(),
        0
    );
}

#[test]
fn destinations_outside_the_source_tree_are_refused() {
    for dest in ["../up", "/abs", "a/../.."] {
        let tree = Tree::new();
        let error = tree
            .unpack(
                &source("x.tar", dest, Some(0)),
                &tar(&[E::File("a", 0o644, b"a")]),
            )
            .expect_err(dest);
        assert_eq!(error.code(), "B023", "{dest}");
        let error = tree
            .unpack(&source("x.patch", dest, None), b"diff")
            .expect_err(dest);
        assert_eq!(error.code(), "B023", "{dest}");
        assert_eq!(tree.listing(), Vec::<String>::new(), "{dest}");
        assert!(!tree.dir.path().join("up").exists(), "{dest}");
    }
}

#[test]
fn link_chains_that_leave_the_destination_are_refused() {
    let cases: [&[E<'_>]; 5] = [
        // Each link stays inside on its own, but `l` passes through `d/a`.
        &[E::Symlink("d/a", ".."), E::Symlink("l", "d/a/../x")],
        // The link that changes where `l` leads comes after it.
        &[E::Symlink("l", "d/a/../x"), E::Symlink("d/a", "..")],
        // Three links: `l3` passes through `s2/l2`, which passes through
        // `s1/l1`.
        &[
            E::Symlink("s1/l1", ".."),
            E::Symlink("s2/l2", "../s1/l1"),
            E::Symlink("l3", "s2/l2/../x"),
        ],
        &[
            E::Symlink("l3", "s2/l2/../x"),
            E::Symlink("s2/l2", "../s1/l1"),
            E::Symlink("s1/l1", ".."),
        ],
        // A link that never resolves.
        &[E::Symlink("a", "b"), E::Symlink("b", "a")],
    ];
    for entries in cases {
        let tree = Tree::new();
        let error = tree
            .unpack(&source("evil.tar", "", Some(0)), &tar(entries))
            .expect_err("an escaping chain");
        assert_eq!(error.code(), "B021", "{error}");
        assert_eq!(tree.listing(), Vec::<String>::new(), "{error}");
    }

    let tree = Tree::new();
    let inside = tar(&[
        E::Dir("d/e/"),
        E::Symlink("d/a", "e"),
        E::Symlink("s/l", "../d/a/../e"),
        E::Symlink("dangling", "missing/../x"),
    ]);
    tree.unpack(&source("ok.tar", "", Some(0)), &inside)
        .expect("every link stays inside");
}

#[test]
fn a_link_cannot_leave_through_a_link_of_an_earlier_source() {
    let tree = Tree::new();
    tree.unpack(
        &source("one.tar", "", Some(0)),
        &tar(&[E::Symlink("d", ".")]),
    )
    .expect("stays inside");
    let error = tree
        .unpack(
            &source("two.tar", "", Some(0)),
            &tar(&[E::Symlink("l", "d/../x")]),
        )
        .expect_err("leaves through the earlier link");
    assert_eq!(error.code(), "B021");
}

#[test]
fn hard_links_point_only_at_earlier_files() {
    let cases: [(&[E<'_>], &str); 4] = [
        (
            &[E::Symlink("s", "a"), E::Link("h", "s")],
            "archive entry h is a hard link to s, which is not an earlier file of the archive",
        ),
        (
            &[
                E::File("d/f", 0o644, b"f"),
                E::Symlink("d/f", "../.."),
                E::Link("h", "d/f"),
            ],
            "archive entry h is a hard link to d/f, which is not an earlier file of the archive",
        ),
        (
            &[E::File("f", 0o644, b"f"), E::Link("h", "../f")],
            "archive entry ../f escapes the destination",
        ),
        (
            &[E::File("f", 0o644, b"f"), E::Link("f", "f")],
            "archive entry f is a hard link to f, which is not an earlier file of the archive",
        ),
    ];
    for (entries, message) in cases {
        let tree = Tree::new();
        let error = tree
            .unpack(&source("evil.tar", "", Some(0)), &tar(entries))
            .expect_err(message);
        assert_eq!(error.code(), "B021");
        assert!(error.to_string().ends_with(message), "{error}");
    }

    let tree = Tree::new();
    tree.unpack(
        &source("ok.tar", "", Some(0)),
        &tar(&[E::File("d/f", 0o600, b"f"), E::Link("e/h", "d/f")]),
    )
    .expect("a link to an earlier file");
    assert_eq!(
        fs::read(tree.dir.path().join("src/e/h")).expect("linked"),
        b"f"
    );
}

#[test]
fn an_oversized_zip_link_target_is_refused() {
    let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    w.add_symlink("l", "a".repeat(5000), options).expect("zip");
    let zip = w.finish().expect("zip").into_inner();
    let tree = Tree::new();
    let error = tree
        .unpack(&source("x.zip", "", Some(0)), &zip)
        .expect_err("too long");
    assert_eq!(error.code(), "B024");
}

#[test]
fn files_keep_the_modification_times_of_the_archive() {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, mtime) in [
        ("foo-1.0/configure", 2_000_000),
        ("foo-1.0/configure.ac", 1_000_000),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_mtime(mtime);
        header.set_cksum();
        builder.append_data(&mut header, name, &b"x"[..]).unwrap();
    }
    let archive = builder.into_inner().unwrap();
    let tree = Tree::new();
    tree.unpack(&source("foo-1.0.tar", "", None), &archive)
        .expect("a safe archive");
    let mtime = |name: &str| {
        fs::metadata(tree.dir.path().join("src").join(name))
            .unwrap()
            .mtime()
    };
    assert_eq!(mtime("configure"), 2_000_000);
    assert_eq!(mtime("configure.ac"), 1_000_000);
}

#[test]
fn zip_files_keep_their_modification_times() {
    let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    for (name, (hour, minute)) in [("configure", (12, 30)), ("configure.ac", (8, 0))] {
        let time = zip::DateTime::from_date_and_time(2020, 2, 29, hour, minute, 10).unwrap();
        let options = zip::write::SimpleFileOptions::default().last_modified_time(time);
        w.start_file(name, options).expect("zip");
        w.write_all(b"x").expect("zip");
    }
    let zip = w.finish().expect("zip").into_inner();
    let tree = Tree::new();
    tree.unpack(&source("x.zip", "", Some(0)), &zip)
        .expect("a safe archive");
    let mtime = |name: &str| {
        fs::metadata(tree.dir.path().join("src").join(name))
            .unwrap()
            .mtime()
    };
    assert_eq!(mtime("configure"), 1_582_979_410);
    assert_eq!(mtime("configure.ac"), 1_582_963_210);
}
