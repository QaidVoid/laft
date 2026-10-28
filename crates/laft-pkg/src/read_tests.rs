use std::os::unix::fs::MetadataExt;

use proptest::prelude::*;

use super::*;
use crate::test_support::{
    END, Member, assemble, build, compress, decompress, members, sample_payload,
};
use crate::{ErrorKind, scan};

/// A sample package as its metadata, manifest and tar members.
fn sample() -> (tempfile::TempDir, Vec<u8>, Vec<Member>) {
    let root = tempfile::tempdir().expect("temporary directory");
    sample_payload(root.path());
    let (_, _, bytes) = build(root.path());
    let members = members(&decompress(&bytes));
    (root, bytes, members)
}

fn package_from(members: &[Member], tail: &[u8]) -> Vec<u8> {
    let mut tar = assemble(members);
    tar.extend_from_slice(tail);
    compress(&tar)
}

fn unpack(bytes: &[u8]) -> Result<tempfile::TempDir, Error> {
    let out = tempfile::tempdir().expect("temporary directory");
    Reader::new(bytes)?.unpack(out.path())?;
    Ok(out)
}

fn member<'a>(members: &'a mut [Member], path: &str) -> &'a mut Member {
    members
        .iter_mut()
        .find(|m| m.header.path == path.as_bytes())
        .expect("member exists")
}

/// A change to one member.
type Tamper = fn(&mut Member);

/// Unpacks a package whose members were changed by `tamper`.
fn tampered(tamper: impl FnOnce(&mut Vec<Member>)) -> Error {
    let (_, _, mut members) = sample();
    tamper(&mut members);
    unpack(&package_from(&members, &END)).expect_err("tampered package is rejected")
}

#[test]
fn unpacking_restores_the_payload() {
    let (root, bytes, _) = sample();
    let out = unpack(&bytes).expect("valid package unpacks");
    let not_dir = |e: &Entry| !matches!(e, Entry::Dir { .. });
    let original: Vec<_> = scan(root.path())
        .expect("scans")
        .into_iter()
        .filter(not_dir)
        .collect();
    let unpacked: Vec<_> = scan(out.path())
        .expect("scans")
        .into_iter()
        .filter(not_dir)
        .collect();
    assert_eq!(original, unpacked);
    let inode = |p: &str| fs::metadata(out.path().join(p)).expect("exists").ino();
    assert_eq!(
        inode("usr/share/doc/ripgrep/COPYING"),
        inode("usr/share/doc/ripgrep/README")
    );
    let reader = Reader::new(bytes.as_slice()).expect("valid package");
    assert_eq!(
        Meta::from_json(reader.meta_bytes()).expect("valid"),
        *reader.meta()
    );
    assert_eq!(
        Manifest::from_json(reader.manifest_bytes()).expect("valid"),
        *reader.manifest()
    );
}

#[test]
fn metadata_and_manifest_are_read_without_the_payload() {
    let (_, _, members) = sample();
    let head = compress(&assemble(&members[..2]));
    let reader = Reader::new(head.as_slice()).expect("the beginning is enough");
    assert_eq!(reader.meta().name.as_str(), "ripgrep");
    assert!(!reader.manifest().entries().is_empty());
    let out = tempfile::tempdir().expect("temporary directory");
    let err = reader
        .unpack(out.path())
        .expect_err("the payload is missing");
    assert_eq!(err.path(), Some("etc"));
}

#[test]
fn corrupt_content_names_the_package_and_path() {
    let err = tampered(|m| member(m, "usr/bin/rg").content[0] ^= 1);
    assert_eq!(err.package(), Some("ripgrep-14.1.1-r1"));
    assert_eq!(err.path(), Some("usr/bin/rg"));
    assert!(matches!(
        err.kind(),
        ErrorKind::Mismatch {
            what: "content hash",
            ..
        }
    ));
    let text = err.to_string();
    assert!(
        text.starts_with("package ripgrep-14.1.1-r1 is corrupt"),
        "{text}"
    );
    assert!(
        text.contains("\n  path:     usr/bin/rg\n  expected: b3:"),
        "{text}"
    );
}

#[test]
fn members_must_match_their_manifest_entries() {
    let cases: [(_, Tamper); 7] = [
        ("mode", |m| m.header.mode = 0o444),
        ("owner", |m| m.header.uid = 31),
        ("group", |m| m.header.gid = 10),
        ("size", |m| {
            m.content.push(b'x');
            m.header.size += 1;
        }),
        ("member path", |m| m.header.path = b"usr/bin/rh".to_vec()),
        ("setuid", |m| m.header.mode |= 0o4000),
        ("setuid", |m| m.header.mode |= 0o2000),
    ];
    for (expected, change) in cases {
        let err = tampered(|m| change(member(m, "usr/bin/rg")));
        assert!(err.to_string().contains(expected), "{expected}: {err}");
    }
    let err = tampered(|m| member(m, "usr/bin").header.kind = Kind::File);
    assert!(err.to_string().contains("member type"), "{err}");
    let err = tampered(|m| member(m, "usr/bin/ripgrep").header.link = b"rh".to_vec());
    assert!(err.to_string().contains("symlink target"), "{err}");
    let err = tampered(|m| m.swap(3, 4));
    assert!(err.to_string().contains("member path"), "{err}");
}

#[test]
fn hard_links_must_point_to_an_identical_earlier_file() {
    let err = tampered(|m| {
        member(m, "usr/share/doc/ripgrep/README").header.link = b"usr/bin/rg".to_vec();
    });
    assert!(err.to_string().contains("hard link"), "{err}");
    let err = tampered(|m| {
        member(m, "usr/share/doc/ripgrep/README").header.link =
            b"usr/share/doc/ripgrep/README".to_vec();
    });
    assert!(err.to_string().contains("hard link"), "{err}");
}

