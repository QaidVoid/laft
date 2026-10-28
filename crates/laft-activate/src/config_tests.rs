use laft_core::LuksOption;

use super::BootConfig;

fn luks_with(options: &str) -> serde_json::Result<BootConfig> {
    serde_json::from_str(&format!(
        r#"{{"luks": {{"cryptroot": {{"device": "UUID=6b1d", "options": {options}}}}}}}"#
    ))
}

#[test]
fn every_luks_option_laft_boot_knows_is_accepted() {
    let config = luks_with(r#"["discard", "no-read-workqueue", "no-write-workqueue"]"#)
        .expect("valid options");
    assert_eq!(
        config.luks["cryptroot"]
            .options
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [
            LuksOption::Discard,
            LuksOption::NoReadWorkqueue,
            LuksOption::NoWriteWorkqueue
        ]
    );
}

#[test]
fn a_misspelled_luks_option_is_rejected_when_the_configuration_is_read() {
    let err = luks_with(r#"["discard", "trim"]"#).unwrap_err();
    assert!(
        err.to_string().contains(r#"invalid LUKS option "trim""#),
        "{err}"
    );
}

#[test]
fn only_the_limine_loader_is_accepted() {
    let config: BootConfig =
        serde_json::from_str(r#"{"boot": {"loader": "limine"}}"#).expect("limine is known");
    assert_eq!(config.boot.loader, super::Loader::Limine);
    let err = serde_json::from_str::<BootConfig>(r#"{"boot": {"loader": "grub"}}"#).unwrap_err();
    assert!(err.to_string().contains("grub"), "{err}");
}

#[test]
fn file_systems_have_no_auto_setting_and_users_no_unknown_keys() {
    let fs = r#"{"fs": {"/": {"fstype": "ext4", "auto": false}}}"#;
    assert!(serde_json::from_str::<BootConfig>(fs).is_err());
    let users = r#"{"users": {"mutabel": false}}"#;
    assert!(serde_json::from_str::<super::EtcConfig>(users).is_err());
}
