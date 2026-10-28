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
