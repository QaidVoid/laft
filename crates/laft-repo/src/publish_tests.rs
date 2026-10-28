use std::fs;

use laft_core::{Blake3Hash, StoreId, Timestamp};
use laft_index::{Defaults, IndexEntry};
use laft_pkg::{Arch, Meta};

use super::*;
use crate::generation::{self, Options};
use crate::sign::{Signer, keygen};

const T0: u64 = 1_761_652_800;

fn channel() -> Channel {
    Channel {
        name: "current".parse().unwrap(),
        arch: Arch::X86_64V3,
    }
}

/// Adds a pool file with `content` to `repo` and returns its entry.
fn package(repo: &Repository, name: &str, content: &[u8]) -> IndexEntry {
    let id = StoreId::from_manifest_hash(&Blake3Hash::of(content));
    let path = repo.pool(id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    fs::create_dir_all(repo.path("buildinfo")).unwrap();
    fs::write(repo.buildinfo(id), b"{}").unwrap();
    IndexEntry {
        meta: Meta {
            format: laft_pkg::FORMAT,
            name: name.parse().unwrap(),
            version: "1.0".parse().unwrap(),
            rev: 1,
            variant: Default::default(),
            arch: Arch::X86_64V3,
            origin: name.parse().unwrap(),
            summary: String::new(),
            homepage: None,
            license: vec![],
            depends: vec![],
            provides: vec![],
            conflicts: vec![],
            replaces: vec![],
            alternatives: vec![],
            triggers: vec![],
            static_deps: vec![],
            abi: Default::default(),
            abi_epoch: 0,
            requested_privileges: vec![],
            installed_size: 0,
        },
        id,
        file: crate::layout::pool_file(id),
        size: content.len() as u64,
        blake3: Blake3Hash::of(content),
    }
}

/// Writes a signed generation at `T0 + offset` listing `packages`.
fn generation(repo: &Repository, signer: &Signer, offset: u64, packages: Vec<IndexEntry>) {
    let index = Index {
        format: Index::FORMAT,
        repo: "core".parse().unwrap(),
        channel: channel().name,
        arch: Arch::X86_64V3,
        generation: 0,
        timestamp: Timestamp::from_secs(0),
        expires: Timestamp::from_secs(1),
        defaults: Defaults::default(),
        packages,
        yanked: vec![],
    };
    let options = Options {
        now: Timestamp::from_secs(T0 + offset),
        expires_in: 86_400,
        keep: 1,
    };
    let index = generation::prepare(repo, &channel(), index, &options).unwrap();
    generation::write(repo, &channel(), &index, Some(signer), &options).unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// What a reader of `target` relies on: the entry point references only
/// package files the target holds, and the generation its signature names
/// has a snapshot to fall back to.
fn assert_consistent(target: &Path) {
    let channel = channel();
    let Some(bytes) = fsx::read_optional(&target.join(channel.index())).unwrap() else {
        return;
    };
    let index = Index::decode(&bytes).unwrap();
    for entry in &index.packages {
        assert_eq!(
            fsx::size(&target.join(&entry.file)).unwrap(),
            Some(entry.size),
            "{} is missing",
            entry.file
        );
    }
    if let Some(signature) = fsx::read_optional(&target.join(sig(&channel.index()))).unwrap() {
        let signature = Signature::decode(&signature).unwrap();
        let comment: IndexComment = signature.trusted_comment().parse().unwrap();
        let snapshot = target.join(channel.snapshot(comment.generation));
        let snapshot = Index::decode(&fs::read(snapshot).unwrap()).unwrap();
        for entry in &snapshot.packages {
            assert!(target.join(&entry.file).is_file());
        }
    }
}

#[test]
fn an_interrupted_publication_never_exposes_a_missing_package() {
    let dir = tempfile::tempdir().unwrap();
    keygen(&dir.path().join("online"), false, None).unwrap();
    let signer = Signer::load(&dir.path().join("online.key"), None).unwrap();
    let repo = Repository::new(dir.path().join("repo"));
    let hello = package(&repo, "hello", b"hello");
    generation(&repo, &signer, 0, vec![hello.clone()]);
    let published = dir.path().join("published");
    run(&repo, &published, None).unwrap();
    assert_consistent(&published);

    let zlib = package(&repo, "zlib", b"zlib");
    generation(&repo, &signer, 3600, vec![hello, zlib]);
    let steps = plan(&repo, &published, None).unwrap().steps;
    assert!(steps.contains(&Step::Remove(channel().snapshot(T0))));
    for done in 0..=steps.len() {
        let target = dir.path().join(format!("target-{done}"));
        copy_tree(&published, &target);
        for step in &steps[..done] {
            apply(&repo, &target, step).unwrap();
            assert_consistent(&target);
        }
    }
    let target = dir.path().join(format!("target-{}", steps.len()));
    assert_eq!(
        fs::read(target.join(channel().index())).unwrap(),
        fs::read(repo.path(&channel().index())).unwrap()
    );
    let again = plan(&repo, &target, None).unwrap();
    assert!(
        again.steps.iter().all(|s| matches!(s, Step::Verify(_))),
        "{:?}",
        again.steps
    );
}

#[test]
fn a_published_file_that_changed_stops_publishing_before_any_copy() {
    let dir = tempfile::tempdir().unwrap();
    keygen(&dir.path().join("online"), false, None).unwrap();
    let signer = Signer::load(&dir.path().join("online.key"), None).unwrap();
    let repo = Repository::new(dir.path().join("repo"));
    let hello = package(&repo, "hello", b"hello");
    generation(&repo, &signer, 0, vec![hello.clone()]);
    let target = dir.path().join("target");
    fs::create_dir_all(target.join("pool")).unwrap();
    fs::write(target.join(&hello.file), b"tampered!").unwrap();
    let error = run(&repo, &target, None).unwrap_err();
    assert!(
        error.to_string().contains("published files never change"),
        "{error}"
    );
    assert!(!target.join(channel().index()).exists());
}
