//! What a loaded configuration records about itself, and evaluation
//! without validation.

mod support;

use support::{DISKS, any_package, config_dir};

#[test]
fn the_files_read_are_listed_with_their_hashes_and_nothing_else() {
    let dir = config_dir(&[
        (
            "system.stav",
            &format!("system {{\n  include \"./net.stav\"\n{DISKS}\n}}\n"),
        ),
        ("net.stav", "hostname = \"forge\"\n"),
        ("unused.stav", "hostname = \"never\"\n"),
    ]);
    let loaded = laft_config::load(dir.path(), &any_package).expect("valid");
    let paths: Vec<&str> = loaded.files.iter().map(|f| f.path.as_str()).collect();
    let expected = [dir.path().join("net.stav"), dir.path().join("system.stav")];
    let expected: Vec<String> = expected
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert_eq!(paths, expected);
    assert_eq!(
        loaded.files[0].hash,
        laft_core::Blake3Hash::of(b"hostname = \"forge\"\n")
    );
}

#[test]
fn the_value_is_canonical_json_of_the_evaluated_system() {
    let dir = config_dir(&[(
        "system.stav",
        &format!("system {{\n  hostname = \"forge\"\n{DISKS}\n}}\n"),
    )]);
    let loaded = laft_config::load(dir.path(), &any_package).expect("valid");
    let value: serde_json::Value = serde_json::from_slice(&loaded.value).expect("JSON");
    assert_eq!(value["hostname"], "forge");
    assert!(value["boot"]["tries"].is_number(), "{value}");
    let again = laft_config::load(dir.path(), &any_package).expect("valid");
    assert_eq!(
        loaded.value, again.value,
        "the same input gives the same bytes"
    );
}

#[test]
fn evaluation_skips_validation_and_package_resolution() {
    let body = format!("system {{\n  packages = [missing-tool]\n{DISKS}\n}}\n");
    let dir = config_dir(&[("system.stav", &body)]);
    let evaluation =
        laft_config::evaluate(dir.path(), &dir.path().join("system.stav")).expect("evaluates");
    let rendered = evaluation
        .evaluated
        .render_at(
            &stav::AttrPath::new(["system", "packages"]),
            stav::Format::Json,
        )
        .expect("renders");
    assert!(rendered.contains("missing-tool"), "{rendered}");
    let Err(report) = laft_config::load(dir.path(), &any_package) else {
        panic!("loading resolves packages");
    };
    assert!(
        report
            .iter()
            .any(|d| d.diagnostic.message.contains("no package `missing-tool`")),
        "{report:?}"
    );
}

#[test]
fn evaluation_reports_type_errors() {
    let dir = config_dir(&[("part.stav", "system { hostname = 3 }\n")]);
    let report = laft_config::evaluate(dir.path(), &dir.path().join("part.stav"))
        .expect_err("a hostname is a string");
    assert!(report.has_errors());
}
