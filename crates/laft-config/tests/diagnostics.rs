//! Mistakes that the language catches are reported with the closest valid
//! name.

mod support;

use support::{codes, config_dir, load, load_body, pointed_at};

fn help(report: &laft_config::Report) -> Option<String> {
    report.iter().next().and_then(|d| d.diagnostic.help.clone())
}

#[test]
fn a_misspelled_group_suggests_the_declared_one() {
    let report = load_body("users { user alice { groups = [wheels] } }").expect_err("no wheels");
    assert_eq!(codes(&report), ["E0802"]);
    assert_eq!(pointed_at(&report, 0), "wheels");
    assert_eq!(help(&report).as_deref(), Some("did you mean `wheel`?"));
}

#[test]
fn a_misspelled_init_suggests_a_variant() {
    let report = load_body("init = sx").expect_err("no sx");
    assert_eq!(codes(&report), ["E0302"]);
    assert!(
        help(&report).is_some_and(|h| h.contains("`s6`")),
        "{report}"
    );
}

#[test]
fn a_misspelled_block_suggests_the_known_one() {
    let report = load_body(r#"servise foo { exec = ["/usr/bin/foo"] }"#).expect_err("no servise");
    assert_eq!(codes(&report), ["E0440"]);
    assert!(
        help(&report).is_some_and(|h| h.contains("`service`")),
        "{report}"
    );
}

#[test]
fn a_misspelled_field_suggests_the_known_one() {
    let report = load_body(r#"hostnme = "forge""#).expect_err("no hostnme");
    assert!(
        help(&report).is_some_and(|h| h.contains("`hostname`")),
        "{report}"
    );
}

#[test]
fn a_misspelled_dependency_suggests_a_declared_service() {
    let report = load_body(r#"service foo { exec = ["/usr/bin/foo"], after = [udevx] }"#)
        .expect_err("no udevx");
    assert_eq!(codes(&report), ["E0802"]);
    assert!(
        help(&report).is_some_and(|h| h.contains("`udevd`")),
        "{report}"
    );
}

#[test]
fn a_host_name_that_is_not_one_label_violates_its_refinement() {
    let report = load_body(r#"hostname = "my_box""#).expect_err("invalid host name");
    assert_eq!(codes(&report), ["E0803"]);
    assert_eq!(pointed_at(&report, 0), r#""my_box""#);
}

#[test]
fn a_misspelled_catalog_part_suggests_the_known_one() {
    let text = format!(
        "import \"std/services\" as services\nsystem {{\n{}\ninclude services.sshdd()\n}}\n",
        support::DISKS
    );
    let dir = config_dir(&[("system.stav", &text)]);
    let report = load(dir.path()).expect_err("no sshdd");
    assert!(
        help(&report).is_some_and(|h| h.contains("`sshd`")),
        "{report}"
    );
}
