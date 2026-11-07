use std::io::Read;

use super::*;
use crate::test_support::write_tree;

fn tree(dir: &Path, modes: u32) -> PathBuf {
    let root = dir.join("vendor");
    let long = format!("deep/{}/file.rs", "x".repeat(150));
    write_tree(
        &root,
        &[
            ("b/lib.rs", b"lib"),
            ("a/build.sh", b"#!/bin/sh"),
            (&long, b"long"),
        ],
    );
    fsx::set_mode(&root.join("a/build.sh"), modes).unwrap();
    std::os::unix::fs::symlink("lib.rs", root.join("b/alias.rs")).unwrap();
    root
}

#[test]
fn the_same_tree_always_gives_the_same_tarball() {
    let (one, two) = (laft_test::tempdir(), laft_test::tempdir());
    let (a, b) = (tree(one.path(), 0o700), tree(two.path(), 0o755));
    filetime_touch(&a.join("b/lib.rs"));
    normalized_tarball(&a, &one.path().join("v.tar")).unwrap();
    normalized_tarball(&b, &two.path().join("v.tar")).unwrap();
    assert_eq!(
        fs::read(one.path().join("v.tar")).unwrap(),
        fs::read(two.path().join("v.tar")).unwrap()
    );
}

/// Moves the modification time of `path` far from the other files.
fn filetime_touch(path: &Path) {
    let file = File::options().append(true).open(path).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000))
        .unwrap();
}

#[test]
fn vcs_directories_are_not_packed() {
    let dir = laft_test::tempdir();
    let root = tree(dir.path(), 0o700);
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main").unwrap();
    fs::create_dir_all(root.join(".jj/repo")).unwrap();
    fs::write(root.join(".jj/repo/store"), b"store").unwrap();
    let file = dir.path().join("v.tar");
    normalized_tarball(&root, &file).unwrap();
    let mut archive = tar::Archive::new(File::open(&file).unwrap());
    let mut seen = Vec::new();
    for entry in archive.entries().unwrap() {
        let path = entry
            .unwrap()
            .path()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        seen.push(path);
    }
    assert!(
        !seen
            .iter()
            .any(|path| path.contains(".git") || path.contains(".jj")),
        "{seen:?}"
    );
    assert!(seen.contains(&"b/lib.rs".to_owned()), "{seen:?}");
}

#[test]
fn entries_are_sorted_and_normalized() {
    let dir = laft_test::tempdir();
    let root = tree(dir.path(), 0o700);
    let file = dir.path().join("v.tar");
    normalized_tarball(&root, &file).unwrap();
    let mut archive = tar::Archive::new(File::open(&file).unwrap());
    let mut seen = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let header = entry.header().clone();
        assert_eq!(header.mtime().unwrap(), SOURCE_DATE_EPOCH);
        assert_eq!((header.uid().unwrap(), header.gid().unwrap()), (0, 0));
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        seen.push((path, header.mode().unwrap()));
        let mut sink = Vec::new();
        entry.read_to_end(&mut sink).unwrap();
    }
    let long = format!("deep/{}/file.rs", "x".repeat(150));
    assert_eq!(
        seen,
        [
            ("a/".to_owned(), 0o755),
            ("a/build.sh".to_owned(), 0o755),
            ("b/".to_owned(), 0o755),
            ("b/alias.rs".to_owned(), 0o777),
            ("b/lib.rs".to_owned(), 0o644),
            ("deep/".to_owned(), 0o755),
            (format!("deep/{}/", "x".repeat(150)), 0o755),
            (long, 0o644),
        ]
    );
}

