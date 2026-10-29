use super::*;

fn pkg(name: &str, version: &str, rev: u32, variant: Variant, id: &[u8]) -> Pkg {
    Pkg {
        name: name.parse().expect("valid"),
        version: version.parse().expect("valid"),
        rev,
        variant,
        id: StoreId::from_manifest_hash(&Blake3Hash::of(id)),
    }
}

fn map(pkgs: Vec<Pkg>) -> BTreeMap<Name, Pkg> {
    pkgs.into_iter().map(|p| (p.name.clone(), p)).collect()
}

#[test]
fn changes_are_classified_by_version_revision_and_variant() {
    let dynamic = Variant::Dynamic;
    let old = map(vec![
        pkg("firefox", "143.0.1", 1, dynamic, b"f1"),
        pkg("mesa", "25.2.5", 1, dynamic, b"m1"),
        pkg("ripgrep", "14.1.1", 1, dynamic, b"r1"),
        pkg("zlib", "1.3", 1, dynamic, b"z1"),
        pkg("gone", "1", 1, dynamic, b"g1"),
    ]);
    let new = map(vec![
        pkg("firefox", "144.0", 1, dynamic, b"f2"),
        pkg("mesa", "25.2.4", 1, dynamic, b"m2"),
        pkg("ripgrep", "14.1.1", 1, Variant::Static, b"r2"),
        pkg("zlib", "1.3", 1, dynamic, b"z1"),
        pkg("seatd", "0.9", 1, dynamic, b"s1"),
    ]);
    let changes = changes(&old, &new);
    let kinds: Vec<(&str, &str)> = changes
        .iter()
        .map(|c| match c {
            Change::Added(n) => ("added", n.name.as_str()),
            Change::Removed(o) => ("removed", o.name.as_str()),
            Change::Upgraded(_, n) => ("upgraded", n.name.as_str()),
            Change::Downgraded(_, n) => ("downgraded", n.name.as_str()),
            Change::Changed(_, n) => ("changed", n.name.as_str()),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("upgraded", "firefox"),
            ("removed", "gone"),
            ("downgraded", "mesa"),
            ("changed", "ripgrep"),
            ("added", "seatd"),
        ]
    );
}

#[test]
fn a_new_revision_is_an_upgrade() {
    let old = map(vec![pkg("vim", "9.1", 1, Variant::Dynamic, b"a")]);
    let new = map(vec![pkg("vim", "9.1", 2, Variant::Dynamic, b"b")]);
    assert!(matches!(changes(&old, &new)[..], [Change::Upgraded(..)]));
}
