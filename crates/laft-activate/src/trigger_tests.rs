use std::fs;

use super::*;

fn id(text: &str) -> StoreId {
    StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(text.as_bytes()))
}

#[test]
fn fingerprints_change_with_every_input() {
    let depmod = laft_core::trigger("depmod").unwrap();
    let inputs = vec!["a".to_owned(), "b".to_owned()];
    let base = fingerprint(depmod, "depmod:6.1", &inputs, Some(id("kmod"))).unwrap();
    assert_eq!(
        base,
        fingerprint(depmod, "depmod:6.1", &inputs, Some(id("kmod"))).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(depmod, "depmod:6.2", &inputs, Some(id("kmod"))).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(depmod, "depmod:6.1", &inputs[..1], Some(id("kmod"))).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(depmod, "depmod:6.1", &inputs, Some(id("kmod2"))).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(depmod, "depmod:6.1", &inputs, None).unwrap()
    );
}

#[test]
fn instances_are_kernels_with_modules_and_themes_with_an_index() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path();
    for path in [
        "usr/lib/modules/6.1/kernel/a.ko.zst",
        "usr/lib/modules/6.2/vmlinuz",
        "usr/lib/modules/6.3/kernel/x/b.ko",
        "usr/share/icons/hicolor/index.theme",
        "usr/share/icons/Adwaita/index.theme",
        "usr/share/icons/plain/a.png",
    ] {
        fs::create_dir_all(tree.join(path).parent().unwrap()).unwrap();
        fs::write(tree.join(path), b"").unwrap();
    }
    let keys = |name| {
        instances(tree, laft_core::trigger(name).unwrap())
            .unwrap()
            .into_iter()
            .map(|k| k.unwrap_or_default())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys("depmod"), ["6.1", "6.3"]);
    assert_eq!(keys("icons"), ["Adwaita", "hicolor"]);
    assert_eq!(keys("fontconfig"), [""]);
}

#[test]
fn records_round_trip_through_the_records_file() {
    let dir = tempfile::tempdir().unwrap();
    let records = BTreeMap::from([(
        "fontconfig".to_owned(),
        record(
            laft_core::Blake3Hash::of(b"x"),
            TriggerStatus::Ok,
            vec!["usr/lib/fontconfig".into()],
        ),
    )]);
    write_records(dir.path(), &records).unwrap();
    assert_eq!(read_records(dir.path()).unwrap(), records);
    assert!(
        read_records(&dir.path().join("missing"))
            .unwrap()
            .is_empty()
    );
}
