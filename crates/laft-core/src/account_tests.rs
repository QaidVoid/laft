use std::collections::BTreeSet;

use super::*;

#[test]
fn users_and_groups_resolve_only_in_their_own_namespace() {
    assert_eq!(system_uid("root"), Some(0));
    assert_eq!(system_gid("root"), Some(0));
    assert_eq!(system_uid("nobody"), Some(65534));
    assert_eq!(system_gid("nobody"), None);
    assert_eq!(system_gid("nogroup"), Some(65534));
    assert_eq!(system_uid("nogroup"), None);
    assert_eq!(system_gid("shadow"), Some(41));
    assert_eq!(system_uid("shadow"), None);
    assert_eq!(system_uid("log"), Some(40));
    assert_eq!(system_gid("log"), Some(40));
}

#[test]
fn unknown_names_are_not_accounts() {
    for name in ["", "Root", "daemon", "bin", "root ", "1000"] {
        assert_eq!(system_uid(name), None, "{name:?}");
        assert_eq!(system_gid(name), None, "{name:?}");
    }
}

#[test]
fn registry_names_and_ids_are_unique_per_namespace() {
    let mut names = BTreeSet::new();
    let mut uids = BTreeSet::new();
    let mut gids = BTreeSet::new();
    for &(name, id, kind) in REGISTRY {
        assert!(names.insert(name), "duplicate name {name}");
        if kind != Kind::Group {
            assert!(uids.insert(id), "duplicate uid {id}");
        }
        if kind != Kind::User {
            assert!(gids.insert(id), "duplicate gid {id}");
        }
    }
}

#[test]
fn registry_ids_stay_in_the_reserved_ranges() {
    for &(name, id, _) in REGISTRY {
        assert!(
            id <= 100 || id == 65534,
            "{name} has id {id} outside the static ranges"
        );
    }
}
