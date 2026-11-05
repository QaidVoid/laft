//! Tests of the pure parts of a build run: requests, constraints, group
//! order, and snapshot verification.

use super::*;
use laft_core::{Blake3Hash, OptionValue, Variant};
use laft_resolve::{BuildRequest, Buildable, Reason};
use std::fs;

/// A buildable package, as a resolution reports it.
fn buildable(name: &str, version: &str, variant: Variant) -> Buildable {
    Buildable {
        name: name.parse().expect("a name"),
        version: version.parse().expect("a version"),
        rev: 1,
        variant,
        defaults: BTreeMap::new(),
    }
}

/// A resolution with builds and nothing else.
fn resolution(view: &str, builds: Vec<BuildRequest>) -> laft_resolve::Resolution<'static> {
    laft_resolve::Resolution {
        view: view.to_owned(),
        packages: BTreeMap::new(),
        alternatives: BTreeMap::new(),
        learned: Vec::new(),
        warnings: Vec::new(),
        builds,
    }
}

/// A build request for `name`, built with one bool option.
fn request_of(name: &str, llvm: bool) -> BuildRequest {
    BuildRequest {
        buildable: buildable(name, "1.0", Variant::default()),
        values: BTreeMap::from([("llvm".to_owned(), OptionValue::Bool(llvm))]),
        reasons: vec![Reason::Requested {
            origin: "/etc/laft/system.stav:1:1".to_owned(),
            text: name.to_owned(),
        }],
    }
}

#[test]
fn a_request_is_written_as_an_item_with_sets_and_a_signature() {
    let values = BTreeMap::from([
        ("llvm".to_owned(), OptionValue::Bool(true)),
        (
            "backends".to_owned(),
            OptionValue::Tag("x11".parse().expect("a tag")),
        ),
    ]);
    let mut request = Request {
        name: "mesa".parse().expect("a name"),
        version: "25.2".parse().expect("a version"),
        variant: Variant::default(),
        values: values.clone(),
        view: "system".to_owned(),
        reasons: Vec::new(),
    };
    assert_eq!(request.item(), "mesa");
    assert_eq!(request.signature(), "backends=x11,llvm=true");
    assert_eq!(
        request.sets(),
        vec!["mesa.backends=x11".to_owned(), "mesa.llvm=true".to_owned()]
    );
    request.variant = Variant::Static;
    assert_eq!(request.item(), "mesa:static");
    let mut static_values = BTreeMap::new();
    static_values.insert("llvm".to_owned(), OptionValue::Bool(false));
    let static_request = Request {
        variant: Variant::Static,
        values: static_values,
        ..request
    };
    assert_eq!(static_request.signature(), "llvm=false");
}

#[test]
fn the_requests_of_a_resolution_come_in_view_order() {
    let resolved = crate::resolve::Resolved {
        system: resolution("system", vec![request_of("mesa", true)]),
        initrd: resolution("initrd", vec![request_of("busybox", false)]),
        views: BTreeMap::from([
            (
                "desk".parse().expect("a name"),
                resolution("desk", vec![request_of("wayland", false)]),
            ),
            (
                "cli".parse().expect("a name"),
                resolution("cli", vec![request_of("neovim", false)]),
            ),
        ]),
    };
    let requests = requests_of(&resolved);
    let views: Vec<&str> = requests
        .iter()
        .map(|request| request.view.as_str())
        .collect();
    assert_eq!(views, vec!["system", "initrd", "cli", "desk"]);
}

#[test]
fn a_constraint_sets_a_value_and_refuses_a_contradiction() {
    let reference = PkgRef::new_with_options(
        "mesa".parse().expect("a name"),
        None,
        Variant::default(),
        laft_core::OptionConstraints::from(vec![laft_core::OptionConstraint::True {
            name: "llvm".to_owned(),
        }]),
    );
    let mut values = BTreeMap::from([("llvm".to_owned(), OptionValue::Bool(false))]);
    constrain(
        &reference.options().as_slice()[0],
        &mut values,
        &BTreeSet::new(),
        &reference,
    )
    .expect("a constraint the values can take");
    assert_eq!(values.get("llvm"), Some(&OptionValue::Bool(true)));
    let fixed = BTreeSet::from(["llvm".to_owned()]);
    values.insert("llvm".to_owned(), OptionValue::Bool(false));
    let error = constrain(
        &reference.options().as_slice()[0],
        &mut values,
        &fixed,
        &reference,
    )
    .expect_err("a fixed value is refused");
    assert!(error.message.contains("fixes it to false"));
}

#[test]
fn groups_build_after_the_groups_whose_packages_they_wait_for() {
    let request = |name: &str| Request {
        name: name.parse().expect("a name"),
        version: "1.0".parse().expect("a version"),
        variant: Variant::default(),
        values: BTreeMap::new(),
        view: "system".to_owned(),
        reasons: Vec::new(),
    };
    let first = (Blake3Hash::of(b"zlib"), "zlib".to_owned());
    let second = (Blake3Hash::of(b"mesa"), "mesa".to_owned());
    let groups = BTreeMap::from([
        (first.clone(), vec![request("zlib")]),
        (second.clone(), vec![request("mesa")]),
    ]);
    let mut waiting = BTreeMap::new();
    waiting.insert(
        second.clone(),
        BTreeSet::from(["zlib".parse().expect("a name")]),
    );
    let ordered = order(&groups, &waiting).expect("an order exists");
    assert_eq!(ordered, vec![first.clone(), second.clone()]);
    let mut both = BTreeMap::new();
    both.insert(
        first.clone(),
        BTreeSet::from(["mesa".parse().expect("a name")]),
    );
    both.insert(
        second.clone(),
        BTreeSet::from(["zlib".parse().expect("a name")]),
    );
    assert!(order(&groups, &both).is_err());
}

#[test]
fn a_cached_snapshot_is_used_only_when_its_hash_matches() {
    let dir = laft_test::tempdir();
    let path = dir.path().join("snapshot.tar.zst");
    fs::write(&path, b"snapshot bytes").expect("writable");
    let source = SnapshotSource {
        repo: "core".parse().expect("a name"),
        snapshot: laft_index::RecipeSnapshot {
            file: "recipes/abc.tar.zst".to_owned(),
            size: 14,
            blake3: Blake3Hash::of(b"snapshot bytes"),
        },
        name: "mesa".parse().expect("a name"),
        version: "25.2".parse().expect("a version"),
        variant: Variant::default(),
        defaults: BTreeMap::new(),
    };
    assert!(verified(&path, &source));
    fs::write(&path, b"other bytes___").expect("writable");
    assert!(!verified(&path, &source));
}

#[test]
fn a_reason_is_one_line() {
    let requested = Reason::Requested {
        origin: "/etc/laft/system.stav:1:1".to_owned(),
        text: "mesa@25".to_owned(),
    };
    assert_eq!(reason_text(&requested), "mesa@25");
    let dependency = Reason::Dependency {
        by: "chromium".parse().expect("a name"),
        dep: "so:libGL.so.1".parse().expect("a dep"),
    };
    assert_eq!(
        reason_text(&dependency),
        "a dependency of chromium: so:libGL.so.1"
    );
}

#[test]
fn the_values_of_a_view_are_found_by_name() {
    let values = BTreeMap::from([(
        "system".parse::<Name>().expect("a name"),
        laft_config::OptionValues::default(),
    )]);
    assert!(view_values(&values, "system").is_some());
    assert!(view_values(&values, "desk").is_none());
}
