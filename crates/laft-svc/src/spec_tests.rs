use std::path::Path;

use crate::test_support::{example, valid};
use crate::{Error, Spec};

#[test]
fn spec_file_round_trips_through_canonical_json() {
    let set = valid(&example());
    for spec in set.specs().values() {
        let bytes = spec.to_bytes().expect("validated specs encode");
        let read = Spec::parse(&bytes, Path::new("spec.json")).expect("the spec reads back");
        assert_eq!(&read, spec);
        assert_eq!(read.to_bytes().expect("encodes"), bytes);
    }
}

#[test]
fn spec_file_matches_the_documented_layout() {
    let set = valid(&example());
    let json = String::from_utf8(set.specs()["myapp"].to_bytes().expect("encodes"))
        .expect("canonical JSON is UTF-8");
    let expected = concat!(
        r#"{"after":["dbus","early"],"cwd":"/var/lib/myapp","down":[],"early":false,"#,
        r#""env":{"RUST_LOG":"info"},"exec":["/usr/bin/myapp","--listen","127.0.0.1:8080"],"#,
        r#""group":null,"groups":[],"kind":"longrun","#,
        r#""limits":{"core":null,"memlock":null,"nofile":65536,"nproc":null},"#,
        r#""log":{"count":5,"dir":"/var/log/myapp","size":5000000,"type":"logged"},"#,
        r#""name":"myapp","on_change":"restart","ready":{"type":"immediate"},"#,
        r#""restart":"on-failure","restart_delay_ms":1000,"runtime_dirs":[],"spec_version":1,"#,
        r#""state_dirs":[{"group":null,"mode":488,"owner":null,"path":"/var/lib/myapp"}],"#,
        r#""stop_signal":15,"stop_timeout_ms":10000,"umask":18,"up":[],"user":"myapp"}"#,
    );
    assert_eq!(json, expected);
}

#[test]
fn other_spec_versions_are_rejected() {
    let set = valid(&example());
    let json = String::from_utf8(set.specs()["dbus"].to_bytes().expect("encodes"))
        .expect("canonical JSON is UTF-8");
    let newer = json.replace(r#""spec_version":1"#, r#""spec_version":2"#);
    match Spec::parse(newer.as_bytes(), Path::new("dbus.json")) {
        Err(Error::Corrupt { reason, .. }) => assert_eq!(reason, "unsupported spec version 2"),
        other => panic!("expected a corrupt spec, got {other:?}"),
    }
    let unknown = json.replace(r#""umask":18"#, r#""umask":18,"extra":1"#);
    assert!(Spec::parse(unknown.as_bytes(), Path::new("dbus.json")).is_err());
}
