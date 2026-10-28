//! Builders shared by the unit tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use laft_core::Blake3Hash;

use crate::{Arch, Compression, Entry, FORMAT, Manifest, Meta, scan, write_package};

pub(crate) fn meta(installed_size: u64) -> Meta {
    Meta {
        format: FORMAT,
        name: "ripgrep".parse().expect("valid name"),
        version: "14.1.1".parse().expect("valid version"),
        rev: 1,
        variant: Default::default(),
        arch: Arch::X86_64V3,
        origin: "ripgrep".parse().expect("valid name"),
        summary: "Recursively search directories for a regex pattern".into(),
        homepage: Some("https://github.com/BurntSushi/ripgrep".into()),
        license: vec!["MIT".into(), "Unlicense".into()],
        depends: vec![
            "so:libc.so.6".parse().expect("valid dependency"),
            "so:libgcc_s.so.1".parse().expect("valid dependency"),
        ],
        provides: vec![],
        conflicts: vec![],
        replaces: vec![],
        alternatives: vec![],
        triggers: vec![],
        static_deps: vec![],
        abi: Default::default(),
        abi_epoch: 0,
        requested_privileges: vec![],
        installed_size,
    }
}

pub(crate) fn dir(path: &str) -> Entry {
    Entry::Dir {
        path: path.into(),
        mode: 0o755,
        owner: None,
        group: None,
    }
}

pub(crate) fn file(path: &str, content: &[u8]) -> Entry {
    Entry::File {
        path: path.into(),
        mode: 0o444,
        size: content.len() as u64,
        hash: Blake3Hash::of(content),
        owner: None,
        group: None,
    }
}

pub(crate) fn symlink(path: &str, target: &str) -> Entry {
    Entry::Symlink {
        path: path.into(),
        target: target.into(),
    }
}

/// The metadata and manifest for `entries`.
pub(crate) fn package(entries: Vec<Entry>) -> (Meta, Manifest) {
    let size = entries
        .iter()
        .map(|e| match e {
            Entry::File { size, .. } => *size,
            _ => 0,
        })
        .sum();
    let meta = meta(size);
    let hash = Blake3Hash::of(&meta.to_json().expect("test metadata is valid"));
    let manifest = Manifest::new(meta.identity(), hash, entries).expect("test manifest is valid");
    (meta, manifest)
}

/// Writes or replaces a file in a payload directory, creating its parents,
/// with the given mode.
pub(crate) fn put(root: &Path, path: &str, content: &[u8], mode: u32) {
    let full = root.join(path);
    fs::create_dir_all(full.parent().expect("payload paths have parents"))
        .expect("can create payload directories");
    if full.exists() {
        fs::remove_file(&full).expect("can replace payload file");
    }
    fs::write(&full, content).expect("can write payload file");
    fs::set_permissions(&full, fs::Permissions::from_mode(mode)).expect("can chmod payload file");
}

/// A payload with directories, files, a duplicate file, a symlink and a
/// path longer than a ustar name.
pub(crate) fn sample_payload(root: &Path) {
    put(root, "usr/bin/rg", b"#!/bin/sh\necho rg\n", 0o555);
    put(root, "usr/share/doc/ripgrep/README", b"ripgrep\n", 0o444);
    put(root, "usr/share/doc/ripgrep/COPYING", b"ripgrep\n", 0o444);
    put(root, "etc/ripgreprc", b"smart-case\n", 0o444);
    let long = format!("usr/share/{}/deep", "d".repeat(120));
    put(root, &long, b"deep\n", 0o444);
    std::os::unix::fs::symlink("rg", root.join("usr/bin/ripgrep")).expect("can create symlink");
    for dir in ["usr", "usr/bin", "usr/share", "etc"] {
        fs::set_permissions(root.join(dir), fs::Permissions::from_mode(0o755))
            .expect("can chmod payload directory");
    }
}

/// Scans `root` and writes its package file.
pub(crate) fn build(root: &Path) -> (Meta, Manifest, Vec<u8>) {
    let (meta, manifest) = package(scan(root).expect("payload scans"));
    let mut bytes = Vec::new();
    write_package(&mut bytes, root, &meta, &manifest, Compression::DEVELOPMENT)
        .expect("package writes");
    (meta, manifest, bytes)
}

/// Compresses a tar stream the way package files are compressed.
pub(crate) fn compress(tar: &[u8]) -> Vec<u8> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).expect("encoder");
    encoder.include_checksum(true).expect("checksum flag");
    std::io::Write::write_all(&mut encoder, tar).expect("compression");
    encoder.finish().expect("compression")
}

/// Decompresses a package file into its tar stream.
pub(crate) fn decompress(package: &[u8]) -> Vec<u8> {
    zstd::stream::decode_all(package).expect("package decompresses")
}

/// One tar member with its content.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    pub(crate) header: crate::tar::Header,
    pub(crate) content: Vec<u8>,
}

/// Splits a tar stream into members, with pax headers applied, and checks
/// that it ends with exactly the end-of-archive marker.
pub(crate) fn members(tar: &[u8]) -> Vec<Member> {
    use crate::tar::{BLOCK, Kind, decode, padding, parse_pax};
    let mut members = Vec::new();
    let mut at = 0;
    let mut pax = None;
    loop {
        let block: &[u8; BLOCK] = tar[at..at + BLOCK].try_into().expect("one block");
        if block.iter().all(|&b| b == 0) {
            assert_eq!(
                tar.len(),
                at + 2 * BLOCK,
                "the archive ends with two zero blocks"
            );
            assert!(tar[at..].iter().all(|&b| b == 0));
            return members;
        }
        let mut header = decode(block).expect("valid header");
        at += BLOCK;
        let content = tar[at..at + header.size as usize].to_vec();
        at += header.size as usize + padding(header.size);
        if header.kind == Kind::PaxHeader {
            pax = Some(parse_pax(&content).expect("valid pax header"));
            continue;
        }
        if let Some(pax) = pax.take() {
            header.path = pax.path.unwrap_or(header.path);
            header.link = pax.link.unwrap_or(header.link);
            header.size = pax.size.unwrap_or(header.size);
        }
        let content = if header.size == 0 {
            Vec::new()
        } else {
            content
        };
        members.push(Member { header, content });
    }
}

/// Builds a tar stream from members, without the end-of-archive marker.
pub(crate) fn assemble(members: &[Member]) -> Vec<u8> {
    let mut out = Vec::new();
    for member in members {
        crate::tar::encode(&member.header, &mut out);
        out.extend_from_slice(&member.content);
        out.resize(
            out.len() + crate::tar::padding(member.content.len() as u64),
            0,
        );
    }
    out
}

/// The end-of-archive marker.
pub(crate) const END: [u8; 1024] = [0; 1024];
