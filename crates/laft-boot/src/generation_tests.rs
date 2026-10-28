use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use super::{Generation, numbers, read, read_link, select};

const RUNNING: &str = "6.17.5-laft1";

fn write_generation(gens: &Path, n: u64, release: &str) {
    let dir = gens.join(n.to_string());
    fs::create_dir_all(dir.join("usr/bin")).unwrap();
    fs::create_dir_all(dir.join("boot")).unwrap();
    fs::write(
        dir.join("gen.json"),
        format!(
            r#"{{"format":1,"generation":{n},"init":{{"backend":"runit","exec":["/usr/bin/runit"]}},"packages":[]}}"#
        ),
    )
    .unwrap();
    fs::write(
        dir.join("boot/boot.json"),
        format!(
            r#"{{"cmdline":[],"firmware":[],"initramfs":"i{n}","kernel_id":"k{n}","kernel_release":"{release}","modules":[],"version":1}}"#
        ),
    )
    .unwrap();
}

fn generation(n: u64, release: &str) -> Generation {
    Generation {
        number: n,
        backend: "runit".into(),
        exec: vec!["/usr/bin/runit".into()],
        kernel_id: format!("k{n}"),
        kernel_release: release.into(),
        initramfs: format!("i{n}"),
    }
}

fn lookup(gens: &[(u64, &str)]) -> impl Fn(u64) -> Result<Generation, String> {
    let gens: BTreeMap<u64, Generation> = gens
        .iter()
        .map(|&(n, release)| (n, generation(n, release)))
        .collect();
    move |n| gens.get(&n).cloned().ok_or_else(|| "missing".to_owned())
}

#[test]
fn complete_generation_is_read() {
    let tmp = laft_test::tempdir();
    write_generation(tmp.path(), 42, RUNNING);
    assert_eq!(read(tmp.path(), 42), Ok(generation(42, RUNNING)));
}

#[test]
fn incomplete_generations_are_not_valid() {
    let tmp = laft_test::tempdir();
    let gens = tmp.path();
    assert!(read(gens, 1).is_err());

    write_generation(gens, 2, RUNNING);
    fs::remove_dir(gens.join("2/usr/bin")).unwrap();
    assert!(read(gens, 2).unwrap_err().contains("usr/bin"));

    write_generation(gens, 3, RUNNING);
    fs::remove_file(gens.join("3/boot/boot.json")).unwrap();
    assert!(read(gens, 3).unwrap_err().contains("boot.json"));

    write_generation(gens, 4, RUNNING);
    fs::rename(gens.join("4"), gens.join("5")).unwrap();
    assert!(read(gens, 5).unwrap_err().contains("another number"));

    write_generation(gens, 6, RUNNING);
    fs::write(
        gens.join("6/gen.json"),
        r#"{"format":1,"generation":6,"init":{"backend":"run/it","exec":["/usr/bin/runit"]}}"#,
    )
    .unwrap();
    assert!(read(gens, 6).unwrap_err().contains("backend"));

    write_generation(gens, 7, RUNNING);
    fs::write(
        gens.join("7/gen.json"),
        r#"{"format":1,"generation":7,"init":{"backend":"runit","exec":[]}}"#,
    )
    .unwrap();
    assert!(read(gens, 7).unwrap_err().contains("PID 1"));
}

#[test]
fn default_link_names_a_number_or_nothing() {
    let tmp = laft_test::tempdir();
    let link = tmp.path().join("default");
    assert_eq!(read_link(&link), Ok(None));
    symlink("42", &link).unwrap();
    assert_eq!(read_link(&link), Ok(Some(42)));
    fs::remove_file(&link).unwrap();
    symlink("../elsewhere", &link).unwrap();
    assert!(read_link(&link).is_err());
}

#[test]
fn requested_generation_wins_over_the_default() {
    let selection = select(
        Some(41),
        Some(42),
        RUNNING,
        lookup(&[(41, RUNNING), (42, RUNNING)]),
    );
    let selection = selection.expect("selectable");
    assert_eq!(selection.generation.number, 41);
    assert!(selection.warnings.is_empty());
}

#[test]
fn default_is_booted_without_a_request() {
    let selection = select(None, Some(42), RUNNING, lookup(&[(42, RUNNING)])).expect("selectable");
    assert_eq!(selection.generation.number, 42);
}

#[test]
fn kernel_mismatch_boots_with_a_warning() {
    let selection =
        select(Some(41), Some(42), RUNNING, lookup(&[(41, "6.16.0-laft1")])).expect("selectable");
    assert_eq!(selection.generation.number, 41);
    assert_eq!(
        selection.warnings,
        ["generation 41 is built for kernel 6.16.0-laft1 but kernel 6.17.5-laft1 is running"]
    );
}

#[test]
fn invalid_request_falls_back_to_a_default_for_the_running_kernel() {
    let selection = select(Some(40), Some(42), RUNNING, lookup(&[(42, RUNNING)]));
    let selection = selection.expect("selectable");
    assert_eq!(selection.generation.number, 42);
    assert_eq!(selection.warnings.len(), 1);
    assert!(selection.warnings[0].contains("generation 40 cannot be booted"));
}

#[test]
fn default_for_another_kernel_does_not_replace_an_invalid_request() {
    let err = select(Some(40), Some(42), RUNNING, lookup(&[(42, "6.16.0-laft1")])).unwrap_err();
    assert_eq!(err, "generation 40 cannot be booted: missing");
}

#[test]
fn nothing_to_boot_is_an_error() {
    assert!(select(None, None, RUNNING, lookup(&[(1, RUNNING)])).is_err());
    assert!(select(None, Some(3), RUNNING, lookup(&[(1, RUNNING)])).is_err());
}

#[test]
fn generation_numbers_are_listed_and_an_unreadable_directory_is_an_error() {
    let tmp = laft_test::tempdir();
    let gens = tmp.path().join("gens");
    assert!(numbers(&gens).is_err());
    write_generation(&gens, 41, RUNNING);
    write_generation(&gens, 42, RUNNING);
    fs::create_dir(gens.join("tmp")).unwrap();
    symlink("42", gens.join("default")).unwrap();
    assert_eq!(numbers(&gens).unwrap(), BTreeSet::from([41, 42]));
}
