use super::{
    FileFacts, MAX_RECORD, check_file, generation_link, parse_privileged, parse_view,
    privileged_paths, version_link,
};

const TRUSTED: FileFacts = FileFacts {
    regular: true,
    uid: 0,
    mode: 0o444,
    size: 200,
};

const VIEW: &str = r#"{
  "version": 1, "name": "dev", "view_gen": 7, "created": "2026-01-01T00:00:00Z",
  "packages": [], "alternatives": {}, "users": ["alice"], "groups": ["wheel"],
  "allowed_uids": [1000], "allowed_gids": [10], "triggers": {}
}"#;

const PRIVILEGED: &str = r#"{
  "version": 1,
  "files": [{"caps": null, "gid": 0, "mode": 2541, "path": "/usr/bin/doas", "source": "x", "uid": 0}],
  "views": {"dev": {"paths": ["/usr/bin/doas"], "version": 7}}
}"#;

#[test]
fn a_read_only_root_owned_file_is_trusted() {
    assert_eq!(check_file(TRUSTED, MAX_RECORD), Ok(()));
    let writable_by_owner = FileFacts {
        mode: 0o644,
        ..TRUSTED
    };
    assert_eq!(check_file(writable_by_owner, MAX_RECORD), Ok(()));
}

#[test]
fn files_others_could_have_written_are_refused() {
    for facts in [
        FileFacts {
            uid: 1000,
            ..TRUSTED
        },
        FileFacts {
            mode: 0o464,
            ..TRUSTED
        },
        FileFacts {
            mode: 0o446,
            ..TRUSTED
        },
        FileFacts {
            regular: false,
            ..TRUSTED
        },
    ] {
        assert!(check_file(facts, MAX_RECORD).is_err(), "{facts:?}");
    }
}

#[test]
fn files_over_the_limit_are_refused() {
    let at_limit = FileFacts {
        size: MAX_RECORD,
        ..TRUSTED
    };
    assert!(check_file(at_limit, MAX_RECORD).is_ok());
    let over = FileFacts {
        size: MAX_RECORD + 1,
        ..TRUSTED
    };
    assert_eq!(check_file(over, MAX_RECORD), Err("too large"));
}

#[test]
fn a_view_record_yields_its_allowed_ids() {
    let view = parse_view(VIEW.as_bytes(), "dev", 7).unwrap();
    assert_eq!(view.allowed_uids, [1000]);
    assert_eq!(view.allowed_gids, [10]);
}

#[test]
fn a_view_record_for_another_view_or_version_is_refused() {
    assert!(parse_view(VIEW.as_bytes(), "other", 7).is_err());
    assert!(parse_view(VIEW.as_bytes(), "dev", 8).is_err());
}

#[test]
fn a_view_record_of_another_format_is_refused() {
    let newer = VIEW.replace(r#""version": 1"#, r#""version": 2"#);
    let error = parse_view(newer.as_bytes(), "dev", 7).unwrap_err();
    assert!(error.contains("format"), "{error}");
}

#[test]
fn a_view_record_without_allowed_ids_is_refused() {
    let missing = VIEW.replace(r#""allowed_uids": [1000], "#, "");
    assert!(parse_view(missing.as_bytes(), "dev", 7).is_err());
    assert!(parse_view(b"not json", "dev", 7).is_err());
    assert!(parse_view(b"", "dev", 7).is_err());
}

#[test]
fn privileged_paths_apply_only_to_the_recorded_view_version() {
    let record = parse_privileged(PRIVILEGED.as_bytes()).unwrap();
    assert_eq!(
        privileged_paths(&record, "dev", 7),
        Some(&["/usr/bin/doas".to_owned()][..])
    );
    assert_eq!(privileged_paths(&record, "dev", 6), None);
    assert_eq!(privileged_paths(&record, "dev", 8), None);
    assert_eq!(privileged_paths(&record, "other", 7), None);
}

#[test]
fn a_privileged_record_of_another_format_is_refused() {
    let newer = PRIVILEGED.replace(r#""version": 1,"#, r#""version": 2,"#);
    assert!(parse_privileged(newer.as_bytes()).is_err());
    assert!(parse_privileged(br#"{"version": 1}"#).is_err());
}

#[test]
fn the_system_link_must_name_a_mounted_generation() {
    assert_eq!(generation_link(b"mounts/42"), Some(42));
    for target in [
        &b"mounts/"[..],
        b"mounts/042",
        b"mounts/0",
        b"mounts/42/",
        b"mounts/4 2",
        b"mounts/+42",
        b"/run/laft/mounts/42",
        b"./mounts/42",
        b"mounts/../42",
        b"42",
        b"mounts/99999999999999999999999",
    ] {
        assert_eq!(generation_link(target), None, "{target:?}");
    }
}

#[test]
fn the_current_link_must_name_a_version_beside_it() {
    assert_eq!(version_link(b"7"), Some(7));
    for target in [
        &b""[..],
        b"07",
        b"0",
        b"../7",
        b"/laft/views/dev/7",
        b"/tmp/evil",
        b"7/",
        b"-7",
    ] {
        assert_eq!(version_link(target), None, "{target:?}");
    }
}
