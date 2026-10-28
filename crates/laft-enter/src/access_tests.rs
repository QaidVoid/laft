use super::{Caller, may_enter};
use crate::records::{ViewRecord, parse_view};

fn view(uids: &[u32], gids: &[u32]) -> ViewRecord {
    let json = format!(
        r#"{{"version": 1, "name": "dev", "view_gen": 1, "allowed_uids": {uids:?}, "allowed_gids": {gids:?}}}"#
    );
    parse_view(json.as_bytes(), "dev", 1).unwrap()
}

fn caller(uid: u32, gid: u32, groups: &[u32]) -> Caller {
    Caller {
        uid,
        gid,
        groups: groups.to_vec(),
    }
}

#[test]
fn root_may_enter_any_view() {
    assert!(may_enter(&caller(0, 0, &[]), &view(&[], &[])));
}

#[test]
fn a_listed_user_may_enter() {
    assert!(may_enter(&caller(1000, 1000, &[]), &view(&[1000], &[])));
}

#[test]
fn a_member_of_a_listed_group_may_enter() {
    let wheel = view(&[], &[10]);
    assert!(may_enter(&caller(1001, 1001, &[100, 10]), &wheel));
    assert!(may_enter(&caller(1001, 10, &[]), &wheel));
}

#[test]
fn anyone_else_is_refused() {
    let dev = view(&[1000], &[10]);
    assert!(!may_enter(&caller(1001, 1001, &[100, 1000]), &dev));
    assert!(!may_enter(&caller(1001, 1000, &[]), &dev));
}

#[test]
fn a_group_id_equal_to_an_allowed_uid_grants_nothing() {
    assert!(!may_enter(
        &caller(1001, 1000, &[1000]),
        &view(&[1000], &[])
    ));
}
