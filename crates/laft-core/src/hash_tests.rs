use super::*;

#[test]
fn blake3_matches_the_reference_digest() {
    let hash = Blake3Hash::of(b"");
    let hex: String = hash.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex,
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
}

#[test]
fn blake3_text_form_round_trips() {
    let hash = Blake3Hash::of(b"laft");
    let text = hash.to_string();
    assert_eq!(text.len(), 3 + 52);
    assert_eq!(text.parse::<Blake3Hash>(), Ok(hash));
}

#[test]
fn blake3_rejects_other_prefixes_and_lengths() {
    let text = Blake3Hash::of(b"laft").to_string();
    assert!(
        text.replacen("b3:", "B3:", 1)
            .parse::<Blake3Hash>()
            .is_err()
    );
    assert!(text[..text.len() - 1].parse::<Blake3Hash>().is_err());
    assert!(text.to_uppercase().parse::<Blake3Hash>().is_err());
}

#[test]
fn sha256_text_form_round_trips() {
    let text = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let hash: Sha256Hash = text.parse().expect("valid sha256 text");
    assert_eq!(hash.to_string(), text);
}

#[test]
fn sha256_rejects_uppercase_and_wrong_lengths() {
    let text = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert!(text.to_uppercase().parse::<Sha256Hash>().is_err());
    assert!(
        text.replace("sha256:", "SHA256:")
            .parse::<Sha256Hash>()
            .is_err()
    );
    assert!(text[..text.len() - 2].parse::<Sha256Hash>().is_err());
    assert!(format!("{text}00").parse::<Sha256Hash>().is_err());
}
