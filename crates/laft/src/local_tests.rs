//! Tests of the key and the repository source of the local repository.

use super::*;
use std::fs;

use laft_index::PublicKey;

#[test]
fn a_local_key_is_generated_once_and_stays_secret() {
    let dir = laft_test::tempdir();
    let local = Local::of(&dir.path().join("root"));
    let first = local.public_key().expect("a key");
    let secret = dir.path().join("root/laft/state/local.key");
    let mode = fs::metadata(&secret).expect("the key exists").permissions();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(mode.mode() & 0o777, 0o600);
    let second = local.public_key().expect("a key");
    assert_eq!(first, second);
    let public = dir.path().join("root/laft/state/local.key.pub");
    let text = fs::read_to_string(&public).expect("the public key exists");
    let line = text.lines().last().expect("a key line").trim();
    let listed: PublicKey = line.parse().expect("a public key");
    assert_eq!(listed, first);
}

#[test]
fn a_repository_without_builds_has_no_index_yet() {
    let dir = laft_test::tempdir();
    let local = Local::of(&dir.path().join("root"));
    assert!(local.source().expect("the source is readable").is_none());
}
