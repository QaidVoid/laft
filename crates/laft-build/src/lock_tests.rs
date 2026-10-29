use super::*;

#[test]
fn a_second_lock_of_a_build_root_fails_at_once_and_names_the_lock() {
    let dir = laft_test::tempdir();
    let root = BuildRoot::new(dir.path().join("build"));
    let held = root.lock("stage", None).unwrap();
    let error = root.lock("build", None).unwrap_err();
    assert_eq!(error.code(), Some("B032"), "{error}");
    let message = error.to_string();
    assert!(
        message.contains(&root.lock_path().display().to_string()),
        "{message}"
    );
    assert!(message.contains("laft-build stage (pid "), "{message}");
    drop(held);
    root.lock("build", None)
        .expect("the lock is free once its holder dropped it");
}

#[test]
fn only_the_token_of_the_holder_shares_the_lock() {
    let dir = laft_test::tempdir();
    let root = BuildRoot::new(dir.path().join("build"));
    let held = root.lock("stage", None).unwrap();
    let shared = root
        .lock("bootstrap", Some(held.token()))
        .expect("the holder's token shares the lock");
    assert_eq!(shared.token(), held.token());
    let other = BuildRoot::new(dir.path().join("other"))
        .lock("stage", None)
        .unwrap();
    let error = root.lock("bootstrap", Some(other.token())).unwrap_err();
    assert_eq!(error.code(), Some("B032"), "{error}");
}