/// A symlink whose target leaves the tree is refused when packed, like
/// the unpacker refuses it: the archive must never carry one.
#[test]
fn a_symlink_leaving_the_tree_is_refused_when_packed() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("vendor");
    write_tree(&root, &[("a/lib.rs", b"lib")]);
    std::os::unix::fs::symlink("../outside", root.join("a/escape.rs")).unwrap();
    let file = dir.path().join("v.tar");
    let e = normalized_tarball(&root, &file).unwrap_err();
    assert_eq!(e.code(), Some("B024"));
    assert!(e.to_string().contains("../outside"), "{}", e);
}

/// A symlink target too long for the archive's link name field is
/// refused with its own problem: no pax `linkpath` record is written, so
/// the archive could not carry it faithfully.
#[test]
fn a_symlink_target_too_long_for_the_archive_is_refused_when_packed() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("vendor");
    write_tree(&root, &[("a/lib.rs", b"lib")]);
    let target = format!("{}.rs", "long".repeat(40));
    assert!(target.len() > 100);
    std::os::unix::fs::symlink(&target, root.join("a/alias.rs")).unwrap();
    let file = dir.path().join("v.tar");
    let e = normalized_tarball(&root, &file).unwrap_err();
    assert_eq!(e.code(), Some("B024"));
    assert!(e.to_string().contains("bytes long"), "{}", e);
}

/// The unpacker refuses an archive with more than the entry bound.
#[test]
fn unpack_refuses_more_than_the_entry_bound() {
    let dir = laft_test::tempdir();
    let tarball = dir.path().join("many.tar.zst");
    {
        let encoder =
            zstd::stream::write::Encoder::new(File::create(&tarball).unwrap(), 3).unwrap();
        let mut builder = tar::Builder::new(encoder);
        for i in 0..UNPACK_MAX_ENTRIES + 1 {
            let mut header = tar::Header::new_ustar();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(0);
            header.set_path(format!("f{i}")).unwrap();
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
        }
        builder
            .into_inner()
            .and_then(|encoder| encoder.finish())
            .unwrap();
    }
    let e = unpack_normalized_tarball(&tarball, &dir.path().join("out")).unwrap_err();
    assert_eq!(e.code(), Some("B024"));
    assert!(e.to_string().contains("entries"), "{}", e);
}

/// The unpacker refuses an archive that decompresses past the size
/// bound, so a small archive cannot expand into the disk.
#[test]
fn unpack_refuses_more_than_the_decompressed_bound() {
    let dir = laft_test::tempdir();
    let tarball = dir.path().join("big.tar.zst");
    {
        let encoder =
            zstd::stream::write::Encoder::new(File::create(&tarball).unwrap(), 3).unwrap();
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(UNPACK_MAX_BYTES + 1);
        header.set_path("big").unwrap();
        header.set_cksum();
        builder
            .append(&header, std::io::repeat(0).take(UNPACK_MAX_BYTES + 1))
            .unwrap();
        builder
            .into_inner()
            .and_then(|encoder| encoder.finish())
            .unwrap();
    }
    let e = unpack_normalized_tarball(&tarball, &dir.path().join("out")).unwrap_err();
    assert_eq!(e.code(), Some("B024"));
    assert!(e.to_string().contains("bytes"), "{}", e);
}

#[test]
fn a_normalized_tarball_unpacks_to_the_same_tree() {
    let dir = laft_test::tempdir();
    let root = tree(dir.path(), 0o755);
    let raw = dir.path().join("v.tar");
    normalized_tarball(&root, &raw).unwrap();
    let tarball = dir.path().join("v.tar.zst");
    let mut bytes = Vec::new();
    {
        let mut writer = zstd::stream::write::Encoder::new(&mut bytes, 3).unwrap();
        std::io::copy(&mut File::open(&raw).unwrap(), &mut writer).unwrap();
        writer.finish().unwrap();
    }
    std::fs::write(&tarball, &bytes).unwrap();
    let dest = dir.path().join("out");
    unpack_normalized_tarball(&tarball, &dest).unwrap();
    let alias = fs::read_link(dest.join("b/alias.rs")).unwrap();
    assert_eq!(alias.to_string_lossy(), "lib.rs");
    assert_eq!(fs::read_to_string(dest.join("b/lib.rs")).unwrap(), "lib");
    let mode = crate::test_support::mode(&dest.join("a/build.sh"));
    assert_eq!(mode & 0o777, 0o755, "the executable bit survives");
    assert_eq!(
        fs::read_to_string(dest.join("a/build.sh")).unwrap(),
        "#!/bin/sh"
    );
    assert!(
        dest.join(format!("deep/{}/file.rs", "x".repeat(150)))
            .is_file()
    );
}

