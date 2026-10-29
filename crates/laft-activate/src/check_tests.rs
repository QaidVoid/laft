use super::*;
use crate::{File, Group, Privileged, User};

fn codes(config: &EtcConfig, security: &Security) -> Vec<Option<&'static str>> {
    check_config(config, security)
        .iter()
        .map(ConfigError::code)
        .collect()
}

#[test]
fn a_valid_configuration_has_no_errors() {
    let mut config = EtcConfig::default();
    config
        .users
        .accounts
        .insert("alice".into(), User::default());
    config.files.insert(
        "/etc/doas.conf".into(),
        File {
            text: Some("permit :wheel\n".into()),
            ..File::default()
        },
    );
    assert_eq!(codes(&config, &Security::default()), []);
}

#[test]
fn source_files_are_not_read() {
    let mut config = EtcConfig::default();
    config.files.insert(
        "/etc/foo.conf".into(),
        File {
            source: Some("/nonexistent/foo.conf".into()),
            ..File::default()
        },
    );
    assert_eq!(codes(&config, &Security::default()), []);
}

#[test]
fn a_file_below_a_derived_file_clashes() {
    let mut config = EtcConfig::default();
    config.files.insert(
        "/etc/hostname/x".into(),
        File {
            text: Some(String::new()),
            ..File::default()
        },
    );
    assert_eq!(codes(&config, &Security::default()), [Some("C0425")]);
}

#[test]
fn account_rules_apply_without_any_recorded_state() {
    let mut config = EtcConfig::default();
    for name in ["alice", "bob"] {
        let user = User {
            uid: Some(1000),
            ..User::default()
        };
        config.users.accounts.insert(name.into(), user);
    }
    let group = Group {
        gid: Some(77),
        ..Group::default()
    };
    config.users.groups.insert("sshd".into(), group);
    assert_eq!(
        codes(&config, &Security::default()),
        [Some("C0414"), Some("C0408")]
    );
}

#[test]
fn a_privileged_entry_that_grants_nothing_is_rejected() {
    let mut security = Security::default();
    let entry = Privileged {
        mode: 0o755,
        ..Privileged::default()
    };
    security.privileged.insert("/usr/bin/foo".into(), entry);
    let disabled = Privileged {
        mode: 0o755,
        enable: false,
        ..Privileged::default()
    };
    security.privileged.insert("/usr/bin/bar".into(), disabled);
    let errors = check_config(&EtcConfig::default(), &security);
    assert_eq!(errors, [ConfigError::Unprivileged("/usr/bin/foo".into())]);
}
