use std::fs;

use super::*;
use crate::test_support::Env;

#[test]
fn a_missing_record_reads_as_the_default() {
    let env = Env::new();
    assert_eq!(
        ManagedRecord::load(&env.root).unwrap(),
        ManagedRecord::default()
    );
    assert_eq!(
        CopiesRecord::load(&env.root).unwrap(),
        CopiesRecord::default()
    );
}

#[test]
fn a_saved_record_reads_back_unchanged() {
    let env = Env::new();
    let record = ManagedRecord {
        generation: Some(3),
        managed: ["hosts".to_owned()].into(),
        dirs: ["app".to_owned()].into(),
        ..ManagedRecord::default()
    };
    fs::create_dir_all(env.path("laft/state")).unwrap();
    record.save(&env.root).unwrap();
    assert_eq!(ManagedRecord::load(&env.root).unwrap(), record);
}

#[test]
fn corrupt_records_and_other_versions_are_refused() {
    let env = Env::new();
    let path = state_file(&env.root, ManagedRecord::NAME);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for text in [
        "{not json",
        r#"{"version": 2, "generation": null, "managed": [], "dirs": []}"#,
        r#"{"version": 1, "generation": null, "managed": [], "dirs": [], "extra": 1}"#,
    ] {
        fs::write(&path, text).unwrap();
        let err = ManagedRecord::load(&env.root).expect_err(text);
        assert!(err.to_string().contains("etc-managed.json"), "{err}");
    }
}
