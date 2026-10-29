use super::*;

fn record(name: &str, reasons: Vec<Reason>) -> PackageRecord {
    PackageRecord {
        name: name.parse().expect("valid"),
        version: "1.0".parse().expect("valid"),
        rev: 1,
        variant: Variant::Dynamic,
        id: laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(name.as_bytes())),
        dir: name.into(),
        reasons,
    }
}

fn needed_by(by: &str, dep: &str) -> Reason {
    Reason::Dependency {
        by: by.parse().expect("valid"),
        dep: dep.into(),
    }
}

#[test]
fn the_chain_is_the_shortest_way_back_to_a_request() {
    let packages = [
        record(
            "app",
            vec![Reason::Requested {
                origin: "system.stav:3:14".into(),
                text: "app".into(),
            }],
        ),
        record(
            "lib",
            vec![
                needed_by("mid", "so:libx.so.1"),
                needed_by("app", "so:libx.so.1"),
            ],
        ),
        record("mid", vec![needed_by("app", "mid")]),
    ];
    let by_name: BTreeMap<&Name, &PackageRecord> = packages.iter().map(|p| (&p.name, p)).collect();
    let chain = chain(&by_name, &"lib".parse().expect("valid"));
    let names: Vec<&str> = chain.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["lib", "app"]);
    assert!(matches!(chain[1].1, Reason::Requested { .. }));
}

#[test]
fn trigger_outputs_are_found_in_the_trigger_records() {
    let dir = laft_test::tempdir();
    fs::write(
        dir.path().join("triggers.json"),
        r#"{"records":{"glib-schemas":{"fingerprint":"x","status":"ok","outputs":["usr/share/glib-2.0/schemas/gschemas.compiled"]}},"version":1}"#,
    )
    .expect("writable");
    let owner = trigger_owner(dir.path(), "usr/share/glib-2.0/schemas/gschemas.compiled");
    assert_eq!(owner.expect("found")["trigger"], "glib-schemas");
    assert!(trigger_owner(dir.path(), "usr/bin/rg").is_none());
}
