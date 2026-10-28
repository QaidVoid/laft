use super::*;
use crate::test_support::TestKey;

#[test]
fn key_ids_read_like_minisign_prints_them() {
    let key: PublicKey = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
        .parse()
        .expect("valid key");
    assert_eq!(key.id().to_string(), "e7620f1842b4e81f");
    assert_eq!(
        "e7620f1842b4e81f".parse::<KeyId>().expect("valid id"),
        key.id()
    );
}

#[test]
fn malformed_key_ids_are_rejected() {
    for text in [
        "",
        "e7620f1842b4e81",
        "E7620F1842B4E81F",
        "+7620f1842b4e81f",
        "e7620f1842b4e81f0",
    ] {
        assert!(text.parse::<KeyId>().is_err(), "{text:?}");
    }
}

#[test]
fn malformed_public_keys_are_rejected() {
    for text in [
        "",
        "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO",
        "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3=",
        "UWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3",
        "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO!",
    ] {
        assert!(text.parse::<PublicKey>().is_err(), "{text:?}");
    }
}

#[test]
fn signature_names_its_key_and_comment() {
    let key = TestKey::generate();
    let signature = Signature::decode(&key.sign(b"data", "laft-keys repo=core version=1"))
        .expect("minisign output decodes");
    assert_eq!(signature.key_id(), key.id());
    assert_eq!(signature.trusted_comment(), "laft-keys repo=core version=1");
    assert!(key.public.verifies(b"data", &signature));
    assert!(!key.public.verifies(b"Data", &signature));
    assert!(!TestKey::generate().public.verifies(b"data", &signature));
}

#[test]
fn signature_without_trusted_comment_is_rejected() {
    let key = TestKey::generate();
    let text = String::from_utf8(key.sign(b"data", "c")).expect("text");
    let lines: Vec<&str> = text.lines().collect();
    for broken in [lines[..2].join("\n"), String::new(), "\u{fffd}".repeat(3)] {
        assert!(Signature::decode(broken.as_bytes()).is_err(), "{broken:?}");
    }
    assert!(Signature::decode(&[0xff, 0xfe]).is_err());
}

#[test]
fn base64_decoding_follows_rfc_4648() {
    assert_eq!(decode_base64("Zm9vYmFy").as_deref(), Some(&b"foobar"[..]));
    assert_eq!(decode_base64("Zm9vYg==").as_deref(), Some(&b"foob"[..]));
    assert_eq!(decode_base64("Zm9vYmE=").as_deref(), Some(&b"fooba"[..]));
    for bad in ["", "Zm9", "Zm9vY===", "Zm==Zm9v", "Zm9v YmFy"] {
        assert_eq!(decode_base64(bad), None, "{bad:?}");
    }
}
