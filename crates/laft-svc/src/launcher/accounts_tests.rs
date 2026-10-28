use std::path::Path;

use super::Accounts;
use crate::validate::resolve;
use crate::{Dir, Error, Service};

const PASSWD: &str = "root:x:0:0:root:/root:/usr/bin/bash\n\
                      messagebus:x:30:30::/var/empty:/usr/bin/nologin\n\
                      myapp:x:101:101::/var/lib/myapp:/usr/bin/nologin\n";
const GROUP: &str = "root:x:0:\nwheel:x:10:alice\nmessagebus:x:30:\nmyapp:x:101:\nvideo:x:21:\n";

fn accounts() -> Accounts {
    Accounts::parse(PASSWD, Path::new("passwd"), GROUP, Path::new("group")).expect("parses")
}

#[test]
fn credentials_default_to_the_primary_group_plus_supplementary_ones() {
    let service = Service {
        user: "myapp".into(),
        groups: ["wheel".to_owned(), "video".to_owned()].into(),
        ..Service::longrun(&["/usr/bin/myapp"])
    };
    let credentials = accounts()
        .credentials(&resolve("myapp", &service))
        .expect("resolves");
    assert_eq!((credentials.uid, credentials.gid), (101, 101));
    assert_eq!(credentials.groups, [10, 21, 101]);
    assert_eq!(credentials.home, "/var/lib/myapp");
}

#[test]
fn explicit_group_replaces_the_primary_group() {
    let service = Service {
        user: "myapp".into(),
        group: Some("video".into()),
        ..Service::longrun(&["/usr/bin/myapp"])
    };
    let credentials = accounts()
        .credentials(&resolve("myapp", &service))
        .expect("resolves");
    assert_eq!(credentials.gid, 21);
    assert_eq!(credentials.groups, [21]);
}

#[test]
fn unknown_account_names_the_service() {
    let service = Service {
        user: "ghost".into(),
        ..Service::longrun(&["/usr/bin/x"])
    };
    let error = accounts().credentials(&resolve("x", &service)).unwrap_err();
    assert_eq!(error.to_string(), r#"service x: no user named "ghost""#);
}

#[test]
fn directory_owner_defaults_follow_the_declared_owner() {
    let service = Service {
        user: "myapp".into(),
        group: Some("video".into()),
        runtime_dirs: vec![
            Dir::new("plain"),
            Dir {
                owner: Some("messagebus".into()),
                ..Dir::new("bus")
            },
            Dir {
                group: Some("wheel".into()),
                ..Dir::new("grouped")
            },
        ],
        ..Service::longrun(&["/usr/bin/myapp"])
    };
    let spec = resolve("myapp", &service);
    let accounts = accounts();
    let credentials = accounts.credentials(&spec).expect("resolves");
    let owners: Vec<(u32, u32)> = spec
        .runtime_dirs
        .iter()
        .map(|d| {
            accounts
                .dir_owner(&spec, &credentials, d)
                .expect("resolves")
        })
        .collect();
    assert_eq!(owners, [(101, 21), (30, 30), (101, 10)]);
}

#[test]
fn malformed_lines_are_rejected() {
    let passwd = "root:x:0:0:root:/root\n";
    match Accounts::parse(passwd, Path::new("passwd"), GROUP, Path::new("group")) {
        Err(Error::Corrupt { reason, .. }) => assert_eq!(reason, "line 1: expected 7 fields"),
        other => panic!("expected a malformed line, got {other:?}"),
    }
    let group = "root:x:zero:\n";
    assert!(Accounts::parse(PASSWD, Path::new("passwd"), group, Path::new("group")).is_err());
}
