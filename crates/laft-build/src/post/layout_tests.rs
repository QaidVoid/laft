use super::*;
use crate::test_support::{elf_fixture, link, write_tree};

fn paths(out: &Path) -> Vec<String> {
    fsx::walk(out)
        .unwrap()
        .into_iter()
        .filter(|(_, k)| *k != Kind::Dir)
        .map(|(r, _)| r)
        .collect()
}

#[test]
fn an_install_without_files_fails_unless_the_package_is_empty() {
    let dir = laft_test::tempdir();
    fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
    let error = check_output(dir.path(), false).expect_err("nothing was installed");
    assert_eq!(error.code(), Some("B101"));
    check_output(dir.path(), true).expect("an empty package needs no files");
    write_tree(dir.path(), &[("usr/bin/x", b"x")]);
    check_output(dir.path(), false).expect("a file was installed");
}

#[test]
fn paths_move_into_usr_and_unwanted_files_go() {
    let dir = laft_test::tempdir();
    let out = dir.path();
    write_tree(
        out,
        &[
            ("bin/a", b"a"),
            ("sbin/b", b"b"),
            ("lib/libx.so.1", b"x"),
            ("usr/sbin/c", b"c"),
            ("usr/lib64/liby.so.1", b"y"),
            ("usr/lib/libx.la", b"la"),
            ("usr/share/info/dir", b"index"),
            ("usr/share/info/x.info", b"info"),
            ("usr/lib/perl5/perllocal.pod", b"pod"),
            ("usr/lib/perl5/auto/X/.packlist", b"list"),
        ],
    );
    link(out, "lib64", "lib");
    normalize_paths(out).expect("the paths normalize");
    assert_eq!(
        paths(out),
        [
            "usr/bin/a",
            "usr/bin/b",
            "usr/bin/c",
            "usr/lib/libx.so.1",
            "usr/lib/liby.so.1",
            "usr/share/info/x.info",
        ]
    );
}

#[test]
fn a_program_in_sbin_and_bin_is_a_collision() {
    let dir = laft_test::tempdir();
    write_tree(dir.path(), &[("usr/sbin/foo", b"1"), ("usr/bin/foo", b"2")]);
    let error = normalize_paths(dir.path()).expect_err("both exist");
    assert_eq!(error.code(), Some("B102"));
    assert!(
        error
            .to_string()
            .contains("usr/sbin/foo and usr/bin/foo both exist")
    );
}

#[test]
fn paths_outside_usr_and_etc_are_forbidden() {
    for path in ["var/lib/foo", "usr/local/bin/foo", "opt/x"] {
        let dir = laft_test::tempdir();
        write_tree(dir.path(), &[(path, b"x"), ("usr/bin/ok", b"ok")]);
        let error = check_paths(dir.path(), Variant::Dynamic, false, &mut Vec::new())
            .expect_err("the path is forbidden");
        assert_eq!(error.code(), Some("B103"));
        assert!(
            error
                .to_string()
                .contains(&format!("forbidden path {path}")),
            "{error}"
        );
    }
}

#[test]
fn static_archives_leave_dynamic_payloads_with_a_warning() {
    let dir = laft_test::tempdir();
    let files: &[(&str, &[u8])] = &[("usr/lib/libz.a", b"!<arch>\n"), ("etc/z.conf", b"z")];
    write_tree(dir.path(), files);
    let mut warnings = Vec::new();
    check_paths(dir.path(), Variant::Dynamic, false, &mut warnings).unwrap();
    assert_eq!(paths(dir.path()), ["etc/z.conf"]);
    assert_eq!(warnings[0].code, "W110");

    for (variant, keep) in [(Variant::Dynamic, true), (Variant::Static, false)] {
        let dir = laft_test::tempdir();
        write_tree(dir.path(), files);
        let mut warnings = Vec::new();
        check_paths(dir.path(), variant, keep, &mut warnings).unwrap();
        assert!(warnings.is_empty());
        assert!(dir.path().join("usr/lib/libz.a").exists());
    }
}

#[test]
fn a_static_payload_moves_out_of_the_sysroot() {
    let dir = laft_test::tempdir();
    let out = dir.path();
    let s = "usr/x86_64-laft-linux-musl/usr";
    let program = elf_fixture("static");
    write_tree(
        out,
        &[
            (&format!("{s}/bin/tool"), &program),
            (&format!("{s}/share/man/man1/tool.1"), b"man"),
            (&format!("{s}/share/doc/tool/README"), b"doc"),
            (&format!("{s}/share/locale/de/tool.mo"), b"mo"),
            (&format!("{s}/lib/libtool.a"), b"!<arch>\n"),
        ],
    );
    static_layout(out).expect("the layout is valid");
    assert_eq!(
        paths(out),
        [
            "usr/bin/tool".to_owned(),
            "usr/share/man/man1/tool.1".to_owned(),
            format!("{s}/lib/libtool.a"),
        ]
    );
}

#[test]
fn a_static_payload_has_no_shared_object_and_no_dynamic_program() {
    let dir = laft_test::tempdir();
    write_tree(
        dir.path(),
        &[("usr/lib/libv.so.1", &elf_fixture("libversioned.so"))],
    );
    assert_eq!(static_layout(dir.path()).unwrap_err().code(), Some("B104"));

    let dir = laft_test::tempdir();
    write_tree(dir.path(), &[("usr/bin/program", &elf_fixture("program"))]);
    assert_eq!(static_layout(dir.path()).unwrap_err().code(), Some("B105"));
}

#[test]
fn files_that_views_generate_are_removed() {
    let dir = laft_test::tempdir();
    write_tree(
        dir.path(),
        &[
            ("usr/share/glib-2.0/schemas/gschemas.compiled", b"c"),
            ("usr/share/glib-2.0/schemas/org.x.gschema.xml", b"x"),
            ("usr/lib/modules/6.17.5-laft1/modules.dep", b"d"),
            ("usr/lib/modules/6.17.5-laft1/kernel/x.ko", b"k"),
        ],
    );
    let mut warnings = Vec::new();
    strip_trigger_outputs(dir.path(), &mut warnings).unwrap();
    assert_eq!(
        paths(dir.path()),
        [
            "usr/lib/modules/6.17.5-laft1/kernel/x.ko",
            "usr/share/glib-2.0/schemas/org.x.gschema.xml",
        ]
    );
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, "W113");
    assert!(warnings[0].message.contains("modules.dep"));
}
