use std::fs;

use stav::{ImportRoot, Limits, RootSpec};

use super::*;

#[test]
fn a_location_inside_a_character_names_that_character() {
    let dir = laft_test::tempdir();
    let entry = dir.path().join("system.stav");
    let text = "# \u{e9}\nsystem {\n  fs \"/\" { device = \"UUID=1\", fstype = ext4 }\n  \
                fs \"/boot\" { device = \"UUID=2\", fstype = vfat }\n}\n";
    fs::write(&entry, text).expect("the entry file can be written");
    let root = RootSpec {
        root_struct: "Root".to_owned(),
        entries: vec![entry.clone()],
        import_roots: vec![ImportRoot::new(dir.path())],
    };
    let checked = stav::check(&host::host(dir.path()), &root).expect("the file checks");
    let evaluated = checked
        .eval(&Limits::default())
        .expect("the file evaluates");
    let cx = Cx::new(&checked, &evaluated);
    let (file, _) = checked
        .files()
        .find(|&(_, name)| Some(name) == entry.to_str())
        .expect("the entry file is loaded");
    let at = |start| {
        cx.location(SourceSpan {
            file,
            span: Span { start, end: start },
        })
    };
    assert!(at(2).ends_with(":1:3"), "{}", at(2));
    assert!(at(3).ends_with(":1:3"), "{}", at(3));
    assert!(at(5).ends_with(":2:1"), "{}", at(5));
}