#[test]
fn an_entry_outside_the_root_is_refused() {
    let dir = laft_test::tempdir();
    let tarball = pack(
        dir.path(),
        &[
            ("recipes/a/package.stav", b"package a {}"),
            ("../escape", b"no"),
        ],
    );
    let dest = dir.path().join("out");
    let error = unpack_normalized_tarball(&tarball, &dest).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("cannot be read") || message.contains("../escape"),
        "{message}"
    );
    assert!(!dir.path().join("escape").exists(), "nothing escapes");
}

#[test]
fn a_control_character_in_a_path_is_refused() {
    let dir = laft_test::tempdir();
    let tarball = pack(dir.path(), &[("recipes/a/\u{7}file.rs", b"no")]);
    let dest = dir.path().join("out");
    let error = unpack_normalized_tarball(&tarball, &dest).unwrap_err();
    assert!(
        error.to_string().contains("not a plain relative path"),
        "{error}"
    );
}

#[test]
fn a_file_does_not_follow_a_symlink_planted_at_its_path() {
    let dir = laft_test::tempdir();
    let tarball = pack(dir.path(), &[("recipes/a/file.rs", b"real")]);
    let dest = dir.path().join("out");
    fs::create_dir_all(dest.join("recipes/a")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("victim"), dest.join("recipes/a/file.rs")).unwrap();
    unpack_normalized_tarball(&tarball, &dest).expect("the file is written");
    assert!(
        fs::symlink_metadata(dest.join("recipes/a/file.rs"))
            .unwrap()
            .file_type()
            .is_file(),
        "the planted symlink is replaced"
    );
    assert!(
        !dir.path().join("victim").exists(),
        "the symlink is not followed"
    );
}

/// A tar.zst of `files` at `dir/v.tar.zst`.
fn pack(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
    let raw = dir.join("plain.tar");
    {
        let file = File::create(&raw).unwrap();
        let mut builder = tar::Builder::new(file);
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            if *name == "../escape" {
                // `set_path` refuses paths that leave the root, so the
                // name goes into the header field directly, like a
                // hand-written archive would hold it.
                let field = &mut header.as_gnu_mut().unwrap().name;
                field[..name.len()].copy_from_slice(name.as_bytes());
                builder.append(&header, *content).unwrap();
            } else {
                builder.append_data(&mut header, name, *content).unwrap();
            }
        }
        builder.into_inner().unwrap().sync_all().unwrap();
    }
    let mut bytes = Vec::new();
    {
        let mut writer = zstd::stream::write::Encoder::new(&mut bytes, 3).unwrap();
        std::io::copy(&mut File::open(&raw).unwrap(), &mut writer).unwrap();
        writer.finish().unwrap();
    }
    let packed = dir.join("v.tar.zst");
    std::fs::write(&packed, &bytes).unwrap();
    packed
}

/// Packing refuses an entry path the unpacker refuses, so a vendored
/// tarball never pins a tree its unpacking cannot read back.
#[test]
fn packing_refuses_entry_paths_the_unpacker_refuses() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("src");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("good"), b"x").unwrap();
    fs::write(root.join("a\nb"), b"x").unwrap();
    let error = normalized_tarball(&root, &dir.path().join("v.tar"))
        .expect_err("the control-character path is refused");
    assert!(error.to_string().contains("B024"), "{error}");
}
