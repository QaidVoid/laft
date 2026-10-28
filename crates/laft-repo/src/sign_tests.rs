use super::*;

#[test]
fn generated_keys_sign_documents_their_public_key_verifies() {
    let dir = laft_test::tempdir();
    let prefix = dir.path().join("online");
    let id = keygen(&prefix, false, None).unwrap();
    let signer = Signer::load(&dir.path().join("online.key"), None).unwrap();
    assert_eq!(signer.id(), id);
    let public = read_public(&dir.path().join("online.pub")).unwrap();
    assert_eq!(public.id(), id);
    let comment = "laft-keys repo=core version=1";
    let signature = signer.sign(b"{}", comment).unwrap();
    let decoded = Signature::decode(&signature).unwrap();
    assert!(public.verifies(b"{}", &decoded));
    assert_eq!(decoded.trusted_comment(), comment);
    assert!(!public.verifies(b"[]", &decoded));
}

#[test]
fn encrypted_keys_need_their_password() {
    let dir = laft_test::tempdir();
    let prefix = dir.path().join("root");
    keygen(&prefix, true, Some("secret".into())).unwrap();
    let path = dir.path().join("root.key");
    assert!(Signer::load(&path, Some("wrong".into())).is_err());
    let signer = Signer::load(&path, Some("secret".into())).unwrap();
    assert!(signer.sign(b"data", "comment").is_ok());
}

#[test]
fn keygen_never_overwrites_a_key() {
    let dir = laft_test::tempdir();
    let prefix = dir.path().join("k");
    let first = keygen(&prefix, false, None).unwrap();
    assert!(keygen(&prefix, false, None).is_err());
    let signer = Signer::load(&dir.path().join("k.key"), None).unwrap();
    assert_eq!(signer.id(), first);
}
