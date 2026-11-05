use std::fs;

use stav::{ImportRoot, Limits, RootSpec};

use super::*;

/// Writes the configuration of one repository called `core`, whose
/// profile is `profile`.
fn write_repo_config(dir: &std::path::Path, profile: &str) {
    let key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    let text = format!(
        "system {{\n  fs \"/\" {{ device = \"UUID=1\", fstype = ext4 }}\n  \
         fs \"/boot\" {{ device = \"UUID=2\", fstype = vfat }}\n  repo core {{\n      \
         url = \"https://example.com/repo\"\n      mirrors = []\n      key = \"{key}\"\n      \
         keys = []\n      channel = current\n      profile = \"{profile}\"\n      \
         priority = 10\n      enable = true\n      allow_http = false\n  }}\n}}\n"
    );
    fs::write(dir.join("system.stav"), text).expect("the entry file can be written");
}

#[test]
fn a_profile_must_be_written_like_an_option_name() {
    let dir = laft_test::tempdir();
    write_repo_config(dir.path(), "web-server");
    let error = crate::repos(dir.path()).expect_err("web-server is not an option name");
    assert!(error.to_string().contains("profile"), "{error}");
    write_repo_config(dir.path(), "web_server");
    let repos = crate::repos(dir.path()).expect("web_server is an option name");
    let core: Name = "core".parse().expect("a name");
    let web_server: Name = "web_server".parse().expect("a name");
    assert_eq!(repos[&core].profile, Some(web_server));
}

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
    let module = crate::options::module(&crate::IndexOptions::default());
    let checked = stav::check(&host::host(dir.path(), &module), &root).expect("the file checks");
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
