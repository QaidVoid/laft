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

#[test]
fn a_build_path_across_two_reads_is_found() {
    let dir = laft_test::tempdir();
    let path = dir.path().join("big");
    for at in [CHUNK - 3, CHUNK, 3 * CHUNK + 1] {
        let mut data = vec![b'x'; 4 * CHUNK];
        data[at..at + BUILD_PATH.len()].copy_from_slice(BUILD_PATH);
        fs::write(&path, &data).unwrap();
        assert!(contains(&path, BUILD_PATH).unwrap(), "{at}");
    }
    fs::write(&path, vec![b'x'; 2 * CHUNK]).unwrap();
    assert!(!contains(&path, BUILD_PATH).unwrap());
}