#[test]
fn the_archive_must_end_after_the_last_entry() {
    let (_, _, members) = sample();
    let extra = Member {
        header: members[2].header.clone(),
        content: Vec::new(),
    };
    let mut longer = members.clone();
    longer.push(extra);
    let err = unpack(&package_from(&longer, &END)).expect_err("extra member");
    assert!(err.to_string().contains("does not list"), "{err}");
    assert!(unpack(&package_from(&members, &[])).is_err(), "no marker");
    assert!(
        unpack(&package_from(&members, &[0; 512])).is_err(),
        "half a marker"
    );
    let mut garbage = END.to_vec();
    garbage.extend(b"junk");
    let err = unpack(&package_from(&members, &garbage)).expect_err("data after the marker");
    assert!(err.to_string().contains("follows"), "{err}");
    let mut padded = END.to_vec();
    padded.extend([0; 8192]);
    assert!(
        unpack(&package_from(&members, &padded)).is_ok(),
        "zero padding is allowed"
    );
}

#[test]
fn metadata_and_manifest_must_come_first_and_agree() {
    let (_, _, members) = sample();
    let read = |members: &[Member]| Reader::new(package_from(members, &END).as_slice()).err();

    let mut swapped = members.clone();
    swapped.swap(0, 1);
    let err = read(&swapped).expect("manifest first is rejected");
    assert!(err.to_string().contains(".laft/meta.json"), "{err}");

    let mut other_meta = members.clone();
    let mut meta = Meta::from_json(&other_meta[0].content).expect("valid metadata");
    meta.summary = "Changed".into();
    other_meta[0].content = meta.to_json().expect("valid metadata");
    other_meta[0].header.size = other_meta[0].content.len() as u64;
    let err = read(&other_meta).expect("metadata hash mismatch is rejected");
    assert!(err.to_string().contains("metadata hash"), "{err}");

    let mut pretty = members.clone();
    let value: serde_json::Value = serde_json::from_slice(&pretty[0].content).expect("JSON");
    pretty[0].content = serde_json::to_vec_pretty(&value).expect("serializable");
    pretty[0].header.size = pretty[0].content.len() as u64;
    let err = read(&pretty).expect("pretty metadata is rejected");
    assert!(err.to_string().contains("not canonical"), "{err}");

    let mut huge = members;
    huge[0].content = vec![b' '; 1024 * 1024 + 1];
    huge[0].header.size = huge[0].content.len() as u64;
    let err = read(&huge).expect("oversized metadata is rejected");
    assert!(err.to_string().contains("larger than"), "{err}");
}

#[test]
fn extended_attributes_are_rejected() {
    let (_, _, members) = sample();
    let mut tar = assemble(&members[..2]);
    let mut records = Vec::new();
    crate::tar::pax_record(&mut records, "SCHILY.xattr.user.comment", b"hello");
    crate::tar::encode(
        &crate::tar::Header {
            kind: Kind::PaxHeader,
            path: b"PaxHeader".to_vec(),
            link: Vec::new(),
            mode: 0,
            uid: 0,
            gid: 0,
            size: records.len() as u64,
        },
        &mut tar,
    );
    tar.extend(&records);
    tar.resize(tar.len() + crate::tar::padding(records.len() as u64), 0);
    tar.extend(assemble(&members[2..]));
    tar.extend(END);
    let err = unpack(&compress(&tar)).expect_err("xattrs are rejected");
    assert!(
        err.to_string().contains("SCHILY.xattr.user.comment"),
        "{err}"
    );
}

#[test]
fn unpacking_needs_an_empty_directory() {
    let (_, bytes, _) = sample();
    let out = tempfile::tempdir().expect("temporary directory");
    fs::write(out.path().join("stray"), b"").expect("can write");
    let err = Reader::new(bytes.as_slice())
        .expect("valid package")
        .unpack(out.path())
        .expect_err("directory is not empty");
    assert!(err.to_string().contains("not empty"), "{err}");
}

#[test]
fn system_account_owners_are_applied() {
    let out = tempfile::tempdir().expect("temporary directory");
    if fs::metadata(out.path()).expect("exists").uid() != 0 {
        println!("skipped: changing ownership needs root");
        return;
    }
    let root = tempfile::tempdir().expect("temporary directory");
    crate::test_support::put(
        root.path(),
        "usr/share/polkit-1/rules.d/50.rules",
        b"rules",
        0o444,
    );
    let mut entries = scan(root.path()).expect("scans");
    for entry in &mut entries {
        if let Entry::Dir { path, owner, .. } = entry
            && path == "usr/share/polkit-1/rules.d"
        {
            *owner = Some("polkitd".into());
        }
    }
    let (meta, manifest) = crate::test_support::package(entries);
    let bytes = crate::write_package(
        Vec::new(),
        root.path(),
        &meta,
        &manifest,
        crate::Compression::DEVELOPMENT,
    )
    .expect("package writes");
    Reader::new(bytes.as_slice())
        .expect("valid package")
        .unpack(out.path())
        .expect("unpacks as root");
    let rules = fs::metadata(out.path().join("usr/share/polkit-1/rules.d")).expect("exists");
    assert_eq!((rules.uid(), rules.gid()), (34, 0));
}

proptest! {
    #[test]
    fn arbitrary_input_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = Reader::new(bytes.as_slice());
        let _ = Reader::new(compress(&bytes).as_slice());
    }
}
