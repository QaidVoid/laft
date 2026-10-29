use super::*;
use crate::test_support::{link, write_tree};

fn lint_tree(setup: impl FnOnce(&Path)) -> (Result<(), BuildError>, Vec<&'static str>) {
    let dir = laft_test::tempdir();
    write_tree(dir.path(), &[("usr/share/licenses/x/LICENSE", b"l")]);
    setup(dir.path());
    let mut warnings = Vec::new();
    let result = lint(dir.path(), &["usr/bin/ww".to_owned()], &mut warnings);
    (result, warnings.iter().map(|w| w.code).collect())
}

#[test]
fn a_symlink_into_the_build_fails() {
    let (result, _) = lint_tree(|out| link(out, "usr/lib/x", "/build/obj/x"));
    assert_eq!(result.unwrap_err().code(), Some("B109"));
}

#[test]
fn suspicious_contents_are_warnings() {
    let (result, codes) = lint_tree(|out| {
        link(out, "usr/bin/opt", "/opt/thing");
        link(out, "usr/bin/ok", "/usr/bin/x");
        write_tree(
            out,
            &[
                ("usr/lib/leaky", b"path /build/src/x.c"),
                ("usr/lib/debug/x.debug", b"/build/src/x.c"),
            ],
        );
    });
    result.unwrap();
    assert_eq!(codes, ["W120", "W121", "W122"]);
}

#[test]
fn a_payload_without_licenses_is_reported() {
    let dir = laft_test::tempdir();
    write_tree(dir.path(), &[("usr/bin/x", b"x")]);
    let mut warnings = Vec::new();
    lint(dir.path(), &[], &mut warnings).unwrap();
    assert_eq!(
        warnings.iter().map(|w| w.code).collect::<Vec<_>>(),
        ["W101"]
    );
}
