use std::collections::BTreeMap;

use super::*;
use crate::test_support::{elf_fixture, link, recipe, subpackage, write_tree};

fn by_suffix(parts: &[Part]) -> BTreeMap<String, Vec<String>> {
    parts
        .iter()
        .map(|p| {
            (
                p.suffix.clone().unwrap_or_else(|| "main".into()),
                p.paths.iter().cloned().collect(),
            )
        })
        .collect()
}

fn run(recipe: &Recipe, setup: impl FnOnce(&Path)) -> BTreeMap<String, Vec<String>> {
    let dir = laft_test::tempdir();
    setup(dir.path());
    let elves = crate::post::scan_elf(dir.path(), &mut Vec::new()).unwrap();
    by_suffix(&split(dir.path(), recipe, &elves).unwrap())
}

#[test]
fn files_go_to_the_automatic_subpackages() {
    let parts = run(&recipe("zlib"), |out| {
        write_tree(
            out,
            &[
                ("usr/lib/libz.so.1.3", &elf_fixture("libversioned.so")),
                ("usr/include/zlib.h", b"h"),
                ("usr/lib/pkgconfig/zlib.pc", b"pc"),
                ("usr/lib/debug/.build-id/ab/cd.debug", b"dbg"),
                ("usr/share/doc/zlib/README", b"doc"),
                ("usr/share/man/man3/zlib.3", b"man3"),
                ("usr/share/man/man1/z.1", b"man1"),
                ("usr/share/licenses/zlib/LICENSE", b"l"),
                ("usr/share/locale/de/z.mo", b"mo"),
                ("usr/lib/libscript.so", b"INPUT(libz.so.1)"),
            ],
        );
        link(out, "usr/lib/libversioned.so.1", "libz.so.1.3");
        link(out, "usr/lib/libz.so", "libz.so.1.3");
    });
    assert_eq!(
        parts,
        BTreeMap::from([
            (
                "main".into(),
                vec![
                    "usr/lib/libversioned.so.1".to_owned(),
                    "usr/lib/libz.so.1.3".to_owned(),
                    "usr/share/licenses/zlib/LICENSE".to_owned(),
                    "usr/share/locale/de/z.mo".to_owned(),
                    "usr/share/man/man1/z.1".to_owned(),
                ]
            ),
            (
                "dev".into(),
                vec![
                    "usr/include/zlib.h".to_owned(),
                    "usr/lib/libscript.so".to_owned(),
                    "usr/lib/libz.so".to_owned(),
                    "usr/lib/pkgconfig/zlib.pc".to_owned(),
                    "usr/share/man/man3/zlib.3".to_owned(),
                ]
            ),
            (
                "dbg".into(),
                vec!["usr/lib/debug/.build-id/ab/cd.debug".to_owned()]
            ),
            ("doc".into(), vec!["usr/share/doc/zlib/README".to_owned()]),
        ])
    );
}

#[test]
fn explicit_subpackages_come_first_and_respect_exclude() {
    let mut recipe = recipe("openssl");
    let mut libs = subpackage();
    libs.exclude = vec!["usr/lib/*.so.1".into()];
    recipe.outputs.subpackages.insert("libs".into(), libs);
    let mut empty = subpackage();
    empty.files = vec!["usr/none/**".into()];
    empty.keep_empty = true;
    recipe.outputs.subpackages.insert("extra".into(), empty);
    recipe.outputs.subpackages.insert("doc".into(), {
        let mut doc = subpackage();
        doc.files = vec!["usr/share/doc/openssl/html/**".into()];
        doc
    });
    let parts = run(&recipe, |out| {
        write_tree(
            out,
            &[
                ("usr/lib/libssl.so.3", b"ssl"),
                ("usr/lib/libold.so.1", b"old"),
                ("usr/share/doc/openssl/html/index.html", b"html"),
                ("usr/share/doc/openssl/README", b"readme"),
            ],
        );
    });
    assert_eq!(parts["libs"], ["usr/lib/libssl.so.3"]);
    assert_eq!(parts["doc"], ["usr/share/doc/openssl/html/index.html"]);
    assert_eq!(
        parts["main"],
        ["usr/lib/libold.so.1", "usr/share/doc/openssl/README"]
    );
    assert!(parts["extra"].is_empty());
}

#[test]
fn empty_directories_go_unless_kept() {
    let mut recipe = recipe("x");
    recipe.options.keep_dirs = vec!["etc/x.d".into()];
    let parts = run(&recipe, |out| {
        write_tree(out, &[("usr/bin/x", b"x")]);
        fs::create_dir_all(out.join("etc/x.d")).unwrap();
        fs::create_dir_all(out.join("usr/share/empty/deeper")).unwrap();
    });
    assert_eq!(parts["main"], ["etc/x.d", "usr/bin/x"]);
}
