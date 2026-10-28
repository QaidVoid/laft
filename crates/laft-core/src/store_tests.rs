use super::*;

#[test]
fn id_is_the_first_32_characters_of_the_manifest_hash() {
    let hash = Blake3Hash::of(b"manifest");
    let id = StoreId::from_manifest_hash(&hash);
    let text = id.to_string();
    assert_eq!(text.len(), 32);
    assert_eq!(text, hash.to_string()["b3:".len().."b3:".len() + 32]);
    assert_eq!(text.parse::<StoreId>(), Ok(id));
}

#[test]
fn dir_name_marks_only_non_default_variants() {
    let id = StoreId::from_manifest_hash(&Blake3Hash::of(b"manifest"));
    let name: Name = "ripgrep".parse().expect("valid name");
    let version: Version = "14.1.1".parse().expect("valid version");
    assert_eq!(
        id.dir_name(&name, &version, Variant::Dynamic),
        format!("{id}-ripgrep-14.1.1")
    );
    assert_eq!(
        id.dir_name(&name, &version, Variant::Static),
        format!("{id}-ripgrep-14.1.1+static")
    );
}

#[test]
fn rejects_ids_of_the_wrong_length_or_alphabet() {
    let id = StoreId::from_manifest_hash(&Blake3Hash::of(b"manifest")).to_string();
    assert!(id[..31].parse::<StoreId>().is_err());
    assert!(id.to_uppercase().parse::<StoreId>().is_err());
    assert!(format!("{id}a").parse::<StoreId>().is_err());
}
