use super::*;

#[test]
fn package_references_are_written_canonically() {
    assert_eq!(parse_pkg("llvm@19:dynamic").as_deref(), Ok("llvm@19"));
    assert_eq!(parse_pkg("ripgrep:static").as_deref(), Ok("ripgrep:static"));
    assert!(parse_pkg("ripgrep:shared").is_err());
    assert!(parse_pkg("Ripgrep").is_err());
}

/// The services of the base part, rendered as JSON.
fn base_services() -> String {
    let dir = laft_test::tempdir();
    let entry = dir.path().join("system.stav");
    std::fs::write(&entry, "system { }\n").expect("the entry file can be written");
    let root = stav::RootSpec {
        root_struct: "Root".to_owned(),
        entries: vec![entry],
        import_roots: vec![stav::ImportRoot::new(dir.path())],
    };
    let evaluated = stav::check(&host(dir.path()), &root)
        .and_then(|checked| checked.eval(&stav::Limits::default()))
        .unwrap_or_else(|e| panic!("the base part evaluates:\n{e}"));
    let services = evaluated
        .value()
        .at(&stav::AttrPath::new(["system", "services"]))
        .expect("the base part declares services");
    services
        .render(stav::Format::Json)
        .expect("JSON renders every value")
}

#[test]
fn the_standard_services_fixture_is_rendered_from_the_base_part() {
    let fixture = include_str!("../../laft-svc/tests/fixtures/standard-services.json");
    let rendered = base_services();
    assert!(
        fixture == rendered,
        "replace crates/laft-svc/tests/fixtures/standard-services.json with:\n{rendered}"
    );
    let parsed: std::collections::BTreeMap<String, laft_svc::Service> =
        serde_json::from_str(fixture).expect("the fixture deserializes");
    assert_eq!(parsed.len(), 15);
}
