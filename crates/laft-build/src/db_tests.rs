use std::collections::BTreeMap;

use super::*;
use crate::KeyInputs;
use crate::test_support::{item, recipe};

fn stored(rev: u32) -> Stored {
    let mut recipe = recipe("zlib");
    recipe.rev = rev;
    let inputs = KeyInputs::new(&recipe, vec![], vec![]);
    Stored {
        record: BuildRecord {
            item: item("zlib"),
            key: inputs.build_key(),
            inputs,
            outputs: BTreeMap::new(),
            imports: BTreeMap::new(),
        },
        buildinfo: format!("zlib-1.0-r{rev}.buildinfo.json"),
        files: vec![format!("zlib-1.0-r{rev}.x86_64-v3.lpkg")],
    }
}

#[test]
fn the_latest_record_of_an_item_is_found_again() {
    let dir = laft_test::tempdir();
    let db = Db::new(dir.path());
    assert_eq!(db.latest(&item("zlib")).unwrap(), None);
    let first = stored(1);
    db.record(&first).unwrap();
    assert_eq!(db.latest(&item("zlib")).unwrap().as_ref(), Some(&first));
    let second = stored(2);
    db.record(&second).unwrap();
    assert_eq!(db.latest(&item("zlib")).unwrap().as_ref(), Some(&second));
    assert_eq!(db.find(&first.record.key).unwrap().as_ref(), Some(&first));
    assert_eq!(db.latest(&item("zlib:static")).unwrap(), None);
}

#[test]
fn a_corrupt_record_is_an_error() {
    let dir = laft_test::tempdir();
    let db = Db::new(dir.path());
    fs::create_dir_all(dir.path().join("latest")).unwrap();
    fs::write(dir.path().join("latest/zlib+dynamic"), b"{").unwrap();
    assert!(db.latest(&item("zlib")).is_err());
}
