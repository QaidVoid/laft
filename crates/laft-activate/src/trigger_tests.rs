use std::fs;
use std::os::unix::fs::symlink;

use super::*;

fn id(text: &str) -> StoreId {
    StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(text.as_bytes()))
}

#[test]
fn fingerprints_change_with_every_input() {
    let depmod = laft_core::trigger("depmod").unwrap();
    let inputs = vec!["a".to_owned(), "b".to_owned()];
    let etc = laft_core::Blake3Hash::of(b"etc");
    let base = fingerprint(depmod, "depmod:6.1", &inputs, Some(id("kmod")), &etc).unwrap();
    assert_eq!(
        base,
        fingerprint(depmod, "depmod:6.1", &inputs, Some(id("kmod")), &etc).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(depmod, "depmod:6.2", &inputs, Some(id("kmod")), &etc).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(depmod, "depmod:6.1", &inputs[..1], Some(id("kmod")), &etc).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(depmod, "depmod:6.1", &inputs, Some(id("kmod2")), &etc).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(depmod, "depmod:6.1", &inputs, None, &etc).unwrap()
    );
    assert_ne!(
        base,
        fingerprint(
            depmod,
            "depmod:6.1",
            &inputs,
            Some(id("kmod")),
            &laft_core::Blake3Hash::of(b"other etc")
        )
        .unwrap()
    );
}

#[test]
fn the_etc_digest_follows_contents_modes_and_links() {
    let dir = laft_test::tempdir();
    let tree = dir.path();
    fs::create_dir_all(tree.join("etc/ld.so.conf.d")).unwrap();
    fs::write(tree.join("etc/ld.so.conf.d/a.conf"), b"/opt/lib\n").unwrap();
    symlink("/etc/static/x", tree.join("etc/x")).unwrap();
    let base = etc_digest(tree).unwrap();
    assert_eq!(base, etc_digest(tree).unwrap());

    fs::write(tree.join("etc/ld.so.conf.d/a.conf"), b"/opt/lib64\n").unwrap();
    let content = etc_digest(tree).unwrap();
    assert_ne!(base, content);

    fs::set_permissions(
        tree.join("etc/ld.so.conf.d/a.conf"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let mode = etc_digest(tree).unwrap();
    assert_ne!(content, mode);

    fs::remove_file(tree.join("etc/x")).unwrap();
    symlink("/etc/static/y", tree.join("etc/x")).unwrap();
    assert_ne!(mode, etc_digest(tree).unwrap());
}

#[test]
fn records_of_another_version_are_not_reused() {
    let dir = laft_test::tempdir();
    for text in [
        r#"{"version": 2, "records": {}}"#,
        r#"{"fontconfig": {"fingerprint": "b3:00", "status": "ok", "outputs": []}}"#,
    ] {
        fs::write(dir.path().join(RECORDS_FILE), text).unwrap();
        assert!(read_records(dir.path()).unwrap().is_empty(), "{text}");
    }
}

#[test]
fn an_unreadable_tool_location_fails_instead_of_skipping() {
    if fsx::is_root() {
        println!("skipping: root can read every directory");
        return;
    }
    let dir = laft_test::tempdir();
    let tree = dir.path();
    fs::create_dir_all(tree.join("usr/share/fonts")).unwrap();
    fs::write(tree.join("usr/share/fonts/a.ttf"), b"").unwrap();
    fs::create_dir_all(tree.join("usr/bin")).unwrap();
    fs::create_dir_all(tree.join("etc")).unwrap();
    fs::set_permissions(tree.join("usr/bin"), fs::Permissions::from_mode(0o000)).unwrap();
    let contributors = [Contributor {
        id: id("fonts"),
        triggers: vec!["fontconfig".into()],
    }];
    let result = run(tree, &contributors, |_| None, None, &mut Vec::new());
    fs::set_permissions(tree.join("usr/bin"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err(), "{result:?}");
}

#[test]
fn instances_are_kernels_with_modules_and_themes_with_an_index() {
    let dir = laft_test::tempdir();
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
    let dir = laft_test::tempdir();
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
