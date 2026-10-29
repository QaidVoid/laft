use super::*;
use crate::test_support::{Env, base_users};

fn declared() -> Declared {
    let env = Env::new();
    crate::declare(&env.lock(), &base_users()).unwrap()
}

fn entry(caps: Option<&str>) -> Privileged {
    Privileged {
        caps: caps.map(str::to_owned),
        ..Privileged::default()
    }
}

#[test]
fn entries_resolve_owners_and_parse_capabilities() {
    let mut errors = Vec::new();
    let unix_chkpwd = Privileged {
        group: "shadow".into(),
        mode: 0o2755,
        ..Privileged::default()
    };
    let planned = plan(
        "/usr/bin/unix_chkpwd",
        &unix_chkpwd,
        &declared(),
        &mut errors,
    )
    .unwrap();
    assert_eq!((planned.1, planned.3), (0, 0o2755));
    assert_eq!(
        Some(planned.2),
        declared().groups.get("shadow").map(|g| g.gid)
    );
    let planned = plan(
        "/usr/lib/helper",
        &entry(Some("cap_setuid=ep")),
        &declared(),
        &mut errors,
    )
    .unwrap();
    assert_eq!(planned.4.unwrap().1.permitted, 1 << 7);
    assert!(errors.is_empty());
}

#[test]
fn invalid_entries_are_reported_together() {
    let declared = declared();
    let mut errors = Vec::new();
    for path in [
        "/opt/x",
        "/usr/bin",
        "/usr/bin/../sbin/x",
        "usr/bin/x",
        "/usr/bin//x",
        "/usr/bin/x/",
    ] {
        assert!(
            plan(path, &entry(None), &declared, &mut errors).is_none(),
            "{path}"
        );
    }
    assert!(
        plan(
            "/usr/bin/a",
            &entry(Some("cap_setuid+x")),
            &declared,
            &mut errors
        )
        .is_none()
    );
    let stranger = Privileged {
        owner: "stranger".into(),
        ..Privileged::default()
    };
    assert!(plan("/usr/bin/b", &stranger, &declared, &mut errors).is_none());
    assert_eq!(errors.len(), 8);
    assert!(
        errors[..6]
            .iter()
            .all(|e| matches!(e, ConfigError::PrivilegedPath(_)))
    );
    assert!(matches!(&errors[6], ConfigError::InvalidCaps { caps, .. } if caps == "cap_setuid+x"));
    assert!(matches!(&errors[7], ConfigError::UnknownAccount { name, .. } if name == "stranger"));
    assert_eq!(errors[7].code(), Some("C0492"));
}
