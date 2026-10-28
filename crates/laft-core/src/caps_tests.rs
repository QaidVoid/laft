use super::*;

fn parse(text: &str) -> FileCapabilities {
    text.parse().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn equals_sets_the_listed_flags() {
    let caps = parse("cap_setuid=ep");
    assert_eq!(caps.permitted, 1 << 7);
    assert_eq!(caps.inheritable, 0);
    assert!(caps.effective);
}

#[test]
fn several_names_and_operations_combine() {
    let caps = parse("cap_net_raw,cap_net_admin+ep");
    assert_eq!(caps.permitted, (1 << 13) | (1 << 12));
    let caps = parse("cap_chown,cap_kill=pi cap_kill-i");
    assert_eq!(caps.permitted, (1 << 0) | (1 << 5));
    assert_eq!(caps.inheritable, 1 << 0);
    assert!(!caps.effective);
}

#[test]
fn high_capabilities_land_in_the_second_word() {
    let caps = parse("cap_checkpoint_restore+p");
    assert_eq!(caps.permitted, 1 << 40);
    let xattr = caps.to_xattr();
    assert_eq!(&xattr[0..4], &0x0200_0000u32.to_le_bytes());
    assert_eq!(&xattr[4..8], &[0; 4]);
    assert_eq!(&xattr[12..16], &(1u32 << 8).to_le_bytes());
}

#[test]
fn xattr_carries_the_effective_flag() {
    let xattr = parse("cap_setgid=ep").to_xattr();
    assert_eq!(&xattr[0..4], &0x0200_0001u32.to_le_bytes());
    assert_eq!(&xattr[4..8], &(1u32 << 6).to_le_bytes());
    assert_eq!(&xattr[8..20], &[0; 12]);
}

#[test]
fn malformed_text_is_rejected() {
    for bad in [
        "",
        "cap_setuid",
        "cap_setuid+x",
        "cap_nonsense=ep",
        "+ep",
        "cap_setuid+",
        "cap_setuid=e",
        "cap_setuid,cap_setgid=p cap_setuid+e",
    ] {
        assert!(bad.parse::<FileCapabilities>().is_err(), "{bad:?}");
    }
}

#[test]
fn empty_list_with_equals_means_every_capability() {
    let caps = parse("=p");
    assert_eq!(caps.permitted, (1 << 41) - 1);
}
