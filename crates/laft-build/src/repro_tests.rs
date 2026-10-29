use super::*;
use crate::test_support::TestPkg;

fn package(dir: &Path, content: &'static [u8]) -> PathBuf {
    let store = laft_store::Store::at(dir.join("store"));
    TestPkg {
        files: &[("usr/bin/tool", content), ("usr/share/doc/tool", b"same")],
        ..TestPkg::new("tool")
    }
    .add(&store, dir);
    dir.join("tool-1.0-r1.x86_64-v3.lpkg")
}

#[test]
fn identical_packages_have_no_differing_files() {
    let (one, two) = (laft_test::tempdir(), laft_test::tempdir());
    let (a, b) = (package(one.path(), b"v1"), package(two.path(), b"v1"));
    assert_eq!(compare(&[a], &[b]).unwrap(), vec![]);
}

#[test]
fn differing_files_are_listed_with_both_versions() {
    let (one, two) = (laft_test::tempdir(), laft_test::tempdir());
    let (a, b) = (package(one.path(), b"v1"), package(two.path(), b"v22"));
    let differing = compare(&[a], &[b]).unwrap();
    assert_eq!(differing.len(), 1);
    assert_eq!(differing[0].path, "usr/bin/tool");
    assert_eq!(differing[0].package.as_str(), "tool");
    assert!(
        differing[0]
            .first
            .as_deref()
            .unwrap()
            .starts_with("file 555 2 ")
    );
    assert!(
        differing[0]
            .second
            .as_deref()
            .unwrap()
            .starts_with("file 555 3 ")
    );
}

#[test]
fn a_report_is_kept_per_item() {
    let dir = laft_test::tempdir();
    let db = Db::new(dir.path());
    let report = ReproReport {
        build_key: Blake3Hash::of(b"key"),
        reproducible: true,
        differing_packages: vec![],
        differing_files: vec![],
        checked: Timestamp::from_secs(0),
    };
    record_repro(&db, &crate::test_support::item("zlib:static"), &report).unwrap();
    let text = std::fs::read(dir.path().join("repro/zlib+static.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<ReproReport>(&text).unwrap(),
        report
    );
}

#[test]
fn package_files_are_compared_byte_for_byte() {
    let (one, two) = (laft_test::tempdir(), laft_test::tempdir());
    let (a, b) = (package(one.path(), b"v1"), package(two.path(), b"v1"));
    assert_eq!(
        differing_packages(std::slice::from_ref(&a), std::slice::from_ref(&b)).unwrap(),
        Vec::<String>::new()
    );
    let mut bytes = std::fs::read(&b).unwrap();
    bytes.push(0);
    std::fs::write(&b, bytes).unwrap();
    let name = "tool-1.0-r1.x86_64-v3.lpkg";
    assert_eq!(
        differing_packages(std::slice::from_ref(&a), std::slice::from_ref(&b)).unwrap(),
        [name]
    );
    assert_eq!(differing_packages(&[a], &[]).unwrap(), [name]);
}
